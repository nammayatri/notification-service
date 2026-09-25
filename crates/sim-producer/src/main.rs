/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

mod config;
mod metrics;
mod payload;
mod write;

use anyhow::{anyhow, Result};
use config::Config;
use governor::{Quota, RateLimiter};
use metrics::Metrics;
use notification_service::{common::types::DeliveryMode, environment::AppConfig};
use shared::redis::types::RedisConnectionPool;
use sim_common::{population, SUMMARY_INTERVAL};
use std::{sync::Arc, time::Duration};
use tokio::{sync::Semaphore, time::sleep};
use tracing::*;

async fn summary_looper(metrics: Arc<Metrics>) {
    loop {
        sleep(SUMMARY_INTERVAL).await;
        info!(
            "[sim-producer] write window {} | write total {}",
            metrics.write_percentiles.drain_window(),
            metrics.write_percentiles.total()
        );
    }
}

fn log_startup_fingerprint(config_path: &str, cfg: &Config, delivery_mode: DeliveryMode) {
    info!("[sim-producer] config={}", config_path);
    info!(
        "[sim-producer] delivery_mode={} target_rate={}/s fanout={} burst={} publish_enabled={} ttl={}s payload_bytes={} local={}..{} fleet={}..{} max_shards={} stream_expiration={}s",
        delivery_mode,
        cfg.target_rate,
        cfg.fanout,
        cfg.burst,
        cfg.publish_enabled,
        cfg.ttl_seconds,
        cfg.payload_bytes,
        cfg.client_index_start,
        cfg.client_index_start + cfg.client_count,
        cfg.client_index_start,
        cfg.client_index_start + cfg.fleet_client_count,
        cfg.max_shards,
        cfg.stream_expiration_seconds
    );
    population::log_population_fingerprint(
        "sim-producer",
        cfg.client_index_start,
        cfg.client_count,
        cfg.max_shards,
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let service_config_path = sim_common::config::service_config_path()?;
    let app_config = serde_dhall::from_file(&service_config_path).parse::<AppConfig>()?;

    let sim_config_path = sim_common::config::sim_config_path()?;
    let cfg = Arc::new(Config::from_dhall(&sim_config_path)?);
    let metrics = Metrics::new(cfg.target_rate)?;

    if cfg.max_shards != app_config.max_shards {
        warn!(
            "[Notification Service Error] - {} producer.max_shards={} disagrees with {} max_shards={}; every write will land on a stream key the service never reads",
            sim_config_path, cfg.max_shards, service_config_path, app_config.max_shards
        );
    }

    let delivery_mode =
        notification_service::environment::delivery_mode_from_env(app_config.delivery_mode);
    config::validate_delivery_path(delivery_mode, cfg.publish_enabled)?;

    log_startup_fingerprint(&sim_config_path, &cfg, delivery_mode);

    let redis_cfg =
        notification_service::environment::redis_settings_from_env(app_config.redis_cfg);
    info!(
        "[sim-producer] redis={}:{} cluster={}",
        redis_cfg.host, redis_cfg.port, redis_cfg.cluster_enabled
    );
    let redis_pool = Arc::new(RedisConnectionPool::new(redis_cfg, None).await?);

    tokio::spawn(sim_common::metrics::serve(
        metrics.registry.clone(),
        cfg.metrics_port,
    ));
    tokio::spawn(summary_looper(metrics.clone()));

    let search_interval = Duration::from_secs_f64(1.0 / cfg.search_rate());
    let quota = Quota::with_period(search_interval).ok_or_else(|| {
        anyhow!("producer.target_rate / producer.fanout produced a zero-length period")
    })?;
    let limiter = RateLimiter::direct(quota);
    let inflight = Arc::new(Semaphore::new(cfg.max_inflight));

    info!(
        "[sim-producer] {:.3} search requests/s, one every {:?}",
        cfg.search_rate(),
        search_interval
    );

    loop {
        limiter.until_ready().await;
        let permit = inflight.clone().acquire_owned().await?;
        let (redis_pool, cfg, metrics) = (redis_pool.clone(), cfg.clone(), metrics.clone());
        tokio::spawn(async move {
            write::emit_search_request(redis_pool, cfg, metrics, search_interval).await;
            drop(permit);
        });
    }
}
