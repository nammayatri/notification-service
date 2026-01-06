/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use crate::{
    common::{
        types::*,
        utils::{abs_diff_utc_as_sec, get_timestamp_from_stream_id},
    },
    environment::AppState,
    notification_client_connection_duration, notification_latency,
    notification_server::Notification,
    outbound::{
        external::{driver_quote_respond, internal_authentication},
        types::DriverRespondReq,
    },
    redis::commands::{get_client_id, get_notification_stream_id, set_client_id},
    tools::{
        callapi::CallApiError,
        error::AppError,
        prometheus::{
            DELIVERED_NOTIFICATIONS, MEASURE_DURATION, NOTIFICATION_CLIENT_CONNECTION_DURATION,
            NOTIFICATION_LATENCY,
        },
    },
    NotificationAck, NotificationPayload, QuoteRequest, QuoteResponse,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::Stream;
use reqwest::Url;
use shared::measure_latency_duration;
use shared::redis::types::RedisConnectionPool;
use std::{env::var, pin::Pin, str::FromStr};
use tokio::{
    sync::mpsc::{self, UnboundedSender},
    time::{sleep, timeout, Instant},
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{metadata::MetadataMap, Request, Response, Status};
use tracing::*;

#[allow(clippy::type_complexity)]
pub struct NotificationService {
    read_notification_tx: UnboundedSender<(ClientId, SenderType, DateTime<Utc>)>,
    app_state: AppState,
}

impl NotificationService {
    #[allow(clippy::type_complexity)]
    pub fn new(
        read_notification_tx: UnboundedSender<(ClientId, SenderType, DateTime<Utc>)>,
        app_state: AppState,
    ) -> Self {
        NotificationService {
            read_notification_tx,
            app_state,
        }
    }
}

#[macros::measure_duration]
async fn get_client_id_from_bpp_authentication(
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

    type ServerStreamPayloadStream =
        Pin<Box<dyn Stream<Item = Result<NotificationPayload, Status>> + Send + Sync>>;

    type QuoteRespondStream =
        Pin<Box<dyn Stream<Item = Result<QuoteResponse, Status>> + Send + Sync>>;

    #[allow(unused_variables)]
    async fn server_stream_payload(
        &self,
        request: Request<NotificationAck>,
    ) -> Result<Response<Self::ServerStreamPayloadStream>, Status> {
        let start_time = Instant::now();

        let metadata: &MetadataMap = request.metadata();
        let token = metadata
            .get("token")
            .and_then(|token| token.to_str().ok())
            .map(|token| token.to_string())
            .ok_or(AppError::InvalidRequest(
                "token (token - Header) not found".to_string(),
            ))?;

        let token_origin = metadata
            .get("token-origin")
            .and_then(|origin| origin.to_str().ok())
            .and_then(|origin| TokenOrigin::from_str(origin).ok())
            .unwrap_or(TokenOrigin::DriverApp);

        let session_id = metadata
            .get("session-id")
            .and_then(|origin| origin.to_str().ok())
            .map(|origin| SessionID(origin.to_string()));

        let ClientId(client_id) = if var("DEV").is_ok() {
            ClientId(token.to_owned())
        } else {
            let internal_auth_cfg = self.app_state.internal_auth_cfg.get(&token_origin).ok_or(
                AppError::InternalError(format!(
                    "InternalAuthConfig Not Found for TokenOrigin: {}",
                    token_origin
                )),
            )?;
            get_client_id_from_bpp_authentication(
                &self.app_state.redis_pool,
                &token,
                &internal_auth_cfg.auth_url,
                &internal_auth_cfg.auth_api_key,
                &internal_auth_cfg.auth_token_expiry,
            )
            .await
            .map_err(|err| {
                AppError::InternalError(format!("Internal Authentication Failed : {:?}", err))
            })?
        };

        info!("Connection Successful - ClientId : {client_id} - token : {token}");

        let (client_tx, client_rx) = mpsc::channel(self.app_state.channel_buffer);

        let (redis_pool, read_notification_tx, max_shards, request_timeout_seconds) = (
            self.app_state.redis_pool.clone(),
            self.read_notification_tx.clone(),
            self.app_state.max_shards,
            self.app_state.request_timeout_seconds,
        );

        let add_client_tx_start_time = Instant::now();
        if let Err(err) = read_notification_tx.clone().send((
            ClientId(client_id.to_owned()),
            SenderType::ClientConnection((session_id.to_owned(), client_tx)),
            Utc::now(),
        )) {
            error!(
                "Failed to Send Data to Notification Reader for Client : {}, Error : {:?}",
                client_id, err
            );
        }
        measure_latency_duration!("add_client_tx", add_client_tx_start_time);

        tokio::spawn(async move {
            sleep(request_timeout_seconds).await;

            let (read_notification_tx_clone, client_id_clone) =
                (read_notification_tx.clone(), client_id.clone());

            info!("Client ({}) Timed Out", client_id_clone);
            notification_client_connection_duration!("TIMED_OUT", start_time);
            if let Err(err) = read_notification_tx_clone.send((
                ClientId(client_id_clone.to_owned()),
                SenderType::ClientDisconnection(session_id),
                Utc::now(),
            )) {
                error!(
                    "Failed to remove client's ({:?}) instance from Reader : {:?}",
                    client_id_clone, err
                );
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(client_rx))))
    }

    #[allow(unused_variables)]
    async fn stream_payload(
        &self,
        request: Request<tonic::Streaming<NotificationAck>>,
    ) -> Result<Response<Self::StreamPayloadStream>, Status> {
        let start_time = Instant::now();

        let metadata: &MetadataMap = request.metadata();
        let token = metadata
            .get("token")
            .and_then(|token| token.to_str().ok())
            .map(|token| token.to_string())
            .ok_or(AppError::InvalidRequest(
                "token (token - Header) not found".to_string(),
            ))?;

        let token_origin = metadata
            .get("token-origin")
            .and_then(|origin| origin.to_str().ok())
            .and_then(|origin| TokenOrigin::from_str(origin).ok())
            .unwrap_or(TokenOrigin::DriverApp);

        let ClientId(client_id) = if var("DEV").is_ok() {
            ClientId(token.to_owned())
        } else {
            let internal_auth_cfg = self.app_state.internal_auth_cfg.get(&token_origin).ok_or(
                AppError::InternalError(format!(
                    "InternalAuthConfig Not Found for TokenOrigin: {}",
                    token_origin
                )),
            )?;
            get_client_id_from_bpp_authentication(
                &self.app_state.redis_pool,
                &token,
                &internal_auth_cfg.auth_url,
                &internal_auth_cfg.auth_api_key,
                &internal_auth_cfg.auth_token_expiry,
            )
            .await
            .map_err(|err| {
                AppError::InternalError(format!("Internal Authentication Failed : {:?}", err))
            })?
        };

        info!("Connection Successful - ClientId : {client_id} - token : {token}");

        let (client_tx, client_rx) = mpsc::channel(self.app_state.channel_buffer);

        let (redis_pool, read_notification_tx, max_shards, request_timeout_seconds) = (
            self.app_state.redis_pool.clone(),
            self.read_notification_tx.clone(),
            self.app_state.max_shards,
            self.app_state.request_timeout_seconds,
        );

        let add_client_tx_start_time = Instant::now();
        if let Err(err) = read_notification_tx.clone().send((
            ClientId(client_id.to_owned()),
            SenderType::ClientConnection((None, client_tx)),
            Utc::now(),
        )) {
            error!(
                "Failed to Send Data to Notification Reader for Client : {}, Error : {:?}",
                client_id, err
            );
        }
        measure_latency_duration!("add_client_tx", add_client_tx_start_time);

        tokio::spawn(async move {
            let (read_notification_tx_clone, client_id_clone) =
                (read_notification_tx.clone(), client_id.clone());

            if let Err(err) = timeout(request_timeout_seconds, async move {
                let mut stream = request.into_inner();

                loop {
                    match stream.message().await {
                        Ok(Some(notification_ack)) => {
                            // This is during the initial connection
                            if notification_ack.id.is_empty() {
                                continue;
                            }
                            // Acknowledgment for sent notification from the client
                            match get_notification_stream_id(&redis_pool, &notification_ack.id)
                                .await
                            {
                                Ok(Some(StreamEntry(notification_stream_id))) => {
                                    notification_latency!(
                                        get_timestamp_from_stream_id(&notification_stream_id)
                                            .inner(),
                                        "ACK"
                                    );
                                }
                                Ok(None) => {
                                    error!("Notification Stream Id Not Found.");
                                }
                                Err(err) => {
                                    error!("Error in getting Notification Stream Id : {:?}", err);
                                }
                            }
                            if let Err(err) = read_notification_tx_clone.send((
                                ClientId(client_id_clone.to_owned()),
                                SenderType::ClientAck((NotificationId(notification_ack.id), None)),
                                Utc::now(),
                            )) {
                                error!(
                                    "Failed to remove client's ({:?}) instance from Reader : {:?}",
                                    client_id_clone, err
                                );
                            }
                            DELIVERED_NOTIFICATIONS.inc();
                        }
                        Ok(None) => {
                            info!("Client ({}) Disconnected", client_id_clone);
                            notification_client_connection_duration!("DISCONNECTED", start_time);
                            if let Err(err) = read_notification_tx_clone.send((
                                ClientId(client_id_clone.to_owned()),
                                SenderType::ClientDisconnection(None),
                                Utc::now(),
                            )) {
                                error!(
                                    "Failed to remove client's ({:?}) instance from Reader : {:?}",
                                    client_id_clone, err
                                );
                            }
                            break;
                        }
                        Err(err) => {
                            info!("Client ({}) Disconnected : {}", client_id_clone, err);
                            notification_client_connection_duration!("DISCONNECTED", start_time);
                            if let Err(err) = read_notification_tx_clone.send((
                                ClientId(client_id_clone.to_owned()),
                                SenderType::ClientDisconnection(None),
                                Utc::now(),
                            )) {
                                error!(
                                    "Failed to remove client's ({:?}) instance from Reader : {:?}",
                                    client_id_clone, err
                                );
                            }
                            break;
                        }
                    }
                }
            })
            .await
            {
                let (read_notification_tx_clone, client_id_clone) =
                    (read_notification_tx.clone(), client_id.clone());

                info!("Client ({}) Timed Out : {}", client_id_clone, err);
                notification_client_connection_duration!("TIMED_OUT", start_time);
                if let Err(err) = read_notification_tx_clone.send((
                    ClientId(client_id_clone.to_owned()),
                    SenderType::ClientDisconnection(None),
                    Utc::now(),
                )) {
                    error!(
                        "Failed to remove client's ({:?}) instance from Reader : {:?}",
                        client_id_clone, err
                    );
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(client_rx))))
    }

    async fn quote_respond(
        &self,
        request: Request<tonic::Streaming<QuoteRequest>>,
    ) -> Result<Response<Self::QuoteRespondStream>, Status> {
        let metadata: &MetadataMap = request.metadata();

        // Extract and validate token from metadata
        let token = metadata
            .get("token")
            .and_then(|token| token.to_str().ok())
            .map(|token| token.to_string())
            .ok_or(AppError::InvalidRequest(
                "token (token - Header) not found".to_string(),
            ))?;

        let token_origin = metadata
            .get("token-origin")
            .and_then(|origin| origin.to_str().ok())
            .and_then(|origin| TokenOrigin::from_str(origin).ok())
            .unwrap_or(TokenOrigin::DriverApp);

        // Validate token and get client_id
        let ClientId(client_id) = if var("DEV").is_ok() {
            ClientId(token.to_owned())
        } else {
            let internal_auth_cfg = self.app_state.internal_auth_cfg.get(&token_origin).ok_or(
                AppError::InternalError(format!(
                    "InternalAuthConfig Not Found for TokenOrigin: {}",
                    token_origin
                )),
            )?;
            get_client_id_from_bpp_authentication(
                &self.app_state.redis_pool,
                &token,
                &internal_auth_cfg.auth_url,
                &internal_auth_cfg.auth_api_key,
                &internal_auth_cfg.auth_token_expiry,
            )
            .await
            .map_err(|err| {
                AppError::InternalError(format!("Internal Authentication Failed : {:?}", err))
            })?
        };

        info!("Connection Successful - ClientId : {client_id} - token : {token}");

        // Extract optional headers from metadata
        let x_package = metadata
            .get("x-package")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let x_bundle_version = metadata
            .get("x-bundle-version")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let x_client_version = metadata
            .get("x-client-version")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let x_config_version = metadata
            .get("x-config-version")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let x_react_bundle_version = metadata
            .get("x-react-bundle-version")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let x_device = metadata
            .get("x-device")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let request_timeout_seconds = self.app_state.request_timeout_seconds;
        let driver_api_base_url = self.app_state.driver_api_base_url.clone();
        let token_clone = token.clone();
        let client_id_for_timeout = client_id.clone();
        let x_package_clone = x_package.clone();
        let x_bundle_version_clone = x_bundle_version.clone();
        let x_client_version_clone = x_client_version.clone();
        let x_config_version_clone = x_config_version.clone();
        let x_react_bundle_version_clone = x_react_bundle_version.clone();
        let x_device_clone = x_device.clone();

        // Spawn a task to handle incoming requests
        tokio::spawn(async move {
            let mut stream = request.into_inner();
            let client_id_clone = client_id.clone();

            if let Err(err) = timeout(request_timeout_seconds, async move {
                loop {
                    match stream.message().await {
                        Ok(Some(request_data)) => {
                            // Convert proto QuoteRequest to DriverRespondReq
                            let driver_req = DriverRespondReq {
                                notification_source: request_data.notification_source,
                                rendered_at: request_data.rendered_at,
                                responded_at: request_data.responded_at,
                                search_request_id: request_data.search_request_id,
                                offered_fare: if request_data.offered_fare == 0.0
                                    || request_data.response == "Reject"
                                {
                                    None
                                } else {
                                    Some(request_data.offered_fare)
                                },
                                response: request_data.response,
                                slot_number: request_data.slot_number,
                            };

                            // Build headers vector
                            let mut api_headers = Vec::new();
                            if let Some(pkg) = &x_package_clone {
                                api_headers.push(("x-package", pkg.as_str()));
                            }
                            if let Some(bundle_ver) = &x_bundle_version_clone {
                                api_headers.push(("x-bundle-version", bundle_ver.as_str()));
                            }
                            if let Some(client_ver) = &x_client_version_clone {
                                api_headers.push(("x-client-version", client_ver.as_str()));
                            }
                            if let Some(config_ver) = &x_config_version_clone {
                                api_headers.push(("x-config-version", config_ver.as_str()));
                            }
                            if let Some(react_bundle_ver) = &x_react_bundle_version_clone {
                                api_headers
                                    .push(("x-react-bundle-version", react_bundle_ver.as_str()));
                            }
                            if let Some(device) = &x_device_clone {
                                api_headers.push(("x-device", device.as_str()));
                            }

                            // Call the internal API
                            match driver_quote_respond(
                                &driver_api_base_url,
                                &token_clone,
                                api_headers,
                                driver_req,
                            )
                            .await
                            {
                                Ok(api_success) => {
                                    info!(
                                        "Successfully processed quote respond for client {}",
                                        client_id_clone
                                    );
                                    let response = QuoteResponse {
                                        status: api_success.result,
                                        reason: "Success".to_string(),
                                    };

                                    if tx.send(Ok(response)).await.is_err() {
                                        error!(
                                            "Failed to send response to client: {}",
                                            client_id_clone
                                        );
                                        break;
                                    }
                                }
                                Err(CallApiError::ExternalAPICallError(error_resp)) => {
                                    error!(
                                        "Driver API call failed for client {}: Status {}",
                                        client_id_clone,
                                        error_resp.status()
                                    );
                                    let response = QuoteResponse {
                                        status: "error".to_string(),
                                        reason: error_resp.text().await.unwrap_or_default(),
                                    };

                                    if tx.send(Ok(response)).await.is_err() {
                                        error!(
                                            "Failed to send error response to client: {}",
                                            client_id_clone
                                        );
                                        break;
                                    }
                                }
                                Err(CallApiError::ConnectionError(err)) => {
                                    error!(
                                        "Connection error for client {}: {}",
                                        client_id_clone, err
                                    );
                                    let response = QuoteResponse {
                                        status: "error".to_string(),
                                        reason: format!("Connection error: {}", err),
                                    };

                                    if tx.send(Ok(response)).await.is_err() {
                                        error!(
                                            "Failed to send error response to client: {}",
                                            client_id_clone
                                        );
                                        break;
                                    }
                                }
                                Err(err) => {
                                    error!(
                                        "Error calling driver API for client {}: {:?}",
                                        client_id_clone, err
                                    );
                                    let response = QuoteResponse {
                                        status: "error".to_string(),
                                        reason: format!("Internal error: {:?}", err),
                                    };

                                    if tx.send(Ok(response)).await.is_err() {
                                        error!(
                                            "Failed to send error response to client: {}",
                                            client_id_clone
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            info!("Client ({}) Disconnected", client_id_clone);
                            break;
                        }
                        Err(err) => {
                            info!("Client ({}) Disconnected : {}", client_id_clone, err);
                            break;
                        }
                    }
                }
            })
            .await
            {
                info!("Client ({}) Timed Out : {}", client_id_for_timeout, err);
            }
        });

        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }
}
