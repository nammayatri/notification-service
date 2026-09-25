/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use crate::{config::Config, metrics::Metrics, payload};
use chrono::Utc;
use fred::{prelude::*, types::XID};
use notification_service::redis::keys::pubsub_channel_key;
use rand::Rng;
use shared::redis::types::RedisConnectionPool;
use sim_common::population;
use std::{sync::Arc, time::Duration};
use tokio::time::sleep;
use tracing::*;

const BURST_SPREAD: Duration = Duration::from_millis(100);

pub struct SearchTarget {
    pub index: u64,
    pub client_id: String,
    pub stream_key: String,
    pub delay: Duration,
    pub local: bool,
}

pub fn plan_search_request(cfg: &Config, search_interval: Duration) -> Vec<SearchTarget> {
    let population = cfg.fleet_client_count as usize;
    let fanout = (cfg.fanout as usize).min(population);
    let spread = if cfg.burst {
        BURST_SPREAD
    } else {
        search_interval
    };

    let mut rng = rand::thread_rng();
    let sampled = rand::seq::index::sample(&mut rng, population, fanout);

    sampled
        .into_iter()
        .enumerate()
        .map(|(position, offset)| {
            let index = cfg.client_index_start + offset as u64;
            let client_id = population::client_id(index);
            let shard = population::shard(&client_id, cfg.max_shards);
            let delay = if cfg.burst {
                Duration::from_secs_f64(rng.gen_range(0.0..spread.as_secs_f64()))
            } else {
                spread.mul_f64(position as f64 / fanout as f64)
            };
            SearchTarget {
                index,
                stream_key: population::stream_key(&client_id, shard),
                client_id,
                delay,
                local: (offset as u64) < cfg.client_count,
            }
        })
        .collect()
}

async fn write_notification(
    redis_pool: &Arc<RedisConnectionPool>,
    cfg: &Config,
    metrics: &Arc<Metrics>,
    target: SearchTarget,
    search_request_id: uuid::Uuid,
) {
    if !target.delay.is_zero() {
        sleep(target.delay).await;
    }

    let locality = if target.local { "local" } else { "remote" };
    let created_at = Utc::now();
    let offer = payload::build_offer(
        &search_request_id,
        created_at,
        cfg.ttl_seconds,
        cfg.payload_bytes,
    );

    let client = redis_pool.writer_pool.next();
    let started_at = std::time::Instant::now();
    let transaction = client.multi();
    let queued = async {
        transaction
            .xadd::<(), _, _, _, _>(
                target.stream_key.as_str(),
                false,
                None::<()>,
                XID::Auto,
                offer.fields.clone(),
            )
            .await?;
        transaction
            .expire::<(), _>(target.stream_key.as_str(), cfg.stream_expiration_seconds)
            .await?;
        transaction.exec::<()>(true).await
    }
    .await;

    match queued {
        Ok(()) => {
            let elapsed = started_at.elapsed();
            debug!(
                "[sim-producer] wrote notification {} to {}",
                offer.notification_id, target.stream_key
            );
            metrics
                .write_latency
                .with_label_values(&["xadd_expire"])
                .observe(elapsed.as_secs_f64());
            metrics
                .write_percentiles
                .record_micros(elapsed.as_micros() as u64);
            metrics.notifications.with_label_values(&[locality]).inc();
        }
        Err(err) => {
            metrics.errors.with_label_values(&["xadd_expire"]).inc();
            error!(
                "[Notification Service Error] - XADD/EXPIRE failed for index {} key {} : {}",
                target.index, target.stream_key, err
            );
            return;
        }
    }

    if cfg.publish_enabled {
        let message = payload::pubsub_message(&target.client_id, Utc::now());
        match client
            .publish::<(), _, _>(pubsub_channel_key(), message)
            .await
        {
            Ok(()) => {
                metrics.publishes.with_label_values(&[locality]).inc();
            }
            Err(err) => {
                metrics.errors.with_label_values(&["publish"]).inc();
                error!(
                    "[Notification Service Error] - PUBLISH failed for client {} : {}",
                    target.client_id, err
                );
            }
        }
    }
}

pub async fn emit_search_request(
    redis_pool: Arc<RedisConnectionPool>,
    cfg: Arc<Config>,
    metrics: Arc<Metrics>,
    search_interval: Duration,
) {
    let search_request_id = uuid::Uuid::new_v4();
    let targets = plan_search_request(&cfg, search_interval);

    metrics
        .search_requests
        .with_label_values(&[if cfg.burst { "burst" } else { "uniform" }])
        .inc();

    let writes = targets.into_iter().map(|target| {
        let (redis_pool, cfg, metrics) = (redis_pool.clone(), cfg.clone(), metrics.clone());
        tokio::spawn(async move {
            write_notification(&redis_pool, &cfg, &metrics, target, search_request_id).await;
        })
    });

    for write in writes.collect::<Vec<_>>() {
        let _ = write.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::load;
    use std::collections::HashSet;

    #[test]
    fn a_search_request_reaches_fanout_distinct_drivers() {
        let cfg = load();
        let targets = plan_search_request(&cfg, Duration::from_millis(50));

        assert_eq!(targets.len(), cfg.fanout as usize);
        let distinct: HashSet<&String> = targets.iter().map(|target| &target.client_id).collect();
        assert_eq!(distinct.len(), targets.len(), "a driver was picked twice");
    }

    #[test]
    fn every_target_falls_inside_the_fleet_index_range() {
        let mut cfg = load();
        cfg.fleet_client_count = 580_000;
        let end = cfg.client_index_start + cfg.fleet_client_count;
        for target in plan_search_request(&cfg, Duration::from_millis(50)) {
            assert!((cfg.client_index_start..end).contains(&target.index));
            assert_eq!(
                target.local,
                target.index < cfg.client_index_start + cfg.client_count
            );
        }
    }

    #[test]
    fn stream_keys_agree_with_the_population_formula() {
        let cfg = load();
        for target in plan_search_request(&cfg, Duration::from_millis(50)) {
            let expected_shard = population::shard(&target.client_id, cfg.max_shards);
            assert_eq!(
                target.stream_key,
                population::stream_key(&target.client_id, expected_shard)
            );
        }
    }

    #[test]
    fn burst_writes_land_inside_the_burst_spread() {
        let cfg = load();
        assert!(cfg.burst, "sim.dhall is expected to burst");
        for target in plan_search_request(&cfg, Duration::from_secs(10)) {
            assert!(target.delay < BURST_SPREAD, "{:?}", target.delay);
        }
    }

    #[test]
    fn a_fanout_wider_than_the_fleet_is_clamped_rather_than_panicking() {
        let mut cfg = load();
        cfg.fleet_client_count = 3;
        cfg.client_count = 3;
        cfg.fanout = 500;
        assert_eq!(
            plan_search_request(&cfg, Duration::from_millis(50)).len(),
            3
        );
    }
}
