/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use crate::distribution::{AckDelayCurve, LifetimeCurve};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use sim_common::config::{client_index_start, env_override, env_override_list};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
pub enum AckMode {
    Instant,
    Delayed,
    Distribution,
    Never,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
pub enum ReconnectMode {
    ServerOnly,
    Distribution,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct LifetimeBucket {
    upto_seconds: f64,
    cumulative_count: f64,
}

#[derive(Debug, Deserialize)]
struct ClientsSection {
    endpoints: Vec<String>,
    client_index_start: u64,
    client_count: u64,
    ack_mode: AckMode,
    ack_delay_ms: u64,
    ack_p50_ms: u64,
    ack_p90_ms: u64,
    ack_p95_ms: u64,
    ack_max_ms: u64,
    reconnect_mode: ReconnectMode,
    reconnect_lifetime_buckets: Vec<LifetimeBucket>,
    never_ack_pct: u64,
    metrics_port: u16,
    max_shards: u64,
    streams_per_connection: u64,
    connect_rate_per_sec: f64,
    token_origin: String,
}

#[derive(Debug, Deserialize)]
struct SimFile {
    clients: ClientsSection,
}

pub struct Config {
    pub endpoints: Vec<String>,
    pub client_index_start: u64,
    pub client_count: u64,
    pub ack_mode: AckMode,
    pub ack_delay_ms: u64,
    pub ack_delays: AckDelayCurve,
    pub reconnect_mode: ReconnectMode,
    pub reconnect_lifetimes: LifetimeCurve,
    pub never_ack_pct: u64,
    pub metrics_port: u16,
    pub max_shards: u64,
    pub streams_per_connection: u64,
    pub connect_rate_per_sec: f64,
    pub token_origin: String,
}

impl Config {
    pub fn from_dhall(path: &str) -> Result<Self> {
        let file = serde_dhall::from_file(path).parse::<SimFile>()?;
        Self::from_section(file.clients, path)
    }

    fn from_section(section: ClientsSection, source: &str) -> Result<Self> {
        let endpoints: Vec<String> = env_override_list("SIM_ENDPOINTS", section.endpoints)
            .into_iter()
            .map(|endpoint| endpoint.trim().to_string())
            .filter(|endpoint| !endpoint.is_empty())
            .collect();
        if endpoints.is_empty() {
            return Err(anyhow!(
                "{source}: clients.endpoints resolved to an empty list"
            ));
        }

        let client_count = env_override("CLIENT_COUNT", section.client_count)?;
        if client_count == 0 {
            return Err(anyhow!("{source}: clients.client_count must be > 0"));
        }

        if section.never_ack_pct > 100 {
            return Err(anyhow!("{source}: clients.never_ack_pct must be 0..=100"));
        }

        let ack_delays = AckDelayCurve::new(
            section.ack_p50_ms,
            section.ack_p90_ms,
            section.ack_p95_ms,
            section.ack_max_ms,
        )
        .map_err(|err| anyhow!("{source}: clients {err}"))?;

        let buckets: Vec<(f64, f64)> = section
            .reconnect_lifetime_buckets
            .iter()
            .map(|bucket| (bucket.upto_seconds, bucket.cumulative_count))
            .collect();
        let reconnect_lifetimes = LifetimeCurve::from_buckets(&buckets)
            .map_err(|err| anyhow!("{source}: clients.{err}"))?;

        Ok(Self {
            endpoints,
            client_index_start: client_index_start(section.client_index_start)?,
            client_count,
            ack_mode: section.ack_mode,
            ack_delay_ms: section.ack_delay_ms,
            ack_delays,
            reconnect_mode: section.reconnect_mode,
            reconnect_lifetimes,
            never_ack_pct: section.never_ack_pct,
            metrics_port: section.metrics_port,
            max_shards: section.max_shards,
            streams_per_connection: section.streams_per_connection.max(1),
            connect_rate_per_sec: section.connect_rate_per_sec,
            token_origin: section.token_origin,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        assert!(!cfg.endpoints.is_empty());
        assert!(cfg.never_ack_pct <= 100);
        assert!(cfg.reconnect_lifetimes.bucket_count() > 0);
        assert_eq!(cfg.metrics_port, 9101);
    }

    #[test]
    fn the_config_carries_the_prod_calibration() {
        let cfg = load();
        assert_eq!(cfg.ack_mode, AckMode::Distribution);
        assert_eq!(cfg.reconnect_mode, ReconnectMode::Distribution);
        assert_eq!(cfg.ack_delays.p50_ms, 800);
        assert_eq!(cfg.ack_delays.p90_ms, 2071);
        assert_eq!(cfg.ack_delays.p95_ms, 2326);
        assert_eq!(cfg.never_ack_pct, 1);
        assert_eq!(cfg.max_shards, 128);
        assert_eq!(
            cfg.reconnect_lifetimes.bucket_count(),
            16,
            "the default profile must stay the 16-bucket prod table"
        );
    }

    #[test]
    fn both_reconnect_profiles_stay_available() {
        let text = std::fs::read_to_string(config_path()).expect("sim.dhall must be readable");
        assert!(text.contains("prod_stream_lifetime_buckets"));
        assert!(text.contains("healthy_stream_lifetime_buckets"));
    }

    #[test]
    fn env_overrides_replace_the_file_values() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        std::env::set_var("CLIENT_COUNT", "19301");
        std::env::set_var("CLIENT_INDEX_START", "4000");
        std::env::set_var(
            "SIM_ENDPOINTS",
            " http://notification-service:50051 , http://other:50051 ",
        );

        let parsed = Config::from_dhall(&config_path());

        std::env::remove_var("CLIENT_COUNT");
        std::env::remove_var("CLIENT_INDEX_START");
        std::env::remove_var("SIM_ENDPOINTS");

        let cfg = parsed.expect("overrides must parse");
        assert_eq!(cfg.client_count, 19_301);
        assert_eq!(cfg.client_index_start, 4_000);
        assert_eq!(
            cfg.endpoints,
            vec![
                "http://notification-service:50051".to_string(),
                "http://other:50051".to_string()
            ]
        );
    }

    #[test]
    fn an_unparseable_override_is_an_error_rather_than_a_silent_fallback() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        std::env::set_var("CLIENT_COUNT", "four thousand");
        let parsed = Config::from_dhall(&config_path());
        std::env::remove_var("CLIENT_COUNT");

        let err = match parsed {
            Ok(_) => panic!("a typo must not run the cell at the file's value"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("CLIENT_COUNT"), "{err}");
    }

    #[test]
    fn a_missing_config_is_an_error_rather_than_a_default() {
        assert!(Config::from_dhall("/no/such/sim.dhall").is_err());
    }
}
