/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use anyhow::{anyhow, Result};
use notification_service::common::types::DeliveryMode;
use serde::Deserialize;
use sim_common::config::{client_index_start, env_override};

#[derive(Debug, Deserialize)]
struct ProducerSection {
    target_rate: f64,
    client_count: u64,
    fleet_client_count: u64,
    client_index_start: u64,
    fanout: u64,
    burst: bool,
    publish_enabled: bool,
    ttl_seconds: i64,
    payload_bytes: usize,
    metrics_port: u16,
    max_shards: u64,
    stream_expiration_seconds: i64,
    max_inflight_search_requests: usize,
}

#[derive(Debug, Deserialize)]
struct SimFile {
    producer: ProducerSection,
}

pub struct Config {
    pub target_rate: f64,
    pub client_count: u64,
    pub fleet_client_count: u64,
    pub client_index_start: u64,
    pub fanout: u64,
    pub burst: bool,
    pub publish_enabled: bool,
    pub ttl_seconds: i64,
    pub payload_bytes: usize,
    pub metrics_port: u16,
    pub max_shards: u64,
    pub stream_expiration_seconds: i64,
    pub max_inflight: usize,
}

impl Config {
    pub fn from_dhall(path: &str) -> Result<Self> {
        let file = serde_dhall::from_file(path).parse::<SimFile>()?;
        Self::from_section(file.producer, path)
    }

    fn from_section(section: ProducerSection, source: &str) -> Result<Self> {
        let client_count = env_override("CLIENT_COUNT", section.client_count)?;
        let fleet_client_count = env_override("FLEET_CLIENT_COUNT", section.fleet_client_count)?;
        let target_rate = env_override("TARGET_RATE", section.target_rate)?;
        let publish_enabled = env_override("PUBLISH_ENABLED", section.publish_enabled)?;

        if client_count == 0 {
            return Err(anyhow!("{source}: producer.client_count must be > 0"));
        }
        if fleet_client_count < client_count {
            return Err(anyhow!(
                "{source}: producer.fleet_client_count must be >= producer.client_count"
            ));
        }
        if target_rate <= 0.0 {
            return Err(anyhow!("{source}: producer.target_rate must be > 0"));
        }
        if section.max_inflight_search_requests == 0 {
            return Err(anyhow!(
                "{source}: producer.max_inflight_search_requests must be > 0"
            ));
        }

        Ok(Self {
            target_rate,
            client_count,
            fleet_client_count,
            client_index_start: client_index_start(section.client_index_start)?,
            fanout: section.fanout.max(1),
            burst: section.burst,
            publish_enabled,
            ttl_seconds: section.ttl_seconds,
            payload_bytes: section.payload_bytes,
            metrics_port: section.metrics_port,
            max_shards: section.max_shards,
            stream_expiration_seconds: section.stream_expiration_seconds,
            max_inflight: section.max_inflight_search_requests,
        })
    }

    pub fn search_rate(&self) -> f64 {
        self.target_rate / self.fanout as f64
    }
}

pub fn validate_delivery_path(delivery_mode: DeliveryMode, publish_enabled: bool) -> Result<()> {
    if (delivery_mode == DeliveryMode::Pubsub) == publish_enabled {
        return Ok(());
    }
    Err(anyhow!(
        "delivery_mode={delivery_mode} disagrees with producer.publish_enabled={publish_enabled}. \
         A Pubsub cell needs the producer to PUBLISH or only the sweep delivers; a Sweep cell must \
         not PUBLISH or it is charged for a channel nothing reads. DELIVERY_MODE \
         (notification_service.dhall delivery_mode) and PUBLISH_ENABLED (sim.dhall \
         producer.publish_enabled) move together."
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn config_path() -> String {
        format!(
            "{}/../../dhall-configs/dev/sim.dhall",
            env!("CARGO_MANIFEST_DIR")
        )
    }

    pub(crate) fn load() -> Config {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        Config::from_dhall(&config_path()).expect("sim.dhall must parse")
    }

    #[test]
    fn the_config_parses_and_validates() {
        let cfg = load();
        assert!(cfg.client_count > 0);
        assert!(cfg.fleet_client_count >= cfg.client_count);
        assert!(cfg.target_rate > 0.0);
        assert!(cfg.fanout > 0);
        assert!(cfg.search_rate() > 0.0);
        assert!(
            cfg.stream_expiration_seconds > 0,
            "without an EXPIRE Redis reclaims nothing"
        );
        assert_eq!(cfg.metrics_port, 9102);
    }

    #[test]
    fn the_config_carries_the_prod_calibration() {
        let cfg = load();
        assert_eq!(cfg.max_shards, 128);
        assert_eq!(cfg.payload_bytes, 1400);
        assert_eq!(cfg.ttl_seconds, 30);
    }

    #[test]
    fn env_overrides_replace_the_file_values() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        std::env::set_var("CLIENT_COUNT", "19301");
        std::env::set_var("FLEET_CLIENT_COUNT", "580000");
        std::env::set_var("TARGET_RATE", "786.0");
        std::env::set_var("PUBLISH_ENABLED", "true");

        let parsed = Config::from_dhall(&config_path());

        for key in [
            "CLIENT_COUNT",
            "FLEET_CLIENT_COUNT",
            "TARGET_RATE",
            "PUBLISH_ENABLED",
        ] {
            std::env::remove_var(key);
        }

        let cfg = parsed.expect("overrides must parse");
        assert_eq!(cfg.client_count, 19_301);
        assert_eq!(cfg.fleet_client_count, 580_000);
        assert_eq!(cfg.target_rate, 786.0);
        assert!(cfg.publish_enabled);
    }

    #[test]
    fn an_unparseable_override_is_an_error_rather_than_a_silent_fallback() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        std::env::set_var("TARGET_RATE", "fast");
        let parsed = Config::from_dhall(&config_path());
        std::env::remove_var("TARGET_RATE");

        let err = match parsed {
            Ok(_) => panic!("a typo must not run the cell at the file's rate"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("TARGET_RATE"), "{err}");
    }

    #[test]
    fn a_fleet_narrower_than_the_connected_population_is_rejected() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        std::env::set_var("CLIENT_COUNT", "4000");
        std::env::set_var("FLEET_CLIENT_COUNT", "100");
        let parsed = Config::from_dhall(&config_path());
        std::env::remove_var("CLIENT_COUNT");
        std::env::remove_var("FLEET_CLIENT_COUNT");

        assert!(parsed.is_err());
    }

    #[test]
    fn the_service_config_loads_into_the_services_own_app_config() {
        use notification_service::environment::AppConfig;

        let path = format!(
            "{}/../../dhall-configs/dev/notification_service.dhall",
            env!("CARGO_MANIFEST_DIR")
        );
        let cfg = serde_dhall::from_file(&path)
            .parse::<AppConfig>()
            .expect("notification_service.dhall must load as AppConfig");

        assert_eq!(cfg.max_shards, 128);
        assert_eq!(
            cfg.max_shards,
            load().max_shards,
            "a producer writing to a different shard count writes to streams nobody reads"
        );
        validate_delivery_path(cfg.delivery_mode, load().publish_enabled)
            .expect("the checked-in pair must describe one delivery path");
    }

    #[test]
    fn a_matching_delivery_path_is_accepted() {
        assert!(validate_delivery_path(DeliveryMode::Pubsub, true).is_ok());
        assert!(validate_delivery_path(DeliveryMode::Sweep, false).is_ok());
    }

    #[test]
    fn a_pubsub_service_without_a_publishing_producer_is_rejected() {
        let err = validate_delivery_path(DeliveryMode::Pubsub, false)
            .expect_err("only the sweep would deliver, and the cell would read as a Pubsub result");
        assert!(err.to_string().contains("PUBLISH_ENABLED"), "{err}");
    }

    #[test]
    fn a_sweep_service_with_a_publishing_producer_is_rejected() {
        let err = validate_delivery_path(DeliveryMode::Sweep, true)
            .expect_err("Sweep would be charged for publishes nothing subscribes to");
        assert!(err.to_string().contains("DELIVERY_MODE"), "{err}");
    }

    #[test]
    fn delivery_mode_comes_from_the_environment() {
        use notification_service::{
            common::types::DeliveryMode, environment::delivery_mode_from_env,
        };

        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());

        std::env::set_var("DELIVERY_MODE", "Pubsub");
        let pubsub = delivery_mode_from_env(DeliveryMode::Sweep);
        std::env::set_var("DELIVERY_MODE", "sweep");
        let sweep = delivery_mode_from_env(DeliveryMode::Pubsub);
        std::env::set_var("DELIVERY_MODE", "   ");
        let blank = delivery_mode_from_env(DeliveryMode::Sweep);
        std::env::remove_var("DELIVERY_MODE");
        let unset = delivery_mode_from_env(DeliveryMode::Pubsub);

        assert_eq!(pubsub, DeliveryMode::Pubsub);
        assert_eq!(sweep, DeliveryMode::Sweep);
        assert_eq!(blank, DeliveryMode::Sweep);
        assert_eq!(unset, DeliveryMode::Pubsub);
    }

    #[test]
    fn an_unrecognised_delivery_mode_is_rejected_rather_than_silently_swept() {
        use notification_service::{
            common::types::DeliveryMode, environment::delivery_mode_from_env,
        };

        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        std::env::set_var("DELIVERY_MODE", "nonsense");
        let outcome = std::panic::catch_unwind(|| delivery_mode_from_env(DeliveryMode::Sweep));
        std::env::remove_var("DELIVERY_MODE");

        assert!(
            outcome.is_err(),
            "a typo must not run a Pubsub cell in Sweep mode"
        );
    }

    #[test]
    fn a_missing_config_is_an_error_rather_than_a_default() {
        assert!(Config::from_dhall("/no/such/sim.dhall").is_err());
    }
}
