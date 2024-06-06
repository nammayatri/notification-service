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
    measure_latency_duration, notification_latency,
    redis::{
        commands::{clean_up_notification, read_client_notifications, set_notification_stream_id},
        keys::notification_client_key,
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
    clients_tx: Arc<Vec<Vec<RwLock<ReaderMap>>>>,
    node: usize,
    shard: u64,
    client_id: &ClientId,
) {
    let handle_client_disconnection_or_failure_clients_tx_write_start_time =
        tokio::time::Instant::now();
    clients_tx
        .get(node)
        .unwrap()
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
    clients_tx: Arc<Vec<Vec<RwLock<ReaderMap>>>>,
    shard_to_node_id: Arc<FxHashMap<Shard, Node>>,
    max_shards: u64,
) {
    let shard = Shard((hash_uuid(&client_id.inner()) % max_shards as u128) as u64);
    let node = shard_to_node_id.get(&shard).unwrap();
    match client_tx {
        Some(client_tx) => {
            info!("[Client Connected] : {:?}", client_id);
            CONNECTED_CLIENTS.inc();

            let client_reciever_clients_tx_write_start_time = tokio::time::Instant::now();
            clients_tx
                .get(node.inner())
                .unwrap()
                .get(shard.inner() as usize)
                .unwrap()
                .write()
                .await
                .insert(client_id, client_tx);
            measure_latency_duration!(
                "client_reciever_clients_tx_write",
                client_reciever_clients_tx_write_start_time
            );
        }
        None => {
            warn!("[Client Disconnected] : {:?}", client_id);
            CONNECTED_CLIENTS.dec();
            handle_client_disconnection_or_failure(
                clients_tx.clone(),
                node.inner(),
                shard.inner(),
                &client_id,
            )
            .await;
        }
    }
}

async fn client_reciever_looper(
    mut read_notification_rx: UnboundedReceiver<(ClientId, Option<ClientTx>)>,
    clients_tx: Arc<Vec<Vec<RwLock<ReaderMap>>>>,
    shard_to_node_id: Arc<FxHashMap<Shard, Node>>,
    max_shards: u64,
) {
    while let Some((client_id, client_tx)) = read_notification_rx.recv().await {
        client_reciever(
            client_id,
            client_tx,
            clients_tx.clone(),
            shard_to_node_id.clone(),
            max_shards,
        )
        .await;
    }
    error!("Error: read_notification_rx closed");
}

#[macros::measure_duration]
async fn read_and_process_notification(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<Vec<RwLock<ReaderMap>>>>,
    max_shards: u64,
) {
    let read_client_notifications_batch_task: Vec<_> = clients_tx
        .iter()
        .enumerate()
        .map(|(_node, shards)|{
            let redis_pool_clone = redis_pool.clone();
            async move {
                let mut notification_stream_keys = Vec::new();
                for (shard, clients) in shards.iter().enumerate() {
                    let read_client_notifications_parallely_in_batch_task_clients_tx_read_start_time =
                            tokio::time::Instant::now();
                    let client_ids: Vec<ClientId> = clients
                        .read()
                        .await
                        .iter()
                        .map(|(client_id, _)| client_id.clone())
                        .collect();
                    measure_latency_duration!(
                        "read_client_notifications_parallely_in_batch_task_clients_tx_read",
                        read_client_notifications_parallely_in_batch_task_clients_tx_read_start_time
                    );
                    client_ids.iter().for_each(|client_id| {
                        notification_stream_keys.push(notification_client_key(&client_id.inner(), &(shard as u64)));
                    })
                }

                        let notifications = read_client_notifications(&redis_pool_clone, notification_stream_keys, max_shards).await;

                        match notifications {
                            Ok(notifications) => {
                                for (ClientId(client_id), notifications, shard) in notifications.into_iter() {
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
                                            let client_tx = shards
                                                .get(shard.inner() as usize)
                                                .unwrap()
                                                .read()
                                                .await
                                                .get(&ClientId(client_id.to_owned()))
                                                .cloned();
                                            measure_latency_duration!(
                                                "read_and_process_notification_clients_tx_read",
                                                read_and_process_notification_clients_tx_read_start_time
                                            );

                                            if let Some(client_tx) = client_tx {
                                                let (client_id_clone, client_tx_clone, redis_pool_clone, shard_clone ) = (client_id.clone(), client_tx.clone(), redis_pool_clone.clone(), shard.clone());
                                                tokio::spawn(async move {
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
                                                });
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
                                error!("read_client_notifications : {:?}", err);
                            }
                        }
            }
        }).collect();

    join_all(read_client_notifications_batch_task).await;
}

async fn read_and_process_notification_looper(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<Vec<RwLock<ReaderMap>>>>,
    max_shards: u64,
    delay: Duration,
) {
    loop {
        read_and_process_notification(redis_pool.clone(), clients_tx.clone(), max_shards).await;
        sleep(delay).await;
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_notification_reader(
    read_notification_rx: UnboundedReceiver<(ClientId, Option<ClientTx>)>,
    graceful_termination_signal_rx: sync::oneshot::Receiver<()>,
    redis_pool: Arc<RedisConnectionPool>,
    reader_delay_millis: u64,
    max_shards: u64,
    node_to_shard: FxHashMap<String, Vec<u64>>,
) {
    let clients_tx: Arc<Vec<Vec<RwLock<ReaderMap>>>> = Arc::new(
        (0..node_to_shard.len())
            .map(|_| {
                (0..max_shards)
                    .map(|_| RwLock::new(FxHashMap::default()))
                    .collect()
            })
            .collect(),
    );

    error!("NODE TO SHARD => {:?}", node_to_shard);

    let mut shard_to_node_id = FxHashMap::default();
    for (node_id, shards) in node_to_shard.into_values().enumerate() {
        for shard in shards.iter() {
            shard_to_node_id
                .entry(Shard(*shard))
                .or_insert(Node(node_id));
        }
    }
    let shard_to_node_id = Arc::new(shard_to_node_id);

    error!("SHARD TO NODE ID => {:?}", shard_to_node_id);

    let rx_task = tokio::spawn(client_reciever_looper(
        read_notification_rx,
        clients_tx.clone(),
        shard_to_node_id.clone(),
        max_shards,
    ));

    let process_notifications_task = tokio::spawn(read_and_process_notification_looper(
        redis_pool.clone(),
        clients_tx.clone(),
        max_shards,
        Duration::from_millis(reader_delay_millis),
    ));

    tokio::select!(
        res = rx_task => {
            error!("[CLIENT_RECIEVER_TASK] : {:?}", res);
        },
        res = process_notifications_task => {
            error!("[READ_PROCESS_NOTIFICATION_TASK] : {:?}", res);
        },
        _ = graceful_termination_signal_rx => {
            error!("[Graceful Shutting Down]");
        }
    );
}
