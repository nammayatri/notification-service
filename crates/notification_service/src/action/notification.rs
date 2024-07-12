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
    outbound::external::internal_authentication,
    redis::commands::{get_client_id, get_notification_stream_id, set_client_id},
    tools::{
        error::AppError,
        prometheus::{
            DELIVERED_NOTIFICATIONS, MEASURE_DURATION, NOTIFICATION_CLIENT_CONNECTION_DURATION,
            NOTIFICATION_LATENCY,
        },
    },
    NotificationAck, NotificationPayload,
};
use anyhow::Result;
use chrono::Utc;
use futures::Stream;
use reqwest::Url;
use shared::measure_latency_duration;
use shared::redis::types::RedisConnectionPool;
use std::{env::var, pin::Pin};
use tokio::{
    sync::mpsc::{self, Sender, UnboundedSender},
    time::{timeout, Instant},
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{metadata::MetadataMap, Request, Response, Status};
use tracing::*;

#[allow(clippy::type_complexity)]
pub struct NotificationService {
    read_notification_tx: UnboundedSender<(
        ClientId,
        Option<Sender<Result<NotificationPayload, Status>>>,
    )>,
    app_state: AppState,
}

impl NotificationService {
    #[allow(clippy::type_complexity)]
    pub fn new(
        read_notification_tx: UnboundedSender<(
            ClientId,
            Option<Sender<Result<NotificationPayload, Status>>>,
        )>,
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

        let ClientId(client_id) = if var("DEV").is_ok() {
            ClientId(token.to_owned())
        } else {
            get_client_id_from_bpp_authentication(
                &self.app_state.redis_pool,
                &token,
                &self.app_state.auth_url,
                &self.app_state.auth_api_key,
                &self.app_state.auth_token_expiry,
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
        if let Err(err) = read_notification_tx
            .clone()
            .send((ClientId(client_id.to_owned()), Some(client_tx)))
        {
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

                            DELIVERED_NOTIFICATIONS.inc();
                        }
                        Ok(None) => {
                            info!("Client ({}) Disconnected", client_id);
                            notification_client_connection_duration!("DISCONNECTED", start_time);
                            if let Err(err) = read_notification_tx
                                .clone()
                                .send((ClientId(client_id.to_owned()), None))
                            {
                                error!(
                                    "Failed to remove client's ({:?}) instance from Reader : {:?}",
                                    client_id, err
                                );
                            }
                            break;
                        }
                        Err(err) => {
                            info!("Client ({}) Disconnected : {}", client_id, err);
                            notification_client_connection_duration!("DISCONNECTED", start_time);
                            if let Err(err) = read_notification_tx
                                .clone()
                                .send((ClientId(client_id.to_owned()), None))
                            {
                                error!(
                                    "Failed to remove client's ({:?}) instance from Reader : {:?}",
                                    client_id, err
                                );
                            }
                            break;
                        }
                    }
                }
            })
            .await
            {
                info!("Client ({}) Timed Out : {}", client_id_clone, err);
                notification_client_connection_duration!("TIMED_OUT", start_time);
                if let Err(err) =
                    read_notification_tx_clone.send((ClientId(client_id_clone.to_owned()), None))
                {
                    error!(
                        "Failed to remove client's ({:?}) instance from Reader : {:?}",
                        client_id_clone, err
                    );
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(client_rx))))
    }
}
