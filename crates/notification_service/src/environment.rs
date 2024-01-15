/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
#![allow(clippy::expect_used)]

use rdkafka::{error::KafkaError, producer::FutureProducer, ClientConfig};
use reqwest::Url;
use serde::Deserialize;
use shared::redis::types::{RedisConnectionPool, RedisSettings};
use std::sync::Arc;
use tracing::info;

use crate::tools::logger::LoggerConfig;

#[derive(Debug, Deserialize, Clone)]
pub struct KafkaConfig {
    pub kafka_key: String,
    pub kafka_host: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct InternalAuthConfig {
    pub auth_url: String,
    pub auth_api_key: String,
    pub auth_token_expiry: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub grpc_port: u16,
    pub http_server_port: u16,
    pub internal_auth_cfg: InternalAuthConfig,
    pub logger_cfg: LoggerConfig,
    pub redis_cfg: RedisSettings,
    pub kafka_cfg: KafkaConfig,
    pub last_known_notification_cache_expiry: u32,
    pub notification_kafka_topic: String,
    pub reader_delay_seconds: u64,
    pub retry_delay_seconds: u64,
}

#[derive(Clone)]
pub struct AppState {
    pub redis_pool: Arc<RedisConnectionPool>,
    pub auth_url: Url,
    pub auth_api_key: String,
    pub auth_token_expiry: u32,
    pub reader_delay_seconds: u64,
    pub retry_delay_seconds: u64,
    pub last_known_notification_cache_expiry: u32,
    pub notification_kafka_topic: String,
    pub producer: Option<FutureProducer>,
    pub grpc_port: u16,
    pub http_server_port: u16,
}

impl AppState {
    pub async fn new(app_config: AppConfig) -> AppState {
        let redis_pool = Arc::new(
            RedisConnectionPool::new(app_config.redis_cfg, None)
                .await
                .expect("Failed to create Redis connection pool"),
        );

        let producer: Option<FutureProducer>;

        let result: Result<FutureProducer, KafkaError> = ClientConfig::new()
            .set(
                app_config.kafka_cfg.kafka_key,
                app_config.kafka_cfg.kafka_host,
            )
            .set("compression.type", "lz4")
            .create();

        match result {
            Ok(val) => {
                producer = Some(val);
            }
            Err(err) => {
                producer = None;
                info!(
                    tag = "[Kafka Connection]",
                    "Error connecting to kafka config: {err}"
                );
            }
        }

        AppState {
            redis_pool,
            auth_url: Url::parse(app_config.internal_auth_cfg.auth_url.as_str())
                .expect("Failed to parse auth_url."),
            auth_api_key: app_config.internal_auth_cfg.auth_api_key,
            auth_token_expiry: app_config.internal_auth_cfg.auth_token_expiry,
            last_known_notification_cache_expiry: app_config.last_known_notification_cache_expiry,
            reader_delay_seconds: app_config.reader_delay_seconds,
            retry_delay_seconds: app_config.retry_delay_seconds,
            notification_kafka_topic: app_config.notification_kafka_topic,
            producer,
            grpc_port: app_config.grpc_port,
            http_server_port: app_config.http_server_port,
        }
    }
}
