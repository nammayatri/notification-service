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
        utils::{hash_uuid, is_stream_id_less_or_eq, transform_notification_data_to_payload},
    },
    redis::commands::{
        clean_up_notification, get_client_last_sent_notification, read_client_notifications,
        set_clients_last_sent_notification, set_notification_stream_id,
    },
    tools::prometheus::{
        CONNECTED_CLIENTS, EXPIRED_NOTIFICATIONS, RETRIED_NOTIFICATIONS, TOTAL_NOTIFICATIONS,
    },
    NotificationPayload,
};
use anyhow::Result;
use chrono::Utc;
use itertools::Itertools;
use rustc_hash::FxHashMap;
use shared::redis::types::RedisConnectionPool;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    sync::mpsc::{Receiver, Sender},
    time::interval,
};
use tonic::Status;
use tracing::*;

#[allow(clippy::type_complexity)]
fn get_clients_last_seen_notification_id(
    clients_tx: &FxHashMap<ClientId, (Sender<Result<NotificationPayload, Status>>, StreamEntry)>,
) -> Vec<(ClientId, StreamEntry)> {
    clients_tx
        .iter()
        .map(|(client_id, (_, last_read_stream_entry))| {
            (client_id.clone(), last_read_stream_entry.clone())
        })
        .collect()
}

pub fn can_retry(id1: &str, id2: &str, retry_delay_seconds: u64) -> bool {
    // Split the stream IDs into timestamp and sequence parts
    let parts1: Vec<&str> = id1.split('-').collect();
    let parts2: Vec<&str> = id2.split('-').collect();

    // Parse timestamp and sequence as integers
    match (
        parts1.first().and_then(|&s| s.parse::<u64>().ok()),
        parts1.get(1).and_then(|&s| s.parse::<u64>().ok()),
        parts2.first().and_then(|&s| s.parse::<u64>().ok()),
        parts2.get(1).and_then(|&s| s.parse::<u64>().ok()),
    ) {
        (Some(ts1), Some(seq1), Some(ts2), Some(seq2)) => {
            let retry_delay_millis = retry_delay_seconds * 1000;
            match (ts1 + retry_delay_millis).cmp(&ts2) {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Equal => seq1 <= seq2,
                std::cmp::Ordering::Greater => false,
            }
        }
        _ => true,
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub async fn run_notification_reader(
    mut read_notification_rx: Receiver<(
        ClientId,
        Option<Sender<Result<NotificationPayload, Status>>>,
    )>,
    graceful_termination_requested: Arc<AtomicBool>,
    redis_pool: Arc<RedisConnectionPool>,
    reader_delay_seconds: u64,
    retry_delay_seconds: u64,
    last_known_notification_cache_expiry: u32,
    max_shards: u64,
    reader_batch: u64,
) {
    let mut clients_tx: FxHashMap<
        ClientId,
        (Sender<Result<NotificationPayload, Status>>, StreamEntry),
    > = FxHashMap::default();
    let mut reader_timer = interval(Duration::from_secs(reader_delay_seconds));
    let mut retry_timer = 0.0;

    loop {
        if graceful_termination_requested.load(Ordering::Relaxed) {
            error!("[Graceful Shutting Down] => Storing following clients last read notification in redis : {:?}", clients_tx);

            let _ = set_clients_last_sent_notification(
                &redis_pool,
                get_clients_last_seen_notification_id(&clients_tx),
                last_known_notification_cache_expiry,
            )
            .await
            .map_err(|err| error!("Error in set_clients_last_sent_notification : {}", err));

            break;
        }
        tokio::select! {
            item = read_notification_rx.recv() => {
                match item {
                    Some((client_id, client_tx)) => {
                        match client_tx {
                            Some(client_tx) => {
                                info!("[Client Connected] : {:?}", client_id);
                                CONNECTED_CLIENTS.inc();
                                let last_read_notification_id = get_client_last_sent_notification(&redis_pool, &client_id).await;
                                clients_tx.insert(client_id, (client_tx, last_read_notification_id.map_or(StreamEntry::default(), |notification_id| notification_id.unwrap_or_default())));
                            },
                            None => {
                                CONNECTED_CLIENTS.dec();
                                error!("[Client Disconnected] : {:?}", client_id);
                                clients_tx.remove(&client_id);
                            }
                        }
                    },
                    None => {
                        error!("[Client Failed to Connect]");
                        continue;
                    },
                }
            },
            _ = reader_timer.tick() => {
                retry_timer += (reader_delay_seconds / retry_delay_seconds) as f64;
                let mut clients_seen_notification_id = FxHashMap::default();
                let clients_grouped_by_shard =
                    clients_tx
                        .keys()
                        .group_by(|client_id| hash_uuid(&client_id.inner()) % max_shards);
                for (shard, clients) in &clients_grouped_by_shard {
                    let clients_last_seen_notification_id = clients.map(|client_id| (client_id.clone(), clients_tx[client_id].1.clone())).collect();
                    match read_client_notifications(&redis_pool, clients_last_seen_notification_id, shard, reader_batch).await {
                        Ok(notifications) => {
                            for (client_id, notifications) in notifications {
                                for notification in notifications {
                                    if notification.ttl < Utc::now() {
                                        // Expired notifications
                                        EXPIRED_NOTIFICATIONS.inc();
                                        TOTAL_NOTIFICATIONS.dec();
                                        let _ = clean_up_notification(
                                            &redis_pool,
                                            &client_id,
                                            &notification.id.inner(),
                                            &notification.stream_id.inner(),
                                            shard,
                                        )
                                        .await.map_err(|err| error!("Error in clean_up_notification : {}", err));
                                    } else if let Some((client_tx, client_last_seen_stream_id)) = clients_tx.get(&ClientId(client_id.to_owned())) {
                                        // Older Sent Notifications to be sent again for retry
                                        if is_stream_id_less_or_eq(&notification.stream_id.inner(), client_last_seen_stream_id.inner().as_str()) {
                                            // Notifications whose acknowledgedement has been delayed since a duration `retry_delay_seconds`
                                            // from when it had to be sent `notification_stream_id` only has to be retried
                                            if retry_timer > retry_delay_seconds as f64 && can_retry(&notification.stream_id.inner(), client_last_seen_stream_id.inner().as_str(), retry_delay_seconds) {
                                                // Notifications to be retried
                                                RETRIED_NOTIFICATIONS.inc();
                                                retry_timer = 0.0;
                                                let _ = client_tx.send(Ok(transform_notification_data_to_payload(notification))).await.map_err(|err| error!("Error in client_tx.send : {}", err));
                                            }
                                        } else {
                                            // Send Notification First Time
                                            TOTAL_NOTIFICATIONS.inc();
                                            clients_seen_notification_id.insert(client_id.to_owned(), notification.stream_id.to_owned());
                                            let _ = set_notification_stream_id(&redis_pool, &notification.id.inner(), &notification.stream_id.inner(), notification.ttl).await.map_err(|err| error!("Error in set_notification_stream_id : {}", err));
                                            let _ = client_tx.send(Ok(transform_notification_data_to_payload(notification))).await.map_err(|err| error!("Error in client_tx.send : {}", err));
                                        }
                                    } else {
                                        warn!("Client ({:?}) entry does not exist, client got disconnected intermittently.", client_id);
                                    }
                                }
                            }
                        },
                        Err(err) => error!("Error in Reading Client Notifications : {:?}", err)
                    }
                }
                for (client_id, notification_stream_id) in clients_seen_notification_id {
                    if let Some((_, last_read_stream_id)) = clients_tx.get_mut(&ClientId(client_id.to_string())) {
                        *last_read_stream_id = notification_stream_id;
                    } else {
                        error!("Client {} Not Found in HashMap", client_id)
                    }
                }
            }
        }
    }
}
