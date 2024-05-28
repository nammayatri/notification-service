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
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{self, mpsc::UnboundedReceiver, RwLock},
    time::sleep,
};
use tracing::*;

#[macros::measure_duration]
async fn store_client_last_sent_notification_context(
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
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    last_known_notification_cache_expiry: u32,
) {
    for client_tx in clients_tx.iter() {
        let _ = set_clients_last_sent_notification(
            &redis_pool,
            get_clients_last_seen_notification_id(client_tx).await,
            last_known_notification_cache_expiry,
        )
        .await
        .map_err(|err| error!("Error in set_clients_last_sent_notification : {}", err));
    }
}

#[allow(clippy::type_complexity)]
#[macros::measure_duration]
async fn get_clients_last_seen_notification_id(
    clients_tx: &RwLock<ReaderMap>,
) -> Vec<(ClientId, StreamEntry)> {
    clients_tx
        .read()
        .await
        .clone()
        .iter()
        .filter_map(|(client_id, (_, last_read_stream_entry))| {
            last_read_stream_entry
                .clone()
                .map(|stream_entry| (client_id.clone(), stream_entry))
        })
        .collect()
}

#[macros::measure_duration]
async fn send_notification(
    client_tx: &ClientTx,
    notification: NotificationData,
    redis_pool: &RedisConnectionPool,
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

            if notification_curr_ts_diff > retry_delay_seconds as f64 {
                RETRIED_NOTIFICATIONS.inc();
                client_tx
                    .send(Ok(transform_notification_data_to_payload(notification)))
                    .await?;
            }
        }
    } else {
        RETRIED_NOTIFICATIONS.inc();
        client_tx
            .send(Ok(transform_notification_data_to_payload(notification)))
            .await?;
    }
    Ok(())
}

#[macros::measure_duration]
async fn read_client_notifications_parallely_in_batch_task(
    redis_pool: &RedisConnectionPool,
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    read_stream_from_beginning: bool,
) -> Vec<(Shard, ClientId, Vec<NotificationData>)> {
    let read_client_notifications_batch_task: Vec<_> = clients_tx
        .iter()
        .enumerate()
        .map(|(shard, clients)| {
            let shard = Shard(shard as u64);
            async move {
                let read_client_notifications_parallely_in_batch_task_clients_tx_read_start_time =
                    tokio::time::Instant::now();
                let clients_last_seen_notification_id = clients
                    .read()
                    .await
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
                measure_latency_duration!(
                    "read_client_notifications_parallely_in_batch_task_clients_tx_read",
                    read_client_notifications_parallely_in_batch_task_clients_tx_read_start_time
                );

                read_client_notifications(redis_pool, clients_last_seen_notification_id, &shard)
                    .await
            }
        })
        .collect();

    let results = join_all(read_client_notifications_batch_task).await;
    results.into_iter().flatten().flatten().collect()
}

#[macros::measure_duration]
async fn handle_client_disconnection_or_failure(
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    redis_pool: Arc<RedisConnectionPool>,
    last_known_notification_cache_expiry: u32,
    shard: u64,
    client_id: &ClientId,
) {
    let handle_client_disconnection_or_failure_clients_tx_read_start_time =
        tokio::time::Instant::now();
    let last_read_notification_id = match clients_tx
        .get(shard as usize)
        .unwrap()
        .read()
        .await
        .get(client_id)
    {
        Some((_, Some(last_read_notification_id))) => Some(last_read_notification_id.to_owned()),
        _ => None,
    };
    measure_latency_duration!(
        "handle_client_disconnection_or_failure_clients_tx_read",
        handle_client_disconnection_or_failure_clients_tx_read_start_time
    );

    if let Some(last_read_notification_id) = last_read_notification_id {
        store_client_last_sent_notification_context(
            redis_pool.clone(),
            client_id.inner(),
            last_read_notification_id.inner(),
            last_known_notification_cache_expiry,
        )
        .await;
    } else {
        error!(
            "Unable to store_client_last_sent_notification_context : {:?}",
            client_id
        )
    }

    let handle_client_disconnection_or_failure_clients_tx_write_start_time =
        tokio::time::Instant::now();
    clients_tx
        .get(shard as usize)
        .unwrap()
        .write()
        .await
        .remove(client_id);
    measure_latency_duration!(
        "handle_client_disconnection_or_failure_clients_tx_write",
        handle_client_disconnection_or_failure_clients_tx_write_start_time
    );
}

#[macros::measure_duration]
async fn client_reciever(
    client_id: ClientId,
    client_tx: Option<ClientTx>,
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    max_shards: u64,
    last_known_notification_cache_expiry: u32,
) {
    let Shard(shard) = Shard((hash_uuid(&client_id.inner()) % max_shards as u128) as u64);
    match client_tx {
        Some(client_tx) => {
            info!("[Client Connected] : {:?}", client_id);
            CONNECTED_CLIENTS.inc();
            let last_read_notification_id =
                get_client_last_sent_notification(&redis_pool, &client_id).await;

            let client_reciever_clients_tx_write_start_time = tokio::time::Instant::now();
            clients_tx
                .get(shard as usize)
                .expect("This error is impossible!")
                .write()
                .await
                .insert(
                    client_id,
                    (client_tx, last_read_notification_id.ok().flatten()),
                );
            measure_latency_duration!(
                "client_reciever_clients_tx_write",
                client_reciever_clients_tx_write_start_time
            );
        }
        None => {
            warn!("[Client Disconnected] : {:?}", client_id);
            handle_client_disconnection_or_failure(
                clients_tx.clone(),
                redis_pool.clone(),
                last_known_notification_cache_expiry,
                shard,
                &client_id,
            )
            .await;
        }
    }
}

async fn client_reciever_looper(
    mut read_notification_rx: UnboundedReceiver<(ClientId, Option<ClientTx>)>,
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    max_shards: u64,
    last_known_notification_cache_expiry: u32,
) {
    while let Some((client_id, client_tx)) = read_notification_rx.recv().await {
        client_reciever(
            client_id,
            client_tx,
            redis_pool.clone(),
            clients_tx.clone(),
            max_shards,
            last_known_notification_cache_expiry,
        )
        .await;
    }
    error!("Error: read_notification_rx closed");
}

#[macros::measure_duration]
async fn read_and_process_notification(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    _last_known_notification_cache_expiry: u32,
    max_shards: u64,
) {
    let read_client_notifications_batch_task_result =
        read_client_notifications_parallely_in_batch_task(
            &redis_pool.clone(),
            clients_tx.clone(),
            false,
        )
        .await;

    for (shard, ClientId(client_id), notifications) in
        read_client_notifications_batch_task_result.into_iter()
    {
        warn!(
            "[Notification_Shard_{}_ClientId-{}] => {:?}",
            shard.inner(),
            client_id,
            notifications
        );

        if notifications.is_empty() {
            measure_latency_duration!("empty_notifications", tokio::time::Instant::now());
        }

        for notification in notifications {
            TOTAL_NOTIFICATIONS.inc();

            if notification.ttl < Utc::now() {
                clear_expired_notification(
                    &Arc::clone(&redis_pool),
                    &shard.clone(),
                    &client_id,
                    &notification.id.clone(),
                    &notification.stream_id.clone(),
                )
                .await;
            } else {
                let read_and_process_notification_clients_tx_read_start_time =
                    tokio::time::Instant::now();
                let client_tx = clients_tx
                    .get(shard.inner() as usize)
                    .expect("This error is impossible!")
                    .read()
                    .await
                    .get(&ClientId(client_id.to_owned()))
                    .map(|(client_tx, _)| client_tx.clone());
                measure_latency_duration!(
                    "read_and_process_notification_clients_tx_read",
                    read_and_process_notification_clients_tx_read_start_time
                );

                if let Some(client_tx) = client_tx {
                    match send_notification(&client_tx, notification.to_owned(), &redis_pool).await
                    {
                        Ok(_) => {
                            let read_and_process_notification_clients_tx_write_start_time =
                                tokio::time::Instant::now();
                            if let Some((_, client_last_seen_stream_id)) = clients_tx
                                .get(shard.inner() as usize)
                                .expect("This error is impossible!")
                                .write()
                                .await
                                .get_mut(&ClientId(client_id.to_owned()))
                            {
                                *client_last_seen_stream_id =
                                    Some(notification.stream_id.to_owned());
                            } else {
                                warn!(
                                    "Client ({:?}) entry does not exist, client got disconnected intermittently.",
                                    client_id
                                );
                            }
                            measure_latency_duration!(
                                "read_and_process_notification_clients_tx_write",
                                read_and_process_notification_clients_tx_write_start_time
                            );

                            let _ = clean_up_notification(
                                &redis_pool,
                                &client_id,
                                &notification.id.inner(),
                                &notification.stream_id.inner(),
                                &Shard((hash_uuid(&client_id) % max_shards as u128) as u64),
                            )
                            .await
                            .map_err(|err| error!("Error in clean_up_notification : {}", err));
                        }
                        Err(err) => {
                            warn!("[Send Failed] : {}", err);
                        }
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
}

async fn read_and_process_notification_looper(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    last_known_notification_cache_expiry: u32,
    max_shards: u64,
) {
    loop {
        read_and_process_notification(
            redis_pool.clone(),
            clients_tx.clone(),
            last_known_notification_cache_expiry,
            max_shards,
        )
        .await;
        sleep(Duration::from_millis(100)).await;
    }
}

#[macros::measure_duration]
async fn retry_notifications(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    _last_known_notification_cache_expiry: u32,
    delay: Duration,
    max_shards: u64,
) {
    let read_client_notifications_batch_task_result =
        read_client_notifications_parallely_in_batch_task(
            &Arc::clone(&redis_pool),
            clients_tx.clone(),
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
                clear_expired_notification(
                    &Arc::clone(&redis_pool),
                    &shard.clone(),
                    &client_id,
                    &notification.id,
                    &notification.stream_id,
                )
                .await;
            } else {
                let retry_notifications_clients_tx_read_start_time = tokio::time::Instant::now();
                let client_tx_and_last_seen_notification_id = clients_tx
                    .get(shard.inner() as usize)
                    .expect("This error is impossible!")
                    .read()
                    .await
                    .get(&ClientId(client_id.to_owned()))
                    .map(|(client_tx, client_last_seen_stream_id)| {
                        (client_tx.clone(), client_last_seen_stream_id.clone())
                    });
                measure_latency_duration!(
                    "retry_notifications_clients_tx_write",
                    retry_notifications_clients_tx_read_start_time
                );

                if let Some((client_tx, client_last_seen_stream_id)) =
                    client_tx_and_last_seen_notification_id
                {
                    match retry_notification_if_eligible(
                        &client_tx,
                        &client_id,
                        &client_last_seen_stream_id,
                        delay.as_secs(),
                        notification.to_owned(),
                    )
                    .await
                    {
                        Ok(_) => {
                            let _ = clean_up_notification(
                                &redis_pool,
                                &client_id,
                                &notification.id.inner(),
                                &notification.stream_id.inner(),
                                &Shard((hash_uuid(&client_id) % max_shards as u128) as u64),
                            )
                            .await
                            .map_err(|err| error!("Error in clean_up_notification : {}", err));
                        }
                        Err(err) => {
                            warn!("[Send Failed] : {}", err);
                        }
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
}

async fn retry_notifications_looper(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    last_known_notification_cache_expiry: u32,
    delay: Duration,
    max_shards: u64,
) {
    loop {
        retry_notifications(
            redis_pool.clone(),
            clients_tx.clone(),
            last_known_notification_cache_expiry,
            delay,
            max_shards,
        )
        .await;
        sleep(delay).await;
    }
}

pub async fn run_notification_reader(
    read_notification_rx: UnboundedReceiver<(ClientId, Option<ClientTx>)>,
    graceful_termination_signal_rx: sync::oneshot::Receiver<()>,
    redis_pool: Arc<RedisConnectionPool>,
    retry_delay_seconds: u64,
    last_known_notification_cache_expiry: u32,
    max_shards: u64,
    _is_acknowledment_required: bool,
) {
    let clients_tx: Arc<Vec<RwLock<ReaderMap>>> = Arc::new(
        (0..max_shards)
            .map(|_| RwLock::new(FxHashMap::default()))
            .collect(),
    );

    let rx_task = tokio::spawn(client_reciever_looper(
        read_notification_rx,
        redis_pool.clone(),
        clients_tx.clone(),
        max_shards,
        last_known_notification_cache_expiry,
    ));

    let process_notifications_task = tokio::spawn(read_and_process_notification_looper(
        redis_pool.clone(),
        clients_tx.clone(),
        last_known_notification_cache_expiry,
        max_shards,
    ));

    let retry_notifications_task = tokio::spawn(retry_notifications_looper(
        redis_pool.clone(),
        clients_tx.clone(),
        last_known_notification_cache_expiry,
        Duration::from_secs(retry_delay_seconds),
        max_shards,
    ));

    tokio::select!(
        res = rx_task => {
            error!("[CLIENT_RECIEVER_TASK] : {:?}", res);
        },
        res = process_notifications_task => {
            error!("[READ_PROCESS_NOTIFICATION_TASK] : {:?}", res);
        },
        res = retry_notifications_task => {
            error!("[READ_PROCESS_NOTIFICATION_TASK] : {:?}", res);
        },
        _ = graceful_termination_signal_rx => {
            error!("[Graceful Shutting Down] => Storing following clients last read notification in redis : {:?}", clients_tx);
            store_clients_last_sent_notification_context(
                redis_pool.clone(),
                clients_tx.clone(),
                last_known_notification_cache_expiry,
            )
            .await;
        }
    );
}
