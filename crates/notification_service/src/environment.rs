/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
#![allow(clippy::expect_used)]

use crate::common::types::{DeliveryGuarantee, DeliveryMode, TokenOrigin};
use reqwest::Url;
use serde::Deserialize;
use shared::redis::types::{RedisConnectionPool, RedisSettings};
use shared::tools::logger::LoggerConfig;
use std::{collections::HashMap, num::NonZeroU32, sync::Arc, time::Duration};

#[derive(Debug, Deserialize, Clone)]
pub struct InternalAuthConfig {
    #[serde(deserialize_with = "deserialize_url")]
    pub auth_url: Url,
    pub auth_api_key: String,
    pub auth_token_expiry: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub grpc_port: u16,
    pub http_server_port: u16,
    pub internal_auth_cfg: HashMap<TokenOrigin, InternalAuthConfig>,
    #[serde(deserialize_with = "deserialize_url")]
    pub driver_api_base_url: Url,
    pub logger_cfg: LoggerConfig,
    pub redis_cfg: RedisSettings,
    pub retry_delay_millis: u64,
    pub sweep_delay_millis: u64,
    pub expired_cleanup_delay_millis: u64,
    pub max_shards: u64,
    pub channel_buffer: usize,
    pub request_timeout_seconds: u64,
    pub delivery_mode: DeliveryMode,
    pub stale_disconnect_guard: bool,
    pub delivery_guarantee: DeliveryGuarantee,
    pub max_delivery_attempts: Option<NonZeroU32>,
}

#[derive(Clone)]
pub struct AppState {
    pub redis_pool: Arc<RedisConnectionPool>,
    pub internal_auth_cfg: HashMap<TokenOrigin, InternalAuthConfig>,
    pub driver_api_base_url: Url,
    pub retry_delay_millis: u64,
    pub sweep_delay_millis: u64,
    pub expired_cleanup_delay_millis: u64,
    pub delivery_mode: DeliveryMode,
    pub delivery_guarantee: DeliveryGuarantee,
    pub max_delivery_attempts: Option<NonZeroU32>,
    pub grpc_port: u16,
    pub http_server_port: u16,
    pub max_shards: u64,
    pub channel_buffer: usize,
    pub request_timeout_seconds: Duration,
    pub stale_disconnect_guard: bool,
}

fn deserialize_url<'de, D>(deserializer: D) -> Result<Url, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    Url::parse(&s).map_err(serde::de::Error::custom)
}

pub fn redis_settings_from_env(mut redis_cfg: RedisSettings) -> RedisSettings {
    if let Some(host) = env_raw("REDIS_HOST") {
        redis_cfg.host = host;
    }
    if let Some(port) = env_override_opt("REDIS_PORT") {
        redis_cfg.port = port;
    }
    if let Some(cluster_enabled) = env_override_opt("REDIS_CLUSTER_ENABLED") {
        redis_cfg.cluster_enabled = cluster_enabled;
    }
    if let Some(pool_size) = env_override_opt("REDIS_POOL_SIZE") {
        redis_cfg.pool_size = pool_size;
    }
    redis_cfg
}

fn env_raw(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

fn env_override_opt<T: std::str::FromStr>(key: &str) -> Option<T> {
    env_raw(key).map(|raw| {
        raw.parse::<T>().unwrap_or_else(|_| {
            panic!(
                "[Notification Service Error] - {} is set to {:?}, which is not a valid {}. Refusing to fall back to the config value: the cell would run at an interval its label denies.",
                key,
                raw,
                std::any::type_name::<T>()
            )
        })
    })
}

fn env_override<T: std::str::FromStr>(key: &str, fallback: T) -> T {
    env_override_opt(key).unwrap_or(fallback)
}

pub fn delivery_mode_from_env(fallback: DeliveryMode) -> DeliveryMode {
    match env_raw("DELIVERY_MODE")
        .map(|raw| raw.to_ascii_lowercase())
        .as_deref()
    {
        None => fallback,
        Some("sweep") => DeliveryMode::Sweep,
        Some("pubsub") => DeliveryMode::Pubsub,
        Some(other) => panic!(
            "[Notification Service Error] - DELIVERY_MODE is set to {:?}, which is neither Sweep nor Pubsub",
            other
        ),
    }
}

impl AppState {
    pub async fn new(app_config: AppConfig) -> AppState {
        let redis_pool = Arc::new(
            RedisConnectionPool::new(redis_settings_from_env(app_config.redis_cfg), None)
                .await
                .expect("Failed to create Redis connection pool"),
        );

        // Override gRPC port from SERVICE_PORT env var if set, otherwise use dhall config.
        let grpc_port = env_override("SERVICE_PORT", app_config.grpc_port);

        let delivery_mode = delivery_mode_from_env(app_config.delivery_mode);
        let sweep_delay_millis = env_override("SWEEP_DELAY_MILLIS", app_config.sweep_delay_millis);
        let retry_delay_millis = env_override("RETRY_DELAY_MILLIS", app_config.retry_delay_millis);
        let request_timeout_seconds = env_override(
            "REQUEST_TIMEOUT_SECONDS",
            app_config.request_timeout_seconds,
        );

        AppState {
            redis_pool,
            internal_auth_cfg: app_config.internal_auth_cfg,
            driver_api_base_url: app_config.driver_api_base_url,
            retry_delay_millis,
            sweep_delay_millis,
            expired_cleanup_delay_millis: app_config.expired_cleanup_delay_millis,
            delivery_mode,
            delivery_guarantee: app_config.delivery_guarantee,
            max_delivery_attempts: app_config.max_delivery_attempts,
            grpc_port,
            http_server_port: app_config.http_server_port,
            max_shards: app_config.max_shards,
            channel_buffer: app_config.channel_buffer,
            request_timeout_seconds: Duration::from_secs(request_timeout_seconds),
            stale_disconnect_guard: app_config.stale_disconnect_guard,
        }
    }
}
