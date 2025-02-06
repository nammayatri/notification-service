/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use crate::{
    channel_delay,
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
            clean_up_notification, read_client_notification, read_client_notifications,
            set_notification_stream_id,
        },
        keys::*,
        types::NotificationData,
    },
    tools::prometheus::{
        CHANNEL_DELAY, CONNECTED_CLIENTS, EXPIRED_NOTIFICATIONS, MEASURE_DURATION,
        NOTIFICATION_LATENCY, RETRIED_NOTIFICATIONS, TOTAL_NOTIFICATIONS,
    },
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::{future::join_all, stream, StreamExt};
use rustc_hash::FxHashMap;
use shared::measure_latency_duration;
use shared::redis::types::RedisConnectionPool;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{self, mpsc::UnboundedReceiver},
    time::sleep,
};
use tracing::*;

#[macros::measure_duration]
async fn send_notification(
    client_id: &ClientId,
    client_tx: &ClientTx,
    notification: NotificationData,
    redis_pool: &RedisConnectionPool,
    shard: &Shard,
) -> Result<()> {
    let _ = set_notification_stream_id(
        redis_pool,
        &notification.id.inner(),
        &notification.stream_id.inner(),
        notification.ttl.inner(),
    )
    .await
    .map_err(|err| {
        error!(
            "[Notification Service Error] - Error in set_notification_stream_id : {:?}",
            err
        )
    });

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
        &client_id.inner(),
        &notification.stream_id.inner(),
        shard,
    )
    .await
    .map_err(|err| {
        error!(
            "[Notification Service Error] - Error in clean_up_notification : {}",
            err
        )
    });

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
    .map_err(|err| {
        error!(
            "[Notification Service Error] - Error in clean_up_notification : {}",
            err
        )
    });
}

#[macros::measure_duration]
async fn handle_client_disconnection_or_failure(
    clients_tx: Arc<Vec<MonitoredRwLock<ReaderMap>>>,
    shard: &Shard,
    client_id: &ClientId,
    session_id: &Option<SessionID>,
) {
    let handle_client_disconnection_or_failure_clients_tx_write_start_time =
        tokio::time::Instant::now();

    let mut client = clients_tx
        .get(shard.inner() as usize)
        .expect("This error is impossible")
        .write(RwLockName::ClientTxStore, RwLockOperation::Write)
        .await;

    match client.get_mut(client_id) {
        Some(SessionMap::Single(_)) => {
            client.remove(client_id);
        }
        Some(SessionMap::Multi(client)) => {
            if let Some(session_id) = session_id.as_ref() {
                client.remove(session_id);
            } else {
                error!(
                    "[Notification Service Error] - Session Id not Found for Multi Session Client"
                );
            }
        }
        None => warn!("[Notification Service Error] - ClientId not found in the shard"),
    }

    measure_latency_duration!(
        "handle_client_disconnection_or_failure_clients_tx_write",
        handle_client_disconnection_or_failure_clients_tx_write_start_time
    );
}

#[macros::measure_duration]
async fn client_reciever(
    redis_pool: Arc<RedisConnectionPool>,
    client_id: ClientId,
    client_req: SenderType,
    clients_tx: Arc<Vec<MonitoredRwLock<ReaderMap>>>,
    max_shards: u64,
) {
    let shard = Shard((hash_uuid(&client_id.inner()) % max_shards as u128) as u64);
    match client_req {
        SenderType::ClientConnection((session_id, client_tx)) => {
            info!("[Client Connected] : {:?}", client_id);
            CONNECTED_CLIENTS.inc();

            let client_reciever_clients_tx_write_start_time = tokio::time::Instant::now();
            let active_notification = Arc::new(MonitoredRwLock::new(
                ActiveNotification::new(&redis_pool, &client_id, &shard).await,
            ));

            if let Some(session_id) = session_id {
                let mut client = clients_tx
                    .get(shard.inner() as usize)
                    .expect("This error is impossible!")
                    .write(RwLockName::ClientTxStore, RwLockOperation::Write)
                    .await;

                if let Some(SessionMap::Multi(client)) = client.get_mut(&client_id) {
                    client.insert(session_id, (client_tx, active_notification));
                } else {
                    let mut client_session = FxHashMap::default();
                    client_session.insert(session_id, (client_tx, active_notification));
                    client.insert(client_id, SessionMap::Multi(client_session));
                }
            } else {
                clients_tx
                    .get(shard.inner() as usize)
                    .expect("This error is impossible!")
                    .write(RwLockName::ClientTxStore, RwLockOperation::Write)
                    .await
                    .insert(
                        client_id,
                        SessionMap::Single((client_tx, active_notification)),
                    );
            };

            measure_latency_duration!(
                "client_reciever_clients_tx_write",
                client_reciever_clients_tx_write_start_time
            );
        }
        SenderType::ClientDisconnection(session_id) => {
            warn!("[Client Disconnected] : {:?} : {:?}", client_id, session_id);
            CONNECTED_CLIENTS.dec();
            handle_client_disconnection_or_failure(
                clients_tx.clone(),
                &shard,
                &client_id,
                &session_id,
            )
            .await;
        }
        SenderType::ClientAck((notification_id, session_id)) => {
            if let Some(session_id) = session_id {
                if let Some(SessionMap::Multi(client)) = clients_tx
                    .get(shard.inner() as usize)
                    .expect("This error is impossible!")
                    .write(RwLockName::ClientTxStore, RwLockOperation::Write)
                    .await
                    .get_mut(&client_id)
                {
                    if let Some((_, active_notification)) = client.get_mut(&session_id) {
                        active_notification
                            .write(RwLockName::ActiveNotificationStore, RwLockOperation::Write)
                            .await
                            .acknowledge(&notification_id);
                    } else {
                        error!("[Notification Service Error] - SessionId not found in the Client Shard");
                    }
                } else {
                    error!("[Notification Service Error] - Multi Client Session not found for the Client");
                }
            } else if let Some(SessionMap::Single((_, active_notification))) = clients_tx
                .get(shard.inner() as usize)
                .expect("This error is impossible!")
                .write(RwLockName::ClientTxStore, RwLockOperation::Write)
                .await
                .get_mut(&client_id)
            {
                active_notification
                    .write(RwLockName::ActiveNotificationStore, RwLockOperation::Write)
                    .await
                    .acknowledge(&notification_id);
            } else {
                error!("[Notification Service Error] - ClientId not found in the Shard");
            };
        }
    }
}

async fn client_reciever_looper(
    redis_pool: Arc<RedisConnectionPool>,
    mut read_notification_rx: UnboundedReceiver<(ClientId, SenderType, DateTime<Utc>)>,
    clients_tx: Arc<Vec<MonitoredRwLock<ReaderMap>>>,
    max_shards: u64,
) {
    while let Some((client_id, client_tx, sent_at)) = read_notification_rx.recv().await {
        channel_delay!(sent_at, &client_tx.to_string());

        client_reciever(
            redis_pool.clone(),
            client_id,
            client_tx,
            clients_tx.clone(),
            max_shards,
        )
        .await;
    }
    error!("[Notification Service Error] - read_notification_rx closed");
}

#[macros::measure_duration]
async fn retry_notifications(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<MonitoredRwLock<ReaderMap>>>,
    read_all_connected_client_notifications: bool,
) {
    let read_client_notifications_batch_task: Vec<_> = clients_tx
        .iter()
        .enumerate()
        .map(|(shard, clients)| {
            let shard = Shard(shard as u64);
            let (redis_pool_clone, clients_tx_clone) = (redis_pool.clone(), clients_tx.clone());
            async move {
                let client_ids = clients
                    .read(RwLockName::ClientTxStore,RwLockOperation::Read)
                    .await
                    .iter()
                    .filter_map(|(client_id, client)| {
                       let read_all = read_all_connected_client_notifications;
                       if read_all {
                            return Some(client_id.to_owned());
                        }
                        match client {
                            SessionMap::Single((_, active_notification)) => {
                                if let Ok(active_notification) = active_notification.try_read(RwLockName::ActiveNotificationStore,RwLockOperation::TryRead) {
                                    if active_notification.count() > 0 {
                                        return Some(client_id.to_owned())
                                    }
                                }
                                None
                            }
                            SessionMap::Multi(client) => {
                                let has_active = client.iter().any(|(_, (_, active_notification))| {
                                    if let Ok(active_notification) = active_notification.try_read(RwLockName::ActiveNotificationStore,RwLockOperation::TryRead) {
                                        if active_notification.count() > 0 {
                                            return true;
                                        }
                                    }
                                    false
                                });

                                if has_active {
                                    Some(client_id.to_owned())
                                } else {
                                    None
                                }
                            }
                        }
                    })
                    .collect();

                let notifications = read_client_notifications(&redis_pool_clone, client_ids, &shard).await;

                match notifications {
                    Ok(notifications) => {
                        for (client_id, notifications) in notifications.into_iter() {

                            if notifications.is_empty() {
                                 match clients_tx_clone
                                    .get(shard.inner() as usize)
                                    .expect("This error is impossible")
                                    .read(RwLockName::ClientTxStore,RwLockOperation::Read)
                                    .await
                                    .get(&client_id) {
                                        Some(SessionMap::Single((_, active_notification))) => {
                                            if let Ok(active_notification) = active_notification.try_write(RwLockName::ActiveNotificationStore,RwLockOperation::TryWrite) {
                                                active_notification.refresh()
                                            }
                                        },
                                        Some(SessionMap::Multi(client)) => {
                                            for (_, (_, active_notification)) in client.iter() {
                                                if let Ok(active_notification) = active_notification.try_write(RwLockName::ActiveNotificationStore,RwLockOperation::TryWrite) {
                                                    active_notification.refresh()
                                                }
                                            }
                                        }
                                        None => {
                                            warn!(
                                                "Client ({:?}) entry does not exist, client got disconnected intermittently.",
                                                client_id
                                            );
                                        }
                                    };
                                continue;
                            }

                            for notification in notifications {
                                if read_all_connected_client_notifications {
                                    TOTAL_NOTIFICATIONS.inc();
                                } else {
                                    RETRIED_NOTIFICATIONS.inc();
                                }

                                if notification.ttl.inner() < Utc::now() {
                                    clear_expired_notification(
                                        &Arc::clone(&redis_pool_clone),
                                        &shard.clone(),
                                        &client_id.inner(),
                                        &notification.stream_id.clone(),
                                    )
                                    .await;
                                } else {
                                    let read_and_process_notification_clients_tx_read_start_time =
                                        tokio::time::Instant::now();

                                    let clients_tx =
                                        match clients_tx_clone
                                            .get(shard.inner() as usize)
                                            .expect("This error is impossible")
                                            .read(RwLockName::ClientTxStore,RwLockOperation::Read)
                                            .await
                                            .get(&client_id) {
                                                Some(SessionMap::Single((client_tx, _))) => {
                                                    vec![client_tx.clone()]
                                                },
                                                Some(SessionMap::Multi(client)) => {
                                                    client.iter().map(|(_, (client_tx, _))| client_tx.clone()).collect()
                                                }
                                                None => {
                                                    warn!(
                                                        "Client ({:?}) entry does not exist, client got disconnected intermittently.",
                                                        client_id
                                                    );
                                                    vec![]
                                                }
                                            };

                                    measure_latency_duration!(
                                        "read_and_process_notification_clients_tx_read",
                                        read_and_process_notification_clients_tx_read_start_time
                                    );

                                    for client_tx in clients_tx {
                                        if let Err(err) = send_notification(
                                            &client_id,
                                            &client_tx,
                                            notification.to_owned(),
                                            &redis_pool_clone,
                                            &shard,
                                        )
                                        .await
                                        {
                                            warn!("[Send Failed] : {}", err);
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Err(err) => {
                        error!("[Notification Service Error] - read_client_notifications : {}", err);
                    }
                }
            }
        })
        .collect();

    join_all(read_client_notifications_batch_task).await;
}

async fn retry_notifications_looper(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<MonitoredRwLock<ReaderMap>>>,
    read_all_connected_client_notifications: bool,
    delay: Duration,
) {
    loop {
        retry_notifications(
            redis_pool.clone(),
            clients_tx.clone(),
            read_all_connected_client_notifications,
        )
        .await;
        sleep(delay).await;
    }
}

async fn active_notification(
    redis_pool: &RedisConnectionPool,
    clients_tx: Arc<Vec<MonitoredRwLock<ReaderMap>>>,
    active_notification_receiver_stream: &mut UnboundedReceiver<(String, String, DateTime<Utc>)>,
    max_shards: u64,
) {
    while let Some((_, client_id, sent_at)) = active_notification_receiver_stream.recv().await {
        channel_delay!(sent_at, "active_notification");

        let client_id = ClientId(client_id);
        let shard = Shard((hash_uuid(&client_id.inner()) % max_shards as u128) as u64);

        let all_clients_tx = match clients_tx
            .get(shard.inner() as usize)
            .expect("This error is impossible")
            .read(RwLockName::ClientTxStore, RwLockOperation::Read)
            .await
            .get(&client_id)
        {
            Some(SessionMap::Single((client_tx, _))) => {
                vec![client_tx.clone()]
            }
            Some(SessionMap::Multi(client)) => client
                .iter()
                .map(|(_, (client_tx, _))| client_tx.clone())
                .collect(),
            None => {
                warn!(
                    "Client ({:?}) entry does not exist, client got disconnected intermittently.",
                    client_id
                );
                vec![]
            }
        };

        if !all_clients_tx.is_empty() {
            TOTAL_NOTIFICATIONS.inc();

            if let Ok(notifications) =
                read_client_notification(redis_pool, &client_id, &shard).await
            {
                match clients_tx
                    .get(shard.inner() as usize)
                    .expect("This error is impossible")
                    .write(RwLockName::ClientTxStore, RwLockOperation::Write)
                    .await
                    .get_mut(&client_id)
                {
                    Some(SessionMap::Single((_, active_notification))) => {
                        active_notification
                            .write(RwLockName::ActiveNotificationStore, RwLockOperation::Write)
                            .await
                            .update(notifications.to_owned());
                    }
                    Some(SessionMap::Multi(client)) => {
                        stream::iter(client.iter_mut())
                            .for_each_concurrent(None, |(_, (_, active_notification))| async {
                                active_notification
                                    .write(
                                        RwLockName::ActiveNotificationStore,
                                        RwLockOperation::Write,
                                    )
                                    .await
                                    .update(notifications.to_owned());
                            })
                            .await;
                    }
                    None => error!(
                        "[Notification Service Error] - ClientId {:?} not found here.",
                        client_id
                    ),
                }

                for notification in notifications {
                    if notification.ttl.inner() < Utc::now() {
                        clear_expired_notification(
                            redis_pool,
                            &shard,
                            &client_id.inner(),
                            &notification.stream_id.clone(),
                        )
                        .await;
                    } else {
                        for client_tx in all_clients_tx.clone() {
                            if let Err(err) = send_notification(
                                &client_id,
                                &client_tx,
                                notification.to_owned(),
                                redis_pool,
                                &shard,
                            )
                            .await
                            {
                                warn!("[Send Failed] : {}", err);
                            }
                        }
                    }
                }
            }
        } else {
            warn!("[Notification Service] - Client Not Connected to this Server")
        }
    }
    error!("[Notification Service Error] - Issue found in the active notification receiver stream.")
}

async fn active_notification_looper(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<Vec<MonitoredRwLock<ReaderMap>>>,
    max_shards: u64,
) {
    let pubsub_channel_key = pubsub_channel_key();
    if let Ok(mut active_notification_receiver_stream) = redis_pool
        .subscribe_channel::<String>(pubsub_channel_key)
        .await
    {
        active_notification(
            &redis_pool,
            clients_tx.clone(),
            &mut active_notification_receiver_stream,
            max_shards,
        )
        .await;
    } else {
        error!("[Notification Service Error] - Unable to Subscribe to Channel")
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_notification_reader(
    read_notification_rx: UnboundedReceiver<(ClientId, SenderType, DateTime<Utc>)>,
    graceful_termination_signal_rx: sync::oneshot::Receiver<()>,
    redis_pool: Arc<RedisConnectionPool>,
    retry_delay_millis: u64,
    max_shards: u64,
    read_all_connected_client_notifications: bool,
) {
    let clients_tx: Arc<Vec<MonitoredRwLock<ReaderMap>>> = Arc::new(
        (0..max_shards)
            .map(|_| MonitoredRwLock::new(FxHashMap::default()))
            .collect(),
    );

    let rx_task = tokio::spawn(client_reciever_looper(
        redis_pool.clone(),
        read_notification_rx,
        clients_tx.clone(),
        max_shards,
    ));

    let retry_notifications_task = tokio::spawn(retry_notifications_looper(
        redis_pool.clone(),
        clients_tx.clone(),
        read_all_connected_client_notifications,
        Duration::from_millis(retry_delay_millis),
    ));

    if read_all_connected_client_notifications {
        tokio::select!(
            res = rx_task => {
                error!("[Notification Service Error] - [CLIENT_RECIEVER_TASK] : {:?}", res);
            },
            res = retry_notifications_task => {
                error!("[Notification Service Error] - [RETRY_NOTIFICATION_TASK] : {:?}", res);
            },
            _ = graceful_termination_signal_rx => {
                error!("[Notification Service Error] - [GRACEFUL_SHUT_DOWN]");
            }
        );
    } else {
        let active_notifications = tokio::spawn(active_notification_looper(
            redis_pool.clone(),
            clients_tx.clone(),
            max_shards,
        ));

        tokio::select!(
            res = rx_task => {
                error!("[Notification Service Error] - [CLIENT_RECIEVER_TASK] : {:?}", res);
            },
            res = active_notifications => {
                error!("[Notification Service Error] - [ACTIVE_NOTIFICATION_TASK] : {:?}", res);
            },
            res = retry_notifications_task => {
                error!("[Notification Service Error] - [RETRY_NOTIFICATION_TASK] : {:?}", res);
            },
            _ = graceful_termination_signal_rx => {
                error!("[Notification Service Error] - [GRACEFUL_SHUT_DOWN]");
            }
        );
    }
}
