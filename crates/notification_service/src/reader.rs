/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use crate::common::types::*;
use crate::common::utils::is_stream_id_less;
use crate::kafka::producers::kafka_stream_notification_updates;
use crate::kafka::types::NotificationStatus;
use crate::redis::commands::{
    clean_up_notification, get_client_last_sent_notification, get_notification_start_time,
    read_client_notifications, set_clients_last_sent_notification, set_notification_start_time,
};
use crate::tools::prometheus::{EXPIRED_NOTIFICATIONS, RETRIED_NOTIFICATIONS};
use crate::NotificationPayload;
use anyhow::Result;
use chrono::{DateTime, Utc};
use rdkafka::producer::FutureProducer;
use rustc_hash::FxHashMap;
use shared::redis::types::RedisConnectionPool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::mpsc::{Receiver, Sender},
    time::interval,
};
use tonic::Status;
use tracing::*;

async fn clean_up_expired_notification(
    redis_pool: &RedisConnectionPool,
    client_id: &str,
    notification_id: &str,
    notification_created_at: &str,
    kafka_producer: &Option<FutureProducer>,
    kafka_topic: &str,
) {
    EXPIRED_NOTIFICATIONS.inc();

    match get_notification_start_time(redis_pool, notification_id).await {
        Ok(Some(start_time)) => {
            if let Ok(created_at) = notification_created_at.parse::<DateTime<Utc>>() {
                let (
                    cloned_kafka_producer,
                    cloned_kafka_topic,
                    cloned_client_id,
                    cloned_notification_id,
                ) = (
                    kafka_producer.clone(),
                    kafka_topic.to_string(),
                    client_id.to_string(),
                    notification_id.to_string(),
                );
                tokio::spawn(async move {
                    let _ = kafka_stream_notification_updates(
                        &cloned_kafka_producer,
                        &cloned_kafka_topic,
                        &cloned_client_id,
                        cloned_notification_id,
                        0,
                        NotificationStatus::EXPIRED,
                        Timestamp(created_at),
                        start_time,
                        None,
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

    let _ = clean_up_notification(redis_pool, client_id, notification_id).await;
}

#[allow(clippy::type_complexity)]
fn get_clients_last_seen_notification_id(
    clients_tx: &FxHashMap<
        ClientId,
        (
            Sender<Result<NotificationPayload, Status>>,
            LastReadStreamEntry,
        ),
    >,
) -> Vec<(ClientId, LastReadStreamEntry)> {
    clients_tx
        .iter()
        .map(|(client_id, (_, last_read_stream_entry))| {
            (client_id.clone(), last_read_stream_entry.clone())
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn run_notification_reader(
    mut read_notification_rx: Receiver<(ClientId, Sender<Result<NotificationPayload, Status>>)>,
    graceful_termination_requested: Arc<AtomicBool>,
    redis_pool: Arc<RedisConnectionPool>,
    reader_delay_seconds: u64,
    retry_delay_seconds: u64,
    last_known_notification_cache_expiry: u32,
    kafka_producer: Option<FutureProducer>,
    kafka_topic: String,
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
        if graceful_termination_requested.load(Ordering::Relaxed) {
            error!("[Graceful Shutting Down] => Storing following clients last read notification in redis : {:?}", clients_tx);

            let _ = set_clients_last_sent_notification(
                &redis_pool,
                get_clients_last_seen_notification_id(&clients_tx),
                last_known_notification_cache_expiry,
            )
            .await;

            break;
        }
        tokio::select! {
            item = read_notification_rx.recv() => {
                error!("[Client Connected] : {:?}", item);
                match item {
                    Some((client_id, client_tx)) => {
                        let last_read_notification_id = get_client_last_sent_notification(&redis_pool, &client_id).await;
                        clients_tx.insert(client_id, (client_tx, last_read_notification_id.map_or(LastReadStreamEntry::default(), |notification_id| notification_id.unwrap_or_default())));
                    },
                    None => {
                        error!("[Client Failed to Connect]");
                        continue;
                    },
                }
            },
            _ = reader_timer.tick() => {
                match read_client_notifications(&redis_pool, get_clients_last_seen_notification_id(&clients_tx)).await {
                    Ok(notifications) => {
                        for (client_id, notifications) in notifications {
                            for notification in notifications {
                                if let Ok(notification_ttl) = notification.ttl.parse::<DateTime<Utc>>() {
                                    if notification_ttl < Utc::now() {
                                        // Expired Notification
                                        let _ = clean_up_expired_notification(&redis_pool, &client_id, &notification.id, &notification.created_at, &kafka_producer, &kafka_topic).await;
                                    } else {
                                        // Send Notifications
                                        if let Some((_, LastReadStreamEntry(last_read_stream_id))) = clients_tx.get_mut(&ClientId(client_id.to_string())) {
                                            *last_read_stream_id = notification.id.to_owned();
                                        }
                                        let _ = set_notification_start_time(&redis_pool, &notification.id, notification_ttl).await;
                                        let _ = clients_tx[&ClientId(client_id.to_owned())].0.send(Ok(notification)).await;
                                    }
                                } else {
                                    error!("Error in parsing notification TTL for notification : {:?}", notification);
                                }
                            }
                        }
                    },
                    Err(err) => error!("Error in Reading Client Notifications during Retry : {:?}", err)
                }
            },
            _ = retry_timer.tick() => {
                match read_client_notifications(&redis_pool, get_clients_last_seen_notification_id(&clients_tx)).await {
                    Ok(notifications) => {
                        for (client_id, notifications) in notifications {
                            for notification in notifications {
                                if is_stream_id_less(notification.id.as_str(), clients_tx[&ClientId(client_id.to_owned())].1.0.as_str()) { // Older Sent Notifications to be sent again for retry
                                    if let Ok(notification_ttl) = notification.ttl.parse::<DateTime<Utc>>() {
                                        if notification_ttl < Utc::now() {
                                            // Expired notifications
                                            let _ = clean_up_expired_notification(&redis_pool, &client_id, &notification.id, &notification.created_at, &kafka_producer, &kafka_topic).await;
                                        } else {
                                            // Notifications to be retried
                                            RETRIED_NOTIFICATIONS.inc();
                                            let _ = clients_tx[&ClientId(client_id.to_owned())].0.send(Ok(notification)).await;
                                        }
                                    } else {
                                        error!("Error in parsing notification TTL for retrying notification : {:?}", notification);
                                    }
                                }
                            }
                        }
                    },
                    Err(err) => error!("Error in Reading Client Notifications during Retry : {:?}", err)
                }
            },
        }
    }
}
