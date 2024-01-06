/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use actix_web::web;
use actix_web::App;
use actix_web::HttpResponse;
use actix_web::HttpServer;
use anyhow::Result;
use chrono::DateTime;
use chrono::Utc;
use futures::Stream;
use notification_service::common::types::*;
use notification_service::common::utils::abs_diff_utc_as_sec;
use notification_service::environment::AppConfig;
use notification_service::environment::AppState;
use notification_service::kafka::producers::kafka_stream_notification_updates;
use notification_service::kafka::types::NotificationStatus;
use notification_service::notification_latency;
use notification_service::notification_server::{Notification, NotificationServer};
use notification_service::outbound::external::internal_authentication;
use notification_service::reader::run_notification_reader;
use notification_service::redis::commands::clean_up_notification;
use notification_service::redis::commands::get_client_id;
use notification_service::redis::commands::get_notification_start_time;
use notification_service::redis::commands::set_client_id;
use notification_service::tools::error::AppError;
use notification_service::tools::logger::setup_tracing;
use notification_service::tools::prometheus::prometheus_metrics;
use notification_service::tools::prometheus::CONNECTED_CLIENTS;
use notification_service::tools::prometheus::NOTIFICATION_LATENCY;
use notification_service::{NotificationAck, NotificationPayload};
use reqwest::Url;
use shared::redis::types::RedisConnectionPool;
use std::env::var;
use std::net::Ipv4Addr;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::signal::unix::signal;
use tokio::signal::unix::SignalKind;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataMap;
use tonic::{transport::Server, Request, Response, Status};
use tracing::*;

pub struct NotificationService {
    read_notification_tx: Sender<(ClientId, Sender<Result<NotificationPayload, Status>>)>,
    app_state: AppState,
}

impl NotificationService {
    pub fn new(
        read_notification_tx: Sender<(ClientId, Sender<Result<NotificationPayload, Status>>)>,
        app_state: AppState,
    ) -> Self {
        NotificationService {
            read_notification_tx,
            app_state,
        }
    }
}

async fn _get_client_id_from_bpp_authentication(
    redis_pool: &RedisConnectionPool,
    token: &str,
    auth_url: &Url,
    auth_api_key: &str,
    auth_token_expiry: &u32,
) -> Result<ClientId> {
    match get_client_id(redis_pool, token).await? {
        Some(client_id) => Ok(client_id),
        None => {
            let response = internal_authentication(auth_url, token, auth_api_key).await?;
            set_client_id(redis_pool, auth_token_expiry, token, &response.client_id).await?;
            Ok(response.client_id)
        }
    }
}

#[tonic::async_trait]
impl Notification for NotificationService {
    type StreamPayloadStream =
        Pin<Box<dyn Stream<Item = Result<NotificationPayload, Status>> + Send + Sync>>;

    async fn stream_payload(
        &self,
        request: Request<tonic::Streaming<NotificationAck>>,
    ) -> Result<Response<Self::StreamPayloadStream>, Status> {
        let metadata: &MetadataMap = request.metadata();
        let token = metadata
            .get("token")
            .and_then(|token| token.to_str().ok())
            .map(|token| token.to_string())
            .ok_or(AppError::InvalidRequest(
                "token (token - Header) not found".to_string(),
            ))?;

        let client_id = metadata
            .get("client-id")
            .and_then(|client_id| client_id.to_str().ok())
            .map(|client_id| client_id.to_string())
            .ok_or(AppError::InvalidRequest(
                "client_id (client_id - Header) not found".to_string(),
            ))?;

        // let ClientId(client_id) = get_client_id_from_bpp_authentication(
        //     &self.app_state.redis_pool,
        //     &token,
        //     &self.app_state.auth_url,
        //     &self.app_state.auth_api_key,
        //     &self.app_state.auth_token_expiry,
        // )
        // .await
        // .map_err(|err| {
        //     AppError::InternalError(format!("Internal Authentication Failed : {:?}", err))
        // })?;

        CONNECTED_CLIENTS.inc();
        info!("Connection Successful - ClientId : {client_id} - token : {token}");

        let (client_tx, client_rx) = mpsc::channel(100000);

        if let Err(err) = self
            .read_notification_tx
            .clone()
            .send((ClientId(client_id.to_owned()), client_tx))
            .await
        {
            Err(AppError::InternalError(format!(
                "Failed to send data to Notification Reader: {:?}",
                err
            )))?
        }

        let (redis_pool, producer, topic) = (
            self.app_state.redis_pool.clone(),
            self.app_state.producer.clone(),
            self.app_state.notification_kafka_topic.clone(),
        );
        tokio::spawn(async move {
            let mut stream = request.into_inner();

            // Acknowledgment for sent notification from the client
            loop {
                match stream.message().await {
                    Ok(Some(notification_ack)) => {
                        match get_notification_start_time(&redis_pool, &notification_ack.id).await {
                            Ok(Some(Timestamp(start_time))) => {
                                notification_latency!(start_time);

                                let (
                                    producer_cloned,
                                    topic_cloned,
                                    client_id_cloned,
                                    notification_id_cloned,
                                ) = (
                                    producer.clone(),
                                    topic.clone(),
                                    client_id.clone(),
                                    notification_ack.id.clone(),
                                );
                                if let Ok(created_at) =
                                    notification_ack.created_at.parse::<DateTime<Utc>>()
                                {
                                    tokio::spawn(async move {
                                        let _ = kafka_stream_notification_updates(
                                            &producer_cloned,
                                            &topic_cloned,
                                            &client_id_cloned,
                                            notification_id_cloned,
                                            0,
                                            NotificationStatus::DELIVERED,
                                            Timestamp(created_at),
                                            Timestamp(start_time),
                                            Some(Timestamp(Utc::now())),
                                        )
                                        .await;
                                    });
                                }
                            }
                            Ok(None) => error!("Notification Start Time Not Found"),
                            Err(err) => {
                                error!("Error in getting Notification Start Time : {:?}", err)
                            }
                        }
                        let _ =
                            clean_up_notification(&redis_pool, &client_id, &notification_ack.id)
                                .await;
                    }
                    Ok(None) => {
                        error!("Client ({}) Disconnected", client_id);
                        break;
                    }
                    _ => continue,
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(client_rx))))
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 1)]
async fn main() -> Result<()> {
    let dhall_config_path = var("DHALL_CONFIG")
        .unwrap_or_else(|_| "./dhall-configs/dev/notification_service.dhall".to_string());
    let app_config = serde_dhall::from_file(dhall_config_path).parse::<AppConfig>()?;

    let _guard = setup_tracing(app_config.logger_cfg);

    std::panic::set_hook(Box::new(|panic_info| {
        error!("Panic Occured : {:?}", panic_info);
    }));

    let app_state = AppState::new(app_config).await;

    #[allow(clippy::type_complexity)]
    let (read_notification_tx, read_notification_rx): (
        Sender<(ClientId, Sender<Result<NotificationPayload, Status>>)>,
        Receiver<(ClientId, Sender<Result<NotificationPayload, Status>>)>,
    ) = mpsc::channel(10000);

    let graceful_termination_requested = Arc::new(AtomicBool::new(false));
    let graceful_termination_requested_sigterm = graceful_termination_requested.to_owned();
    let graceful_termination_requested_sigint = graceful_termination_requested.to_owned();
    // Listen for SIGTERM signal.
    tokio::spawn(async move {
        let mut sigterm = signal(SignalKind::terminate()).unwrap();
        sigterm.recv().await;
        graceful_termination_requested_sigterm.store(true, Ordering::Relaxed);
    });
    // Listen for SIGINT (Ctrl+C) signal.
    tokio::spawn(async move {
        let mut ctrl_c = signal(SignalKind::interrupt()).unwrap();
        ctrl_c.recv().await;
        graceful_termination_requested_sigint.store(true, Ordering::Relaxed);
    });

    let read_notification_thread = run_notification_reader(
        read_notification_rx,
        graceful_termination_requested,
        app_state.redis_pool.clone(),
        app_state.reader_delay_seconds,
        app_state.retry_delay_seconds,
        app_state.last_known_notification_cache_expiry,
        app_state.producer.clone(),
        app_state.notification_kafka_topic.clone(),
    );

    let prometheus = prometheus_metrics();
    let http_server = HttpServer::new(move || {
        App::new().wrap(prometheus.clone()).route(
            "/health",
            web::get()
                .to(|| Box::pin(async { HttpResponse::Ok().body("Notification Service Is Up!") })),
        )
    })
    .bind((Ipv4Addr::UNSPECIFIED, app_state.prometheus_port))?
    .run();

    let addr = format!("[::1]:{}", app_state.port).parse()?;
    let notification_service = NotificationService::new(read_notification_tx, app_state);
    let grpc_server = Server::builder()
        .add_service(NotificationServer::new(notification_service))
        .serve(addr);

    let (http_result, grpc_result, _read_notification_result) =
        tokio::join!(http_server, grpc_server, read_notification_thread);
    http_result?;
    grpc_result?;

    Ok(())
}
