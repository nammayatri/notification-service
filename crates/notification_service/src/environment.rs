/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
#![allow(clippy::expect_used)]

use crate::common::types::TokenOrigin;
use reqwest::Url;
use serde::Deserialize;
use shared::redis::types::{RedisConnectionPool, RedisSettings};
use shared::tools::logger::LoggerConfig;
use std::{collections::HashMap, sync::Arc, time::Duration};

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
    pub logger_cfg: LoggerConfig,
    pub redis_cfg: RedisSettings,
    pub retry_delay_millis: u64,
    pub max_shards: u64,
    pub channel_buffer: usize,
    pub request_timeout_seconds: u64,
    pub read_all_connected_client_notifications: bool,
}

#[derive(Clone)]
pub struct AppState {
    pub redis_pool: Arc<RedisConnectionPool>,
    pub internal_auth_cfg: HashMap<TokenOrigin, InternalAuthConfig>,
    pub retry_delay_millis: u64,
    pub grpc_port: u16,
    pub http_server_port: u16,
    pub max_shards: u64,
    pub channel_buffer: usize,
    pub request_timeout_seconds: Duration,
}

fn deserialize_url<'de, D>(deserializer: D) -> Result<Url, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    Url::parse(&s).map_err(serde::de::Error::custom)
}

impl AppState {
    pub async fn new(app_config: AppConfig) -> AppState {
        let redis_pool = Arc::new(
            RedisConnectionPool::new(app_config.redis_cfg, None)
                .await
                .expect("Failed to create Redis connection pool"),
        );

        AppState {
            redis_pool,
            internal_auth_cfg: app_config.internal_auth_cfg,
            retry_delay_millis: app_config.retry_delay_millis,
            grpc_port: app_config.grpc_port,
            http_server_port: app_config.http_server_port,
            max_shards: app_config.max_shards,
            channel_buffer: app_config.channel_buffer,
            request_timeout_seconds: Duration::from_secs(app_config.request_timeout_seconds),
        }
    }
}
