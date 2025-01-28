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
            abs_diff_utc_as_sec, get_timestamp_from_stream_id, hash_uuid,
            transform_notification_data_to_payload,
        },
    },
    notification_latency,
    redis::{
        commands::{
            clean_up_notification, handle_retry_clients, read_client_notifications,
            set_notification_stream_id,
        },
        keys::pubsub_channel_key,
        types::NotificationData,
    },
    tools::prometheus::{
        CONNECTED_CLIENTS, EXPIRED_NOTIFICATIONS, MEASURE_DURATION, NOTIFICATION_LATENCY,
        TOTAL_NOTIFICATIONS,
    },
};
use anyhow::Result;
use chrono::Utc;
use futures::future::join_all;
use rustc_hash::FxHashMap;
use shared::measure_latency_duration;
use shared::redis::types::RedisConnectionPool;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{self, mpsc::UnboundedReceiver, RwLock},
    time::sleep,
};
use tracing::*;

#[macros::measure_duration]
async fn send_notification(
    client_id: &str,
    client_tx: &ClientTx,
    notification: NotificationData,
    redis_pool: &RedisConnectionPool,
    shard: &Shard,
) -> Result<()> {
    let _ = set_notification_stream_id(
        redis_pool,
        &notification.id.inner(),
        &notification.stream_id.inner(),
        notification.ttl,
    )
    .await
    .map_err(|err| error!("Error in set_notification_stream_id : {:?}", err));

    client_tx
        .send(Ok(transform_notification_data_to_payload(
            notification.clone(),
        )))
        .await?;

    notification_latency!(
        get_timestamp_from_stream_id(&notification.stream_id.inner()).inner(),
        "NACK"
    );

    let _ = clean_up_notification(
        redis_pool,
        client_id,
        &notification.stream_id.inner(),
        shard,
    )
    .await
    .map_err(|err| error!("Error in clean_up_notification : {}", err));

    Ok(())
}

#[macros::measure_duration]
async fn clear_expired_notification(
    redis_pool: &RedisConnectionPool,
    shard: &Shard,
    client_id: &str,
    notification_stream_id: &StreamEntry,
) {
    EXPIRED_NOTIFICATIONS.inc();

    let _ = clean_up_notification(
        redis_pool,
        client_id,
        &notification_stream_id.inner(),
        shard,
    )
    .await
    .map_err(|err| error!("Error in clean_up_notification : {}", err));
}

#[macros::measure_duration]
async fn handle_client_disconnection_or_failure(
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    shard: u64,
    client_id: &ClientId,
) {
    let handle_client_disconnection_or_failure_clients_tx_write_start_time =
        tokio::time::Instant::now();
    clients_tx
        .get(shard as usize)
        .expect("This is impossible.")
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
    client_tx: SenderType,
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    max_shards: u64,
) {
    let Shard(shard) = Shard((hash_uuid(&client_id.inner()) % max_shards as u128) as u64);
    match client_tx {
        SenderType::ClientConnection(Some(client_tx)) => {
            info!("[Client Connected] : {:?}", client_id);
            CONNECTED_CLIENTS.inc();

            let client_reciever_clients_tx_write_start_time = tokio::time::Instant::now();
            clients_tx
                .get(shard as usize)
                .expect("This error is impossible!")
                .write()
                .await
                .insert(client_id, (ActiveNotificationCounter(0), client_tx));
            measure_latency_duration!(
                "client_reciever_clients_tx_write",
                client_reciever_clients_tx_write_start_time
            );
        }
        SenderType::ClientConnection(None) => {
            warn!("[Client Disconnected] : {:?}", client_id);
            CONNECTED_CLIENTS.dec();
            handle_client_disconnection_or_failure(clients_tx.clone(), shard, &client_id).await;
        }
        SenderType::ClientAck => {
            let mut clients_map = clients_tx
                .get(shard as usize)
                .expect("This error is impossible")
                .write()
                .await;
            if let Some((active_notification, _sender)) =
                clients_map.get_mut(&ClientId(client_id.inner().clone()))
            {
                active_notification.decrement(1);
            } else {
                error!("ClientId not found in the shard");
            }
        }
    }
}

pub fn get_bucket_from_timestamp(bucket_expiry_in_seconds: &u64, ts: u64) -> u64 {
    ts / bucket_expiry_in_seconds
}

async fn client_reciever_looper(
    redis_pool: Arc<RedisConnectionPool>,
    mut read_notification_rx: UnboundedReceiver<(ClientId, SenderType)>,
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    max_shards: u64,
    redis_retry_key_window: u64,
) {
    while let Some((client_id, client_tx)) = read_notification_rx.recv().await {
        client_reciever(client_id, client_tx, clients_tx.clone(), max_shards).await;
    }
    handle_retry_clients(redis_pool, clients_tx, max_shards, redis_retry_key_window).await;
    error!("Error: read_notification_rx closed");
}

#[macros::measure_duration]
async fn read_and_process_notification(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
) {
    let read_client_notifications_batch_task: Vec<_> = clients_tx
        .iter()
        .enumerate()
        .map(|(shard, clients)| {
            let shard = Shard(shard as u64);
            let (redis_pool_clone, clients_tx_clone) = (redis_pool.clone(), clients_tx.clone());
            async move {
                let read_client_notifications_parallely_in_batch_task_clients_tx_read_start_time =
                    tokio::time::Instant::now();
                let client_ids = clients
                    .read()
                    .await
                    .iter()
                    .filter(|(_, (active_notification, _))| active_notification.inner() > 0)
                    .map(|(client_id, _)| client_id.clone())
                    .collect();
                measure_latency_duration!(
                    "read_client_notifications_parallely_in_batch_task_clients_tx_read",
                    read_client_notifications_parallely_in_batch_task_clients_tx_read_start_time
                );

                let notifications = read_client_notifications(&redis_pool_clone, client_ids, &shard).await;

                match notifications {
                    Ok(notifications) => {
                        for (ClientId(client_id), notifications) in notifications.into_iter() {
                            if notifications.is_empty() {
                                let mut client_tx_map = clients_tx_clone.get(shard.inner() as usize).expect("This error is impossible!").write().await;
                                match client_tx_map.get_mut(&ClientId(client_id.clone())) {
                                    Some((active_notification, _sender)) => {
                                        active_notification.reset();
                                    }
                                    None => {
                                        error!("No active notification found for client ID {:?}", client_id);
                                    }
                                }
                                continue;
                            }
                            for notification in notifications {
                                TOTAL_NOTIFICATIONS.inc();

                                if notification.ttl < Utc::now() {
                                    clear_expired_notification(
                                        &Arc::clone(&redis_pool_clone),
                                        &shard.clone(),
                                        &client_id,
                                        &notification.stream_id.clone(),
                                    )
                                    .await;
                                } else {
                                    let read_and_process_notification_clients_tx_read_start_time =
                                        tokio::time::Instant::now();
                                    let client_tx = clients_tx_clone
                                        .get(shard.inner() as usize)
                                        .expect("This error is impossible!")
                                        .read()
                                        .await
                                        .get(&ClientId(client_id.to_owned()))
                                        .cloned();
                                    measure_latency_duration!(
                                        "read_and_process_notification_clients_tx_read",
                                        read_and_process_notification_clients_tx_read_start_time
                                    );

                                    if let Some(client_tx) = client_tx {
                                        let (_active_notification, sender) = client_tx;
                                        let (client_id_clone, client_tx_clone, redis_pool_clone, shard_clone ) = (client_id.clone(), sender.clone(), redis_pool_clone.clone(), shard.clone());
                                        // tokio::spawn(async move {
                                            if let Err(err) = send_notification(
                                                &client_id_clone,
                                                &client_tx_clone,
                                                notification.to_owned(),
                                                &redis_pool_clone,
                                                &shard_clone,
                                            )
                                            .await
                                            {
                                                warn!("[Send Failed] : {}", err);
                                            }
                                        // });
                                    } else {
                                        warn!(
                                            "Client ({:?}) entry does not exist, client got disconnected intermittently.",
                                            client_id
                                        );
                                    }
                                }
                            }
                        }
                    },
                    Err(err) => {
                        error!("read_client_notifications : {}", err);
                    }
                }
            }
        })
        .collect();

    join_all(read_client_notifications_batch_task).await;
}

async fn read_and_process_notification_looper(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    delay: Duration,
) {
    loop {
        read_and_process_notification(redis_pool.clone(), clients_tx.clone()).await;
        sleep(delay).await;
    }
}

async fn update_active_notification(
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    active_notification_receiver_stream: &mut UnboundedReceiver<(String, String)>,
    max_shards: u64,
) {
    loop {
        let res = active_notification_receiver_stream.recv().await;
        match res {
            Some((_channel_name, client_id)) => {
                let Shard(shard) = Shard((hash_uuid(&client_id) % max_shards as u128) as u64);
                let mut clients_map = clients_tx
                    .get(shard as usize)
                    .expect("This error is impossible")
                    .write()
                    .await;
                if let Some((active_notification, _client_tx)) =
                    clients_map.get_mut(&ClientId(client_id.clone()))
                {
                    active_notification.increment(1);
                    info!(
                        "Updated ActiveNotificationCounter for client: {}",
                        client_id
                    );
                } else {
                    info!("ClientId {} not found here", client_id);
                }
            }
            None => error!("Issue found in the active notification receiver stream."),
        }
    }
}

async fn update_active_notification_looper(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    max_shards: u64,
) {
    let pubsub_channel_key = pubsub_channel_key();
    let mut active_notification_receiver_stream = redis_pool
        .subscribe_channel_as_str(pubsub_channel_key)
        .await
        .unwrap();
    update_active_notification(
        clients_tx.clone(),
        &mut active_notification_receiver_stream,
        max_shards,
    )
    .await;
}

async fn transfer_clients_to_redis(
    clients_tx: Arc<Vec<RwLock<ReaderMap>>>,
    redis_pool: Arc<RedisConnectionPool>,
    redis_retry_key_window: u64,
    redis_retry_bucket_expiry: u64,
) {
    let redis_retry_bucket_key =
        (Utc::now().timestamp() as u64 / redis_retry_key_window).to_string();
    let mut result: Vec<(String, u64)> = Vec::new();
    for rwlock in clients_tx.iter() {
        let reader_map = rwlock.read().await;
        for (client_id, (counter, _)) in reader_map.iter() {
            if counter.inner() > 0 {
                result.push((client_id.clone().inner(), counter.inner()));
            }
        }
    }
    let _ = redis_pool
        .set_hash_fields_with_hashmap_expiry(
            &redis_retry_bucket_key,
            result,
            redis_retry_bucket_expiry as i64,
        )
        .await;
}

#[allow(clippy::too_many_arguments)]
pub async fn run_notification_reader(
    read_notification_rx: UnboundedReceiver<(ClientId, SenderType)>,
    graceful_termination_signal_rx: sync::oneshot::Receiver<()>,
    redis_pool: Arc<RedisConnectionPool>,
    reader_delay_millis: u64,
    max_shards: u64,
    redis_retry_bucket_expiry: u64,
    redis_retry_key_window: u64,
) {
    let clients_tx: Arc<Vec<RwLock<ReaderMap>>> = Arc::new(
        (0..max_shards)
            .map(|_| RwLock::new(FxHashMap::default()))
            .collect(),
    );

    let rx_task = tokio::spawn(client_reciever_looper(
        redis_pool.clone(),
        read_notification_rx,
        clients_tx.clone(),
        max_shards,
        redis_retry_key_window,
    ));

    let update_active_notifications = tokio::spawn(update_active_notification_looper(
        redis_pool.clone(),
        clients_tx.clone(),
        max_shards,
    ));

    let process_notifications_task = tokio::spawn(read_and_process_notification_looper(
        redis_pool.clone(),
        clients_tx.clone(),
        Duration::from_millis(reader_delay_millis),
    ));
    tokio::select!(
        res = rx_task => {
            error!("[CLIENT_RECIEVER_TASK] : {:?}", res);
        },
        res = process_notifications_task => {
            error!("[READ_PROCESS_NOTIFICATION_TASK] : {:?}", res);
        },
        res = update_active_notifications => {
            error!("[UPDATE_ACTIVE_NOTIFICATION_TASK] : {:?}", res);
        },
        _ = graceful_termination_signal_rx => {
            error!("[Graceful Shutting Down]");
            transfer_clients_to_redis(clients_tx.clone(),redis_pool,redis_retry_key_window,redis_retry_bucket_expiry).await;
        }
    );
}
