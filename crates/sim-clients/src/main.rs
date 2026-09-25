/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

mod config;
mod distribution;
mod metrics;
mod seen;
mod stream;

use anyhow::Result;
use config::Config;
use metrics::Metrics;
use sim_common::{population, SUMMARY_INTERVAL};
use std::{sync::Arc, time::Duration};
use tokio::time::sleep;
use tracing::*;

async fn summary_looper(metrics: Arc<Metrics>) {
    loop {
        sleep(SUMMARY_INTERVAL).await;
        info!(
            "[sim-clients] delivery window {} | ack window {}",
            metrics.delivery_percentiles.drain_window(),
            metrics.ack_percentiles.drain_window()
        );
        info!(
            "[sim-clients] delivery total {} | ack total {}",
            metrics.delivery_percentiles.total(),
            metrics.ack_percentiles.total()
        );
    }
}

fn log_startup_fingerprint(config_path: &str, cfg: &Config) {
    info!("[sim-clients] config={}", config_path);
    info!(
        "[sim-clients] endpoints={:?} indices={}..{} ack_mode={:?} ack_delay_ms={} ack_percentiles={}/{}/{}/{} reconnect_mode={:?} reconnect_buckets={} never_ack_pct={} streams_per_connection={}",
        cfg.endpoints,
        cfg.client_index_start,
        cfg.client_index_start + cfg.client_count,
        cfg.ack_mode,
        cfg.ack_delay_ms,
        cfg.ack_delays.p50_ms,
        cfg.ack_delays.p90_ms,
        cfg.ack_delays.p95_ms,
        cfg.ack_delays.max_ms,
        cfg.reconnect_mode,
        cfg.reconnect_lifetimes.bucket_count(),
        cfg.never_ack_pct,
        cfg.streams_per_connection
    );
    population::log_population_fingerprint(
        "sim-clients",
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

    let config_path = sim_common::config::sim_config_path()?;
    let cfg = Arc::new(Config::from_dhall(&config_path)?);
    let metrics = Metrics::new()?;

    log_startup_fingerprint(&config_path, &cfg);

    stream::preflight_endpoints(&cfg.endpoints).await?;

    let group_count = cfg.client_count.div_ceil(cfg.streams_per_connection);
    let mut channels = Vec::with_capacity(group_count as usize);
    for group in 0..group_count {
        let endpoint = Arc::new(cfg.endpoints[(group as usize) % cfg.endpoints.len()].clone());
        channels.push((stream::build_channel(&endpoint)?, endpoint));
    }

    tokio::spawn(sim_common::metrics::serve(
        metrics.registry.clone(),
        cfg.metrics_port,
    ));
    tokio::spawn(summary_looper(metrics.clone()));

    let stagger = if cfg.connect_rate_per_sec > 0.0 {
        Duration::from_secs_f64(1.0 / cfg.connect_rate_per_sec)
    } else {
        Duration::ZERO
    };

    let mut clients = Vec::with_capacity(cfg.client_count as usize);
    for offset in 0..cfg.client_count {
        let (channel, endpoint) = &channels[(offset / cfg.streams_per_connection) as usize];
        let (cfg, metrics, channel, endpoint) = (
            cfg.clone(),
            metrics.clone(),
            channel.clone(),
            endpoint.clone(),
        );
        let index = cfg.client_index_start + offset;
        clients.push(tokio::spawn(async move {
            if !stagger.is_zero() {
                sleep(stagger * (offset as u32)).await;
            }
            stream::run_client(index, cfg, channel, endpoint, metrics).await;
        }));
    }

    for client in clients {
        let _ = client.await;
    }
    Ok(())
}
