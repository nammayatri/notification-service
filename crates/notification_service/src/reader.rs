/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use crate::common::utils::{diff_utc, is_stream_id_less};
use crate::common::{types::*, utils::decode_notification_payload};
use crate::redis::keys::notification_duration_key;
use crate::tools::prometheus::{EXPIRED_NOTIFICATIONS, RETRIED_NOTIFICATIONS};
use crate::NotificationPayload;
use anyhow::Result;
use chrono::{DateTime, Utc};
use rustc_hash::FxHashMap;
use shared::redis::types::RedisConnectionPool;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::mpsc::{Receiver, Sender},
    time::interval,
};
use tonic::Status;
use tracing::{error, info};

pub async fn run_notification_reader(
    mut read_notification_rx: Receiver<(ClientId, Sender<Result<NotificationPayload, Status>>)>,
    redis_pool: Arc<RedisConnectionPool>,
    reader_delay_seconds: u64,
    retry_delay_seconds: u64,
) {
    let mut clients_tx: FxHashMap<
        ClientId,
        (
            Sender<Result<NotificationPayload, Status>>,
            LastReadStreamEntry,
        ),
    > = FxHashMap::default();
    let mut reader_timer = interval(Duration::from_secs(reader_delay_seconds));
    let mut retry_timer = interval(Duration::from_secs(retry_delay_seconds));

    loop {
        tokio::select! {
            item = read_notification_rx.recv() => {
                error!("[Client Connected] : {:?}", item);
                match item {
                    Some((client_id, client_tx)) => {
                        clients_tx.insert(client_id, (client_tx, LastReadStreamEntry::default()));
                    },
                    None => {
                        error!(tag = "[Client Failed to Connect]");
                        continue;
                    },
                }
            },
            _ = reader_timer.tick() => {
                let current_time = Utc::now();
                let client_stream_keys: Vec<String> = clients_tx.keys().map(|ClientId(client_id)| client_id.to_string()).collect();
                let client_stream_ids: Vec<String> = clients_tx.values().map(|(_, LastReadStreamEntry(last_read_stream_id))| last_read_stream_id.to_string()).collect();
                if !client_stream_keys.is_empty() {
                    if let Ok(notifications) = redis_pool.xread(client_stream_keys.to_owned(), client_stream_ids.to_owned()).await {
                        if let Ok(notifications) = decode_notification_payload(notifications) {
                            for (client_id, notifications) in notifications {
                                for notification in notifications {
                                    error!("NOTIFICATION notification : {:?} : {}", notification, client_id);
                                    let notification_ttl : DateTime<Utc> = notification.ttl.parse().unwrap();
                                    if notification_ttl < current_time {
                                        // Expired Notification
                                        EXPIRED_NOTIFICATIONS.inc();
                                        let _ = redis_pool.xdel(client_id.as_str(), notification.id.as_str()).await;
                                    } else {
                                        // Send Notifications
                                        if let Some((_, LastReadStreamEntry(last_read_stream_id))) = clients_tx.get_mut(&ClientId(client_id.to_string())) {
                                            *last_read_stream_id = notification.id.to_owned();
                                        }
                                        let _ = redis_pool.set_key(notification_duration_key(&NotificationId(notification.id.to_owned())).as_str(), Timestamp(Utc::now()), diff_utc(current_time, notification_ttl).num_seconds() as u32).await;
                                        let _ = clients_tx[&ClientId(client_id.to_owned())].0.send(Ok(notification)).await;
                                    }
                                }
                            }
                        }
                    }
                }
            },
            _ = retry_timer.tick() => {
                let current_time = Utc::now();
                let client_stream_keys: Vec<String> = clients_tx.keys().map(|ClientId(client_id)| client_id.to_string()).collect();
                let client_stream_ids: Vec<String> = clients_tx.values().map(|(_, LastReadStreamEntry(last_read_stream_id))| last_read_stream_id.to_string()).collect();
                if !client_stream_keys.is_empty() {
                    if let Ok(notifications) = redis_pool.xread(client_stream_keys.to_owned(), client_stream_ids.to_owned()).await {
                        if let Ok(notifications) = decode_notification_payload(notifications) {
                            for (client_id, notifications) in notifications {
                                for notification in notifications {
                                    if is_stream_id_less(notification.id.as_str(), clients_tx[&ClientId(client_id.to_owned())].1.0.as_str()) { // Older Sent Notifications to be sent again for retry
                                        let notification_ttl : DateTime<Utc> = notification.ttl.parse().unwrap();
                                        info!("RETRY NOTIFICATION notification : {} : {} : {}", notification_ttl, current_time, notification_ttl < current_time);
                                        if notification_ttl < current_time {
                                            // Expired notifications
                                            EXPIRED_NOTIFICATIONS.inc();
                                            let _ = redis_pool.xdel(client_id.as_str(), notification.id.as_str()).await;
                                        } else {
                                            // Notifications to be retried
                                            RETRIED_NOTIFICATIONS.inc();
                                            let _ = clients_tx[&ClientId(client_id.to_owned())].0.send(Ok(notification)).await;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            },
        }
    }
}
