/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
#![allow(clippy::expect_used)]

use crate::redis::types::{RedisConnectionPool, RedisSettings};
use crate::utils::logger::LoggerConfig;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub port: u16,
    pub logger_cfg: LoggerConfig,
    pub redis_cfg: RedisConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RedisConfig {
    pub redis_host: String,
    pub redis_port: u16,
    pub redis_pool_size: usize,
    pub redis_partition: usize,
    pub reconnect_max_attempts: u32,
    pub reconnect_delay: u32,
    pub default_ttl: u32,
    pub default_hash_ttl: u32,
    pub stream_read_count: u64,
}

#[derive(Clone)]
pub struct AppState {
    pub redis_cfg: Arc<RedisConnectionPool>,
}

impl AppState {
    pub async fn new(app_config: AppConfig) -> AppState {
        let redis_cfg = Arc::new(
            RedisConnectionPool::new(RedisSettings::new(
                app_config.redis_cfg.redis_host,
                app_config.redis_cfg.redis_port,
                app_config.redis_cfg.redis_pool_size,
                app_config.redis_cfg.redis_partition,
                app_config.redis_cfg.reconnect_max_attempts,
                app_config.redis_cfg.reconnect_delay,
                app_config.redis_cfg.default_ttl,
                app_config.redis_cfg.default_hash_ttl,
                app_config.redis_cfg.stream_read_count,
            ))
            .await
            .expect("Failed to create Redis connection pool"),
        );

        AppState { redis_cfg }
    }
}
