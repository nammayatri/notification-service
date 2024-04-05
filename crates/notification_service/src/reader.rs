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
        utils::{
            abs_diff_utc_as_sec, get_timestamp_from_stream_id, hash_uuid, is_stream_id_less_or_eq,
            transform_notification_data_to_payload,
        },
    },
    measure_latency_duration,
    redis::{
        commands::{
            clean_up_notification, get_client_last_sent_notification, read_client_notifications,
            set_client_last_sent_notification, set_clients_last_sent_notification,
            set_notification_stream_id,
        },
        types::NotificationData,
    },
    tools::prometheus::{
        CONNECTED_CLIENTS, EXPIRED_NOTIFICATIONS, MEASURE_DURATION, RETRIED_NOTIFICATIONS,
        TOTAL_NOTIFICATIONS,
    },
};
use anyhow::Result;
use chrono::Utc;
use futures::future::join_all;
use rustc_hash::FxHashMap;
use shared::redis::types::RedisConnectionPool;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{sync::mpsc::Receiver, time::interval};
use tracing::*;

#[macros::measure_duration]
async fn _store_client_last_sent_notification_context(
    redis_pool: Arc<RedisConnectionPool>,
    client_id: String,
    last_seen_notification_id: String,
    last_known_notification_cache_expiry: u32,
) {
    let _ = set_client_last_sent_notification(
        &redis_pool,
        client_id,
        last_seen_notification_id,
        last_known_notification_cache_expiry,
    )
    .await
    .map_err(|err| error!("Error in set_client_last_sent_notification : {}", err));
}

#[macros::measure_duration]
async fn store_clients_last_sent_notification_context(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: &ReaderMap,
    last_known_notification_cache_expiry: u32,
) {
    let _ = set_clients_last_sent_notification(
        &redis_pool,
        get_clients_last_seen_notification_id(clients_tx).await,
        last_known_notification_cache_expiry,
    )
    .await
    .map_err(|err| error!("Error in set_clients_last_sent_notification : {}", err));
}

#[allow(clippy::type_complexity)]
#[macros::measure_duration]
async fn get_clients_last_seen_notification_id(
    clients_tx: &ReaderMap,
) -> Vec<(ClientId, StreamEntry)> {
    clients_tx
        .iter()
        .flat_map(|(_, clients)| {
            clients
                .iter()
                .filter_map(|(client_id, (_, last_read_stream_entry))| {
                    last_read_stream_entry
                        .clone()
                        .map(|stream_entry| (client_id.clone(), stream_entry))
                })
        })
        .collect()
}

#[macros::measure_duration]
async fn send_notification(
    client_tx: &ClientTx,
    notification: NotificationData,
    redis_pool: &RedisConnectionPool,
    client_last_seen_stream_id: &mut Option<StreamEntry>,
) -> Result<()> {
    client_tx
        .send(Ok(transform_notification_data_to_payload(
            notification.clone(),
        )))
        .await?;

    let _ = set_notification_stream_id(
        redis_pool,
        &notification.id.inner(),
        &notification.stream_id.inner(),
        notification.ttl,
    )
    .await
    .map_err(|err| error!("Error in set_notification_stream_id : {:?}", err));
    TOTAL_NOTIFICATIONS.inc();
    *client_last_seen_stream_id = Some(notification.stream_id);
    Ok(())
}

#[macros::measure_duration]
async fn clear_expired_notification(
    redis_pool: &RedisConnectionPool,
    shard: &Shard,
    client_id: &str,
    notification_id: &NotificationId,
    notification_stream_id: &StreamEntry,
) {
    EXPIRED_NOTIFICATIONS.inc();
    let _ = clean_up_notification(
        redis_pool,
        client_id,
        &notification_id.inner(),
        &notification_stream_id.inner(),
        shard,
    )
    .await
    .map_err(|err| error!("Error in clean_up_notification : {}", err));
}

#[macros::measure_duration]
async fn retry_notification_if_eligible(
    client_tx: &ClientTx,
    client_id: &str,
    client_last_seen_stream_id: &Option<StreamEntry>,
    retry_delay_seconds: u64,
    notification: NotificationData,
) -> Result<()> {
    if let Some(client_last_seen_stream_id) = client_last_seen_stream_id {
        // Older Sent Notifications to be sent again for retry
        if is_stream_id_less_or_eq(
            &notification.stream_id.inner(),
            client_last_seen_stream_id.inner().as_str(),
        ) {
            let Timestamp(notification_ts) =
                get_timestamp_from_stream_id(&notification.stream_id.inner());
            let notification_curr_ts_diff = abs_diff_utc_as_sec(notification_ts, Utc::now());

            debug!("[Notification_ClientId-{}] => NotificationTimestamp : {}, NotificationCurrentTimeDiff : {}, RetryDelay : {}", client_id, notification_ts, notification_curr_ts_diff, retry_delay_seconds);

            if notification_curr_ts_diff > retry_delay_seconds {
                client_tx
                    .send(Ok(transform_notification_data_to_payload(notification)))
                    .await?;
                RETRIED_NOTIFICATIONS.inc();
            }
        }
    }
    Ok(())
}

#[macros::measure_duration]
async fn read_client_notifications_parallely_in_batch_task(
    redis_pool: &RedisConnectionPool,
    clients_tx: &ReaderMap,
    read_stream_from_beginning: bool,
) -> Vec<(Shard, ClientId, Vec<NotificationData>)> {
    let read_client_notifications_batch_task: Vec<_> = clients_tx
        .iter()
        .map(|(shard, clients)| {
            let clients_last_seen_notification_id = clients
                .iter()
                .map(|(client_id, (_, last_seen_stream_id))| {
                    let stream_id = if read_stream_from_beginning {
                        StreamEntry::default()
                    } else {
                        last_seen_stream_id.clone().unwrap_or_default()
                    };
                    (client_id.clone(), stream_id)
                })
                .collect();
            read_client_notifications(redis_pool, clients_last_seen_notification_id, shard)
        })
        .collect();

    join_all(read_client_notifications_batch_task)
        .await
        .into_iter()
        .flat_map(|result| {
            if let Err(err) = &result {
                error!("Error in read_client_notifications_batch_task: {:?}", err);
            }
            result.ok()
        })
        .flatten()
        .collect()
}

#[macros::measure_duration]
async fn client_reciever(
    notification_rx: Option<(ClientId, Option<ClientTx>)>,
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: &mut ReaderMap,
    max_shards: u64,
    last_known_notification_cache_expiry: u32,
) {
    if let Some((client_id, client_tx)) = notification_rx {
        let shard = Shard(hash_uuid(&client_id.inner()) % max_shards);
        match client_tx {
            Some(client_tx) => {
                info!("[Client Connected] : {:?}", client_id);
                CONNECTED_CLIENTS.inc();
                let last_read_notification_id =
                    get_client_last_sent_notification(&redis_pool, &client_id).await;
                clients_tx.entry(shard).or_default().insert(
                    client_id,
                    (client_tx, last_read_notification_id.ok().flatten()),
                );
            }
            None => {
                warn!("[Client Disconnected] : {:?}", client_id);
                clients_tx
                    .get_mut(&shard)
                    .and_then(|clients| clients.remove(&client_id));
                store_clients_last_sent_notification_context(
                    redis_pool.clone(),
                    clients_tx,
                    last_known_notification_cache_expiry,
                )
                .await;
            }
        }
    } else {
        error!("Error : read_notification_rx gave None");
    }
}

#[macros::measure_duration]
async fn read_and_process_notification(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: &mut ReaderMap,
) {
    let read_client_notifications_batch_task_result =
        read_client_notifications_parallely_in_batch_task(&redis_pool.clone(), clients_tx, false)
            .await;

    for (shard, ClientId(client_id), notifications) in
        read_client_notifications_batch_task_result.into_iter()
    {
        debug!(
            "[Notification_Shard_{}_ClientId-{}] => {:?}",
            shard.inner(),
            client_id,
            notifications
        );

        for notification in notifications {
            if notification.ttl < Utc::now() {
                let (redis_pool, shard, client_id, notification_id, notification_stream_id) = (
                    redis_pool.clone(),
                    shard.clone(),
                    client_id.clone(),
                    notification.id.clone(),
                    notification.stream_id.clone(),
                );
                tokio::spawn(async move {
                    clear_expired_notification(
                        &Arc::clone(&redis_pool),
                        &shard.clone(),
                        &client_id,
                        &notification_id,
                        &notification_stream_id,
                    )
                    .await;
                });
            } else if let Some((client_tx, ref mut client_last_seen_stream_id)) = clients_tx
                .get_mut(&shard)
                .and_then(|clients| clients.get_mut(&ClientId(client_id.to_owned())))
            {
                if let Err(err) = send_notification(
                    client_tx,
                    notification.to_owned(),
                    &redis_pool,
                    client_last_seen_stream_id,
                )
                .await
                {
                    warn!("[Send Failed] : {}", err);
                }
            } else {
                warn!(
                    "Client ({:?}) entry does not exist, client got disconnected intermittently.",
                    client_id
                );
            }
        }
    }
}

#[macros::measure_duration]
async fn retry_notifications(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: &mut ReaderMap,
    delay: Duration,
) {
    let read_client_notifications_batch_task_result =
        read_client_notifications_parallely_in_batch_task(
            &Arc::clone(&redis_pool),
            clients_tx,
            true,
        )
        .await;

    for (shard, ClientId(client_id), notifications) in
        read_client_notifications_batch_task_result.into_iter()
    {
        debug!(
            "[Notification_Shard_{}_ClientId-{}] => {:?}",
            shard.inner(),
            client_id,
            notifications
        );

        for notification in notifications {
            if notification.ttl < Utc::now() {
                let (redis_pool, shard, client_id, notification_id, notification_stream_id) = (
                    redis_pool.clone(),
                    shard.clone(),
                    client_id.clone(),
                    notification.id.clone(),
                    notification.stream_id.clone(),
                );
                tokio::spawn(async move {
                    clear_expired_notification(
                        &Arc::clone(&redis_pool),
                        &shard.clone(),
                        &client_id,
                        &notification_id,
                        &notification_stream_id,
                    )
                    .await;
                });
            } else if let Some((client_tx, client_last_seen_stream_id)) = clients_tx
                .get_mut(&shard)
                .and_then(|clients| clients.get_mut(&ClientId(client_id.to_owned())))
            {
                if let Err(err) = retry_notification_if_eligible(
                    client_tx,
                    &client_id,
                    client_last_seen_stream_id,
                    delay.as_secs(),
                    notification,
                )
                .await
                {
                    warn!("[Send Failed] : {}", err);
                }
            } else {
                warn!(
                    "Client ({:?}) entry does not exist, client got disconnected intermittently.",
                    client_id
                );
            }
        }
    }
}

async fn graceful_termination_of_connected_clients(
    graceful_termination_requested: Arc<AtomicBool>,
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: &ReaderMap,
    last_known_notification_cache_expiry: u32,
) {
    if graceful_termination_requested.load(Ordering::Relaxed) {
        error!("[Graceful Shutting Down] => Storing following clients last read notification in redis : {:?}", clients_tx);
        store_clients_last_sent_notification_context(
            redis_pool.clone(),
            clients_tx,
            last_known_notification_cache_expiry,
        )
        .await;
    }
}

pub async fn run_notification_reader(
    mut read_notification_rx: Receiver<(ClientId, Option<ClientTx>)>,
    graceful_termination_requested: Arc<AtomicBool>,
    redis_pool: Arc<RedisConnectionPool>,
    reader_delay_seconds: u64,
    retry_delay_seconds: u64,
    last_known_notification_cache_expiry: u32,
    max_shards: u64,
) {
    let mut clients_tx: ReaderMap = FxHashMap::default();

    let mut process_notifications_task_timer = interval(Duration::from_secs(reader_delay_seconds));
    let mut retry_notifications_task_timer = interval(Duration::from_secs(retry_delay_seconds));

    loop {
        graceful_termination_of_connected_clients(
            graceful_termination_requested.clone(),
            redis_pool.clone(),
            &clients_tx,
            last_known_notification_cache_expiry,
        )
        .await;

        tokio::select!(
            notification_rx = read_notification_rx.recv() => {
                client_reciever(
                    notification_rx,
                    redis_pool.clone(),
                    &mut clients_tx,
                    max_shards,
                    last_known_notification_cache_expiry,
                ).await;
            },
            _ = process_notifications_task_timer.tick() => {
                read_and_process_notification(
                    redis_pool.clone(),
                    &mut clients_tx,
                ).await;
            },
            _ = retry_notifications_task_timer.tick() => {
                retry_notifications(
                    redis_pool.clone(),
                    &mut clients_tx,
                    Duration::from_secs(retry_delay_seconds),
                ).await;
            }
        );
    }
}
