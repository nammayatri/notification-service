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
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use shared::measure_latency_duration;
use shared::redis::types::RedisConnectionPool;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{self, mpsc::Receiver},
    time::sleep,
};
use tracing::*;

#[macros::measure_duration]
async fn client_tx_send(client_tx: &ClientTx, notification: &NotificationData) -> Result<()> {
    client_tx
        .send(Ok(transform_notification_data_to_payload(
            notification.clone(),
        )))
        .await?;
    Ok(())
}

#[macros::measure_duration]
async fn send_notification(
    client_tx: &ClientTx,
    notification: NotificationData,
    redis_pool: &RedisConnectionPool,
    source: &'static str,
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

    client_tx_send(client_tx, &notification).await?;

    notification_latency!(
        get_timestamp_from_stream_id(&notification.stream_id.inner()).inner(),
        "NACK",
        source
    );

    Ok(())
}

#[macros::measure_duration]
async fn clear_expired_notification(
    redis_pool: &RedisConnectionPool,
    shard: &Shard,
    client_id: &str,
    notification_stream_id: &StreamEntry,
) {
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
    clients_tx: Arc<ReaderMap>,
    client_id: &ClientId,
    session_id: &Option<SessionID>,
) {
    let start = tokio::time::Instant::now();

    // For Single sessions, remove the whole entry. For Multi, remove the named
    // session and drop the entry if no sessions remain. Hold the mutable bucket
    // guard for the smallest possible window — no `.await` while held.
    let should_remove = if let Some(mut entry) = clients_tx.get_mut(client_id) {
        match &mut entry.value_mut().sessions {
            SessionMap::Single(_) => true,
            SessionMap::Multi(sessions) => {
                if let Some(session_id) = session_id.as_ref() {
                    sessions.remove(session_id);
                } else {
                    error!(
                        "[Notification Service Error] - Session Id not Found for Multi Session Client"
                    );
                }
                sessions.is_empty()
            }
        }
    } else {
        warn!("[Notification Service Error] - ClientId not found");
        false
    };

    if should_remove {
        clients_tx.remove(client_id);
    }

    measure_latency_duration!(
        "handle_client_disconnection_or_failure_clients_tx_write",
        start
    );
}

#[macros::measure_duration]
async fn client_reciever(
    redis_pool: Arc<RedisConnectionPool>,
    client_id: ClientId,
    client_req: SenderType,
    clients_tx: Arc<ReaderMap>,
    max_shards: u64,
    read_all_connected_client_notifications: bool,
) {
    match client_req {
        SenderType::ClientConnection((session_id, client_tx)) => {
            info!("[Client Connected] : {:?}", client_id);
            CONNECTED_CLIENTS.inc();

            // Compute shard ONCE at connect time. Every later op reads
            // `entry.shard` instead of recomputing `hash_uuid(client_id) % max_shards`.
            // This is the only place in the code (other than pubsub message decode)
            // that calls hash_uuid for an in-memory routing decision.
            let shard = Shard((hash_uuid(&client_id.inner()) % max_shards as u128) as u64);

            let start = tokio::time::Instant::now();

            let active_notification = Arc::new(Mutex::new(ActiveNotification::default()));
            let client_tx_for_catchup = client_tx.clone();

            // Hold the DashMap bucket guard for a tiny synchronous critical section.
            // No `.await` inside this block — that's the DashMap deadlock invariant.
            {
                let mut entry =
                    clients_tx
                        .entry(client_id.clone())
                        .or_insert_with(|| ClientEntry {
                            shard: shard.clone(),
                            sessions: if session_id.is_some() {
                                SessionMap::Multi(FxHashMap::default())
                            } else {
                                // Placeholder; will be overwritten just below.
                                SessionMap::Single((client_tx.clone(), active_notification.clone()))
                            },
                        });

                match (&mut entry.sessions, session_id) {
                    (SessionMap::Multi(sessions), Some(session_id)) => {
                        sessions.insert(session_id, (client_tx, active_notification));
                    }
                    (sessions @ SessionMap::Multi(_), None) => {
                        // Existed as Multi but the new connection is single-session.
                        *sessions = SessionMap::Single((client_tx, active_notification));
                    }
                    (sessions @ SessionMap::Single(_), Some(session_id)) => {
                        // Existed as Single but a multi-session connection arrived;
                        // promote to Multi and keep the new session.
                        let mut map = FxHashMap::default();
                        map.insert(session_id, (client_tx, active_notification));
                        *sessions = SessionMap::Multi(map);
                    }
                    (sessions @ SessionMap::Single(_), None) => {
                        *sessions = SessionMap::Single((client_tx, active_notification));
                    }
                }
            }

            measure_latency_duration!("client_reciever_clients_tx_write", start);

            if !read_all_connected_client_notifications {
                let redis_pool = redis_pool.clone();
                let clients_tx = clients_tx.clone();
                tokio::spawn(async move {
                    dispatch_and_send_notifications(
                        &redis_pool,
                        &clients_tx,
                        &client_id,
                        &shard,
                        &[client_tx_for_catchup],
                        "catchup",
                    )
                    .await;
                });
            }
        }
        SenderType::ClientDisconnection(session_id) => {
            warn!("[Client Disconnected] : {:?} : {:?}", client_id, session_id);
            CONNECTED_CLIENTS.dec();
            handle_client_disconnection_or_failure(clients_tx.clone(), &client_id, &session_id)
                .await;
        }
    }
}

async fn client_reciever_looper(
    redis_pool: Arc<RedisConnectionPool>,
    mut read_notification_rx: Receiver<(ClientId, SenderType, DateTime<Utc>)>,
    clients_tx: Arc<ReaderMap>,
    max_shards: u64,
    read_all_connected_client_notifications: bool,
) {
    while let Some((client_id, client_tx, sent_at)) = read_notification_rx.recv().await {
        channel_delay!(sent_at, &client_tx.to_string());

        client_reciever(
            redis_pool.clone(),
            client_id,
            client_tx,
            clients_tx.clone(),
            max_shards,
            read_all_connected_client_notifications,
        )
        .await;
    }
    error!("[Notification Service Error] - read_notification_rx closed");
}

// Retry-sweep concurrency budget.
//
// `retry_notifications` fans out at three nesting levels — clients within a
// shard, notifications within a client, sessions within a notification — each
// capped by one of the constants below. Worst-case in-flight work per shard is
// the product: 256 * 32 * 8 = 65,536 concurrent ops. The caps exist so a hot
// shard with e.g. 20k clients * 50 notifications * 5 sessions doesn't spawn
// ~5M tasks at once.
const RETRY_PER_CLIENT_CONCURRENCY: usize = 256;
const RETRY_PER_NOTIFICATION_CONCURRENCY: usize = 32;
const RETRY_PER_TARGET_CONCURRENCY: usize = 8;

/// Snapshot a client's `Arc<Mutex<ActiveNotification>>`s without holding a
/// DashMap guard across an `.await`. Cloning the `Arc` is cheap; the caller
/// then locks the mutex synchronously when it needs to mutate.
fn snapshot_active_notifications(
    clients_tx: &Arc<ReaderMap>,
    client_id: &ClientId,
) -> Vec<Arc<Mutex<ActiveNotification>>> {
    match clients_tx.get(client_id) {
        Some(entry) => match &entry.value().sessions {
            SessionMap::Single((_, active)) => vec![active.clone()],
            SessionMap::Multi(client) => client.values().map(|(_, a)| a.clone()).collect(),
        },
        None => vec![],
    }
}

/// Like `snapshot_active_notifications` but also returns target client_txs.
fn snapshot_client(
    clients_tx: &Arc<ReaderMap>,
    client_id: &ClientId,
) -> (Option<Arc<Mutex<ActiveNotification>>>, Vec<ClientTx>) {
    match clients_tx.get(client_id) {
        Some(entry) => match &entry.value().sessions {
            SessionMap::Single((client_tx, active)) => {
                (Some(active.clone()), vec![client_tx.clone()])
            }
            SessionMap::Multi(client) => {
                let active = client.values().next().map(|(_, a)| a.clone());
                let txs = client.values().map(|(tx, _)| tx.clone()).collect();
                (active, txs)
            }
        },
        None => {
            warn!(
                "Client ({:?}) entry does not exist, client got disconnected intermittently.",
                client_id
            );
            (None, vec![])
        }
    }
}

async fn refresh_active_for_empty_client(clients_tx: &Arc<ReaderMap>, client_id: &ClientId) {
    let actives = snapshot_active_notifications(clients_tx, client_id);
    if actives.is_empty() {
        warn!(
            "Client ({:?}) entry does not exist, client got disconnected intermittently.",
            client_id
        );
        return;
    }
    for active in actives {
        active.lock().refresh();
    }
}

async fn process_notification(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    shard: Shard,
    client_id: ClientId,
    notification: NotificationData,
) {
    let (active, target_client_txs) = snapshot_client(&clients_tx, &client_id);

    let (count_total, count_retry, count_expired) = if let Some(active) = active.as_ref() {
        let mut guard = active.lock();
        let ct = guard.try_claim_total(&notification);
        let cr = if !ct {
            guard.try_claim_retry(&notification.id)
        } else {
            false
        };
        let ce = if notification.ttl.inner() < Utc::now() {
            guard.try_claim_expired(&notification.id)
        } else {
            false
        };
        (ct, cr, ce)
    } else {
        (false, false, false)
    };

    if count_total {
        TOTAL_NOTIFICATIONS
            .with_label_values(&[&notification.category])
            .inc();
    }
    if count_retry {
        RETRIED_NOTIFICATIONS
            .with_label_values(&[&notification.category])
            .inc();
    }

    if notification.ttl.inner() < Utc::now() {
        if count_expired {
            EXPIRED_NOTIFICATIONS
                .with_label_values(&[&notification.category])
                .inc();
        }
        clear_expired_notification(
            &redis_pool,
            &shard,
            &client_id.inner(),
            &notification.stream_id,
        )
        .await;
        return;
    }

    if target_client_txs.is_empty() {
        return;
    }

    let notification = Arc::new(notification);
    stream::iter(target_client_txs.into_iter())
        .for_each_concurrent(RETRY_PER_TARGET_CONCURRENCY, |client_tx| {
            let redis_pool = redis_pool.clone();
            let notification = notification.clone();
            async move {
                if let Err(err) =
                    send_notification(&client_tx, (*notification).clone(), &redis_pool, "retry")
                        .await
                {
                    warn!("[Send Failed] : {}", err);
                }
            }
        })
        .await;
}

async fn process_client_in_retry(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    shard: Shard,
    client_id: ClientId,
    notifications: Vec<NotificationData>,
) {
    if notifications.is_empty() {
        refresh_active_for_empty_client(&clients_tx, &client_id).await;
        return;
    }

    stream::iter(notifications.into_iter())
        .for_each_concurrent(RETRY_PER_NOTIFICATION_CONCURRENCY, |notification| {
            let redis_pool = redis_pool.clone();
            let clients_tx = clients_tx.clone();
            let shard = shard.clone();
            let client_id = client_id.clone();
            async move {
                process_notification(redis_pool, clients_tx, shard, client_id, notification).await;
            }
        })
        .await;
}

#[macros::measure_duration]
async fn retry_notifications(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    max_shards: u64,
    read_all_connected_client_notifications: bool,
) {
    // Step 1: one pass over the DashMap, NO `.await` held, grouping client_ids
    // by their cached Redis shard. This preserves today's 128-batched-XREAD
    // pattern: read_client_notifications is invoked once per shard, carrying
    // up to N/max_shards client stream keys per call.
    let mut by_shard: Vec<Vec<ClientId>> = (0..max_shards as usize).map(|_| Vec::new()).collect();

    for entry in clients_tx.iter() {
        let client_id = entry.key();
        let shard_idx = entry.value().shard.inner() as usize;

        if read_all_connected_client_notifications {
            by_shard[shard_idx].push(client_id.clone());
            continue;
        }

        // Event-driven mode: only sweep clients whose ActiveNotification map
        // has entries (i.e., notifications that haven't been ACKed yet).
        // `try_lock` because we don't want a sweep to block on a writer.
        let has_active = match &entry.value().sessions {
            SessionMap::Single((_, active)) => {
                active.try_lock().map(|g| g.count() > 0).unwrap_or(false)
            }
            SessionMap::Multi(client) => client
                .values()
                .any(|(_, active)| active.try_lock().map(|g| g.count() > 0).unwrap_or(false)),
        };
        if has_active {
            by_shard[shard_idx].push(client_id.clone());
        }
    }
    // All DashMap guards dropped here, before any `.await`.

    // Step 2: same 128-task `join_all` as today, each issuing one
    // read_client_notifications call with that shard's client_ids.
    let tasks: Vec<_> = by_shard
        .into_iter()
        .enumerate()
        .map(|(shard_idx, client_ids)| {
            let shard = Shard(shard_idx as u64);
            let redis_pool = redis_pool.clone();
            let clients_tx = clients_tx.clone();
            async move {
                if client_ids.is_empty() {
                    return;
                }
                let notifications =
                    read_client_notifications(&redis_pool, client_ids, &shard).await;

                match notifications {
                    Ok(notifications) => {
                        stream::iter(notifications.into_iter())
                            .for_each_concurrent(
                                RETRY_PER_CLIENT_CONCURRENCY,
                                |(client_id, notifs)| {
                                    let redis_pool = redis_pool.clone();
                                    let clients_tx = clients_tx.clone();
                                    let shard = shard.clone();
                                    async move {
                                        process_client_in_retry(
                                            redis_pool, clients_tx, shard, client_id, notifs,
                                        )
                                        .await;
                                    }
                                },
                            )
                            .await;
                    }
                    Err(err) => {
                        error!(
                            "[Notification Service Error] - read_client_notifications : {}",
                            err
                        );
                    }
                }
            }
        })
        .collect();

    join_all(tasks).await;
}

async fn retry_notifications_looper(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    max_shards: u64,
    read_all_connected_client_notifications: bool,
    delay: Duration,
) {
    loop {
        retry_notifications(
            redis_pool.clone(),
            clients_tx.clone(),
            max_shards,
            read_all_connected_client_notifications,
        )
        .await;
        sleep(delay).await;
    }
}

#[macros::measure_duration]
async fn active_notification_dispatch(
    redis_pool: &RedisConnectionPool,
    clients_tx: &Arc<ReaderMap>,
    client_id: &ClientId,
    shard: &Shard,
) -> Option<(Vec<NotificationData>, Arc<Mutex<ActiveNotification>>)> {
    let notifications = read_client_notification(redis_pool, client_id, shard)
        .await
        .ok()?;

    // Snapshot all session-actives, then mutate them with the sync mutex.
    // No DashMap guard held across `.await`.
    let (primary_active, all_actives) = match clients_tx.get(client_id) {
        Some(entry) => match &entry.value().sessions {
            SessionMap::Single((_, active)) => (Some(active.clone()), vec![active.clone()]),
            SessionMap::Multi(client) => {
                let actives: Vec<_> = client.values().map(|(_, a)| a.clone()).collect();
                let primary = actives.first().cloned();
                (primary, actives)
            }
        },
        None => {
            error!(
                "[Notification Service Error] - ClientId {:?} not found here.",
                client_id
            );
            return None;
        }
    };

    for active in all_actives {
        active.lock().update(notifications.to_owned());
    }

    primary_active.map(|active| (notifications, active))
}

async fn dispatch_and_send_notifications(
    redis_pool: &RedisConnectionPool,
    clients_tx: &Arc<ReaderMap>,
    client_id: &ClientId,
    shard: &Shard,
    target_client_txs: &[ClientTx],
    source: &'static str,
) {
    let Some((notifications, active)) =
        active_notification_dispatch(redis_pool, clients_tx, client_id, shard).await
    else {
        return;
    };

    for notification in notifications {
        let (count_total, count_expired) = {
            let mut guard = active.lock();
            let ct = guard.try_claim_total(&notification);
            let ce = if notification.ttl.inner() < Utc::now() {
                guard.try_claim_expired(&notification.id)
            } else {
                false
            };
            (ct, ce)
        };

        if count_total {
            TOTAL_NOTIFICATIONS
                .with_label_values(&[&notification.category])
                .inc();
        }

        if notification.ttl.inner() < Utc::now() {
            if count_expired {
                EXPIRED_NOTIFICATIONS
                    .with_label_values(&[&notification.category])
                    .inc();
            }
            clear_expired_notification(
                redis_pool,
                shard,
                &client_id.inner(),
                &notification.stream_id,
            )
            .await;
        } else {
            for client_tx in target_client_txs {
                if let Err(err) =
                    send_notification(client_tx, notification.to_owned(), redis_pool, source).await
                {
                    warn!("[Send Failed] : {}", err);
                }
            }
        }
    }
}

async fn active_notification(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    active_notification_receiver_stream: &mut tokio::sync::mpsc::UnboundedReceiver<(
        String,
        NotificationMessage,
        DateTime<Utc>,
    )>,
) {
    while let Some((_, message, sent_at)) = active_notification_receiver_stream.recv().await {
        let NotificationMessage {
            stream_id,
            timestamp,
        } = message;
        channel_delay!(timestamp, "active_notification_pubsub_delay");
        channel_delay!(sent_at, "active_notification");

        let client_id = ClientId(stream_id);

        // Read the cached shard from the DashMap entry instead of recomputing
        // hash_uuid + modulo. Snapshot client_txs in the same lookup.
        let (shard_opt, all_clients_tx) = match clients_tx.get(&client_id) {
            Some(entry) => {
                let shard = entry.value().shard.clone();
                let txs = match &entry.value().sessions {
                    SessionMap::Single((client_tx, _)) => vec![client_tx.clone()],
                    SessionMap::Multi(client) => {
                        client.values().map(|(tx, _)| tx.clone()).collect()
                    }
                };
                (Some(shard), txs)
            }
            None => (None, vec![]),
        };

        let Some(shard) = shard_opt else {
            warn!(
                "Client ({:?}) entry does not exist, client got disconnected intermittently.",
                client_id
            );
            continue;
        };

        if !all_clients_tx.is_empty() {
            let redis_pool = redis_pool.clone();
            let clients_tx = clients_tx.clone();
            dispatch_and_send_notifications(
                &redis_pool,
                &clients_tx,
                &client_id,
                &shard,
                &all_clients_tx,
                "pubsub",
            )
            .await;
        } else {
            warn!("[Notification Service] - Client Not Connected to this Server")
        }
    }
    error!("[Notification Service Error] - Issue found in the active notification receiver stream.")
}

async fn active_notification_looper(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    max_shards: u64,
) {
    let pubsub_channel_key = pubsub_channel_key();
    loop {
        match redis_pool
            .subscribe_channel::<NotificationMessage>(pubsub_channel_key)
            .await
        {
            Ok(mut active_notification_receiver_stream) => {
                info!(
                    "[Notification Service] - Subscribed to pubsub channel {}",
                    pubsub_channel_key
                );
                active_notification(
                    redis_pool.clone(),
                    clients_tx.clone(),
                    &mut active_notification_receiver_stream,
                )
                .await;
                error!(
                    "[Notification Service Error] - Pubsub subscription dropped, sweeping all connected clients before re-subscribing"
                );
            }
            Err(err) => {
                error!(
                    "[Notification Service Error] - Unable to Subscribe to Channel: {:?}",
                    err
                );
            }
        }

        retry_notifications(redis_pool.clone(), clients_tx.clone(), max_shards, true).await;

        sleep(Duration::from_secs(1)).await;
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_notification_reader(
    read_notification_rx: Receiver<(ClientId, SenderType, DateTime<Utc>)>,
    graceful_termination_signal_rx: sync::oneshot::Receiver<()>,
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    retry_delay_millis: u64,
    max_shards: u64,
    read_all_connected_client_notifications: bool,
) {
    let rx_task = tokio::spawn(client_reciever_looper(
        redis_pool.clone(),
        read_notification_rx,
        clients_tx.clone(),
        max_shards,
        read_all_connected_client_notifications,
    ));

    let retry_notifications_task = tokio::spawn(retry_notifications_looper(
        redis_pool.clone(),
        clients_tx.clone(),
        max_shards,
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
