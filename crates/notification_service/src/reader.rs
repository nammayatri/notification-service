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
            abs_diff_utc_as_sec, get_timestamp_from_stream_id, hash_uuid, max_stream_id,
            transform_notification_data_to_payload,
        },
    },
    notification_latency,
    redis::{
        commands::{
            clean_up_notifications_batch, read_client_notification, read_client_notifications,
        },
        keys::*,
        types::NotificationData,
    },
    tools::prometheus::{
        CHANNEL_DELAY, CLEANUP_PUSH_SKIPPED, CONNECTED_CLIENTS, EXPIRED_NOTIFICATIONS,
        MEASURE_DURATION, NOTIFICATION_LATENCY, RETRIED_NOTIFICATIONS, TOTAL_NOTIFICATIONS,
    },
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use futures::{future::join_all, stream, StreamExt};
use parking_lot::Mutex;
use rustc_hash::{FxHashMap, FxHashSet};
use shared::measure_latency_duration;
use shared::redis::types::RedisConnectionPool;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{self, mpsc::Receiver},
    time::sleep,
};
use tracing::*;

pub fn new_expired_queue() -> ExpiredQueue {
    Arc::new(DashMap::with_hasher(FxBuildHasher::default()))
}

fn try_push_expired(
    expired_queue: &ExpiredQueue,
    client_id: &ClientId,
    shard: u64,
    stream_id: String,
    origin: &'static str,
) {
    if let Some(entry) = expired_queue.get(client_id) {
        match entry.value().stream_ids.try_lock() {
            Some(mut guard) => {
                guard.insert(stream_id);
                return;
            }
            None => {
                CLEANUP_PUSH_SKIPPED.with_label_values(&[origin]).inc();
                return;
            }
        }
    }

    let mut set = FxHashSet::default();
    set.insert(stream_id);
    let _ = expired_queue.insert(
        client_id.clone(),
        ExpiredEntry {
            shard,
            stream_ids: Mutex::new(set),
        },
    );
}

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
    source: &'static str,
) -> Result<()> {
    client_tx_send(client_tx, &notification).await?;

    notification_latency!(
        get_timestamp_from_stream_id(&notification.stream_id.inner()).inner(),
        "NACK",
        source
    );

    Ok(())
}

async fn expire_notifications_looper(
    redis_pool: Arc<RedisConnectionPool>,
    expired_queue: ExpiredQueue,
    delay: Duration,
) {
    loop {
        sleep(delay).await;
        flush_expired_queue(&redis_pool, &expired_queue).await;
    }
}

#[macros::measure_duration]
async fn flush_expired_queue(redis_pool: &Arc<RedisConnectionPool>, expired_queue: &ExpiredQueue) {
    let mut by_shard: FxHashMap<u64, Vec<(String, Vec<String>)>> = FxHashMap::default();

    let keys: Vec<ClientId> = expired_queue.iter().map(|e| e.key().clone()).collect();

    for client_id in keys {
        let (shard, ids) = match expired_queue.get(&client_id) {
            Some(entry) => {
                let shard = entry.value().shard;
                let mut guard = entry.value().stream_ids.lock();
                if guard.is_empty() {
                    continue;
                }
                let ids: Vec<String> = guard.drain().collect();
                (shard, ids)
            }
            None => continue,
        };
        expired_queue.remove_if(&client_id, |_, e| e.stream_ids.lock().is_empty());

        by_shard
            .entry(shard)
            .or_default()
            .push((client_id.inner(), ids));
    }

    for (shard, clients) in by_shard {
        if let Err(e) = clean_up_notifications_batch(redis_pool, shard, clients).await {
            error!(
                "[Notification Service Error] - flush_expired_queue shard={} : {}",
                shard, e
            );
        }
    }
}

#[macros::measure_duration]
async fn handle_client_disconnection_or_failure(
    clients_tx: Arc<ReaderMap>,
    client_id: &ClientId,
    session_id: &Option<SessionID>,
) {
    let start = tokio::time::Instant::now();

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
    expired_queue: ExpiredQueue,
    max_shards: u64,
    read_all_connected_client_notifications: bool,
) {
    match client_req {
        SenderType::ClientConnection((session_id, client_tx)) => {
            info!("[Client Connected] : {:?}", client_id);
            CONNECTED_CLIENTS.inc();

            let shard = Shard((hash_uuid(&client_id.inner()) % max_shards as u128) as u64);

            let start = tokio::time::Instant::now();

            let active_notification = Arc::new(Mutex::new(ActiveNotification::default()));
            let client_tx_for_catchup = client_tx.clone();

            {
                let mut entry =
                    clients_tx
                        .entry(client_id.clone())
                        .or_insert_with(|| ClientEntry {
                            shard: shard.clone(),
                            last_read_id: Mutex::new(StreamEntry::default()),
                            sessions: if session_id.is_some() {
                                SessionMap::Multi(FxHashMap::default())
                            } else {
                                SessionMap::Single((client_tx.clone(), active_notification.clone()))
                            },
                        });

                match (&mut entry.sessions, session_id) {
                    (SessionMap::Multi(sessions), Some(session_id)) => {
                        sessions.insert(session_id, (client_tx, active_notification));
                    }
                    (sessions @ SessionMap::Multi(_), None) => {
                        *sessions = SessionMap::Single((client_tx, active_notification));
                    }
                    (sessions @ SessionMap::Single(_), Some(session_id)) => {
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
                let expired_queue = expired_queue.clone();
                tokio::spawn(async move {
                    dispatch_and_send_notifications(
                        &redis_pool,
                        &clients_tx,
                        &expired_queue,
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
    expired_queue: ExpiredQueue,
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
            expired_queue.clone(),
            max_shards,
            read_all_connected_client_notifications,
        )
        .await;
    }
    error!("[Notification Service Error] - read_notification_rx closed");
}

const RETRY_PER_CLIENT_CONCURRENCY: usize = 256;
const RETRY_PER_NOTIFICATION_CONCURRENCY: usize = 32;
const RETRY_PER_TARGET_CONCURRENCY: usize = 8;

fn ingest_backfill(clients_tx: &Arc<ReaderMap>, client_id: &ClientId, notifs: &[NotificationData]) {
    if notifs.is_empty() {
        return;
    }
    advance_cursor(clients_tx, client_id, notifs);
    let (_, actives) = snapshot_session_actives(clients_tx, client_id);
    if actives.is_empty() {
        return;
    }
    for active in actives {
        let mut guard = active.lock();
        for n in notifs {
            if guard.try_claim_total(n) {
                TOTAL_NOTIFICATIONS.with_label_values(&[&n.category]).inc();
            }
        }
        guard.update(notifs.to_vec());
    }
}

#[macros::measure_duration]
async fn backfill_new_entries(
    redis_pool: &Arc<RedisConnectionPool>,
    clients_tx: &Arc<ReaderMap>,
    max_shards: u64,
) {
    let mut by_shard: Vec<Vec<(ClientId, StreamEntry)>> =
        (0..max_shards as usize).map(|_| Vec::new()).collect();
    for entry in clients_tx.iter() {
        let shard_idx = entry.value().shard.inner() as usize;
        let cursor = entry.value().last_read_id.lock().clone();
        by_shard[shard_idx].push((entry.key().clone(), cursor));
    }

    let tasks: Vec<_> = by_shard
        .into_iter()
        .enumerate()
        .map(|(shard_idx, items)| {
            let shard = Shard(shard_idx as u64);
            let redis_pool = redis_pool.clone();
            let clients_tx = clients_tx.clone();
            async move {
                if items.is_empty() {
                    return;
                }
                match read_client_notifications(&redis_pool, items, &shard).await {
                    Ok(results) => {
                        for (client_id, notifs) in results {
                            ingest_backfill(&clients_tx, &client_id, &notifs);
                        }
                    }
                    Err(err) => error!(
                        "[Notification Service Error] - read_client_notifications : {}",
                        err
                    ),
                }
            }
        })
        .collect();
    join_all(tasks).await;
}

struct PendingClientWork {
    client_id: ClientId,
    shard: Shard,
    target_client_txs: Vec<ClientTx>,
    active: Arc<Mutex<ActiveNotification>>,
}

#[macros::measure_duration]
async fn retry_pending_in_memory(
    redis_pool: &Arc<RedisConnectionPool>,
    clients_tx: &Arc<ReaderMap>,
    expired_queue: &ExpiredQueue,
) {
    let mut work: Vec<PendingClientWork> = Vec::new();
    for entry in clients_tx.iter() {
        let shard = entry.value().shard.clone();
        let client_id = entry.key().clone();
        match &entry.value().sessions {
            SessionMap::Single((tx, active)) => work.push(PendingClientWork {
                client_id,
                shard,
                target_client_txs: vec![tx.clone()],
                active: active.clone(),
            }),
            SessionMap::Multi(sessions) => {
                if let Some((_, (_, primary_active))) = sessions.iter().next() {
                    let txs: Vec<ClientTx> = sessions.values().map(|(tx, _)| tx.clone()).collect();
                    work.push(PendingClientWork {
                        client_id,
                        shard,
                        target_client_txs: txs,
                        active: primary_active.clone(),
                    });
                }
            }
        }
    }

    stream::iter(work.into_iter())
        .for_each_concurrent(RETRY_PER_CLIENT_CONCURRENCY, |w| {
            let _redis_pool = redis_pool.clone();
            let expired_queue = expired_queue.clone();
            async move {
                let pending = w.active.lock().pending_redelivery();
                if pending.is_empty() {
                    return;
                }
                stream::iter(pending.into_iter())
                    .for_each_concurrent(RETRY_PER_NOTIFICATION_CONCURRENCY, |notification| {
                        let active = w.active.clone();
                        let txs = w.target_client_txs.clone();
                        let client_id = w.client_id.clone();
                        let shard = w.shard.clone();
                        let expired_queue = expired_queue.clone();
                        async move {
                            if notification.ttl.inner() < Utc::now() {
                                let reason = active
                                    .lock()
                                    .try_claim_expired_with_reason(&notification.id);
                                if let Some(reason) = reason {
                                    EXPIRED_NOTIFICATIONS
                                        .with_label_values(&[
                                            &notification.category,
                                            reason.as_str(),
                                        ])
                                        .inc();
                                }
                                try_push_expired(
                                    &expired_queue,
                                    &client_id,
                                    shard.inner(),
                                    notification.stream_id.inner(),
                                    "retry",
                                );
                                active.lock().acknowledge(&notification.id);
                                return;
                            }

                            if active.lock().try_claim_retry(&notification.id) {
                                RETRIED_NOTIFICATIONS
                                    .with_label_values(&[&notification.category])
                                    .inc();
                            }

                            stream::iter(txs.into_iter())
                                .for_each_concurrent(RETRY_PER_TARGET_CONCURRENCY, |client_tx| {
                                    let active = active.clone();
                                    let notification = notification.clone();
                                    let notification_id = notification.id.clone();
                                    async move {
                                        match send_notification(&client_tx, notification, "retry")
                                            .await
                                        {
                                            Ok(()) => active
                                                .lock()
                                                .mark_sent(&notification_id, Utc::now()),
                                            Err(err) => warn!("[Send Failed] : {}", err),
                                        }
                                    }
                                })
                                .await;
                        }
                    })
                    .await;
            }
        })
        .await;
}

#[macros::measure_duration]
async fn retry_notifications(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    expired_queue: ExpiredQueue,
    max_shards: u64,
    _read_all_connected_client_notifications: bool,
) {
    backfill_new_entries(&redis_pool, &clients_tx, max_shards).await;
    retry_pending_in_memory(&redis_pool, &clients_tx, &expired_queue).await;
}

async fn retry_notifications_looper(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    expired_queue: ExpiredQueue,
    max_shards: u64,
    read_all_connected_client_notifications: bool,
    delay: Duration,
) {
    loop {
        retry_notifications(
            redis_pool.clone(),
            clients_tx.clone(),
            expired_queue.clone(),
            max_shards,
            read_all_connected_client_notifications,
        )
        .await;
        sleep(delay).await;
    }
}

#[allow(clippy::type_complexity)]
fn snapshot_session_actives(
    clients_tx: &Arc<ReaderMap>,
    client_id: &ClientId,
) -> (
    Option<Arc<Mutex<ActiveNotification>>>,
    Vec<Arc<Mutex<ActiveNotification>>>,
) {
    match clients_tx.get(client_id) {
        Some(entry) => match &entry.value().sessions {
            SessionMap::Single((_, active)) => (Some(active.clone()), vec![active.clone()]),
            SessionMap::Multi(client) => {
                let actives: Vec<_> = client.values().map(|(_, a)| a.clone()).collect();
                let primary = actives.first().cloned();
                (primary, actives)
            }
        },
        None => (None, vec![]),
    }
}

fn advance_cursor(clients_tx: &Arc<ReaderMap>, client_id: &ClientId, notifs: &[NotificationData]) {
    if notifs.is_empty() {
        return;
    }
    let max_in_batch = notifs
        .iter()
        .map(|n| n.stream_id.inner())
        .reduce(|a, b| max_stream_id(&a, &b))
        .unwrap_or_default();
    if let Some(entry) = clients_tx.get(client_id) {
        let mut cursor = entry.value().last_read_id.lock();
        let advanced = max_stream_id(&cursor.inner(), &max_in_batch);
        *cursor = StreamEntry(advanced);
    }
}

fn read_cursor(clients_tx: &Arc<ReaderMap>, client_id: &ClientId) -> StreamEntry {
    clients_tx
        .get(client_id)
        .map(|entry| entry.value().last_read_id.lock().clone())
        .unwrap_or_default()
}

#[macros::measure_duration]
async fn active_notification_dispatch(
    redis_pool: &RedisConnectionPool,
    clients_tx: &Arc<ReaderMap>,
    client_id: &ClientId,
    shard: &Shard,
) -> Option<(Vec<NotificationData>, Arc<Mutex<ActiveNotification>>)> {
    let cursor = read_cursor(clients_tx, client_id);
    let notifications = read_client_notification(redis_pool, client_id, shard, &cursor)
        .await
        .ok()?;

    let (primary_active, all_actives) = snapshot_session_actives(clients_tx, client_id);
    if primary_active.is_none() {
        error!(
            "[Notification Service Error] - ClientId {:?} not found here.",
            client_id
        );
        return None;
    }

    advance_cursor(clients_tx, client_id, &notifications);

    for active in all_actives {
        active.lock().update(notifications.to_owned());
    }

    primary_active.map(|active| (notifications, active))
}

async fn dispatch_and_send_notifications(
    redis_pool: &RedisConnectionPool,
    clients_tx: &Arc<ReaderMap>,
    expired_queue: &ExpiredQueue,
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
        let (count_total, expiry_reason) = {
            let mut guard = active.lock();
            let ct = guard.try_claim_total(&notification);
            let reason = if notification.ttl.inner() < Utc::now() {
                guard.try_claim_expired_with_reason(&notification.id)
            } else {
                None
            };
            (ct, reason)
        };

        if count_total {
            TOTAL_NOTIFICATIONS
                .with_label_values(&[&notification.category])
                .inc();
        }

        if notification.ttl.inner() < Utc::now() {
            if let Some(reason) = expiry_reason {
                EXPIRED_NOTIFICATIONS
                    .with_label_values(&[&notification.category, reason.as_str()])
                    .inc();
            }
            try_push_expired(
                expired_queue,
                client_id,
                shard.inner(),
                notification.stream_id.inner(),
                "dispatch",
            );
        } else {
            for client_tx in target_client_txs {
                match send_notification(client_tx, notification.to_owned(), source).await {
                    Ok(()) => {
                        active.lock().mark_sent(&notification.id, Utc::now());
                    }
                    Err(err) => warn!("[Send Failed] : {}", err),
                }
            }
        }
    }
}

async fn active_notification(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    expired_queue: ExpiredQueue,
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
            let expired_queue_ref = expired_queue.clone();
            dispatch_and_send_notifications(
                &redis_pool,
                &clients_tx,
                &expired_queue_ref,
                &client_id,
                &shard,
                &all_clients_tx,
                "pubsub_fresh",
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
    expired_queue: ExpiredQueue,
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
                    expired_queue.clone(),
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

        retry_notifications(
            redis_pool.clone(),
            clients_tx.clone(),
            expired_queue.clone(),
            max_shards,
            true,
        )
        .await;

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
    expired_cleanup_delay_millis: u64,
    max_shards: u64,
    read_all_connected_client_notifications: bool,
) {
    let expired_queue = new_expired_queue();

    let rx_task = tokio::spawn(client_reciever_looper(
        redis_pool.clone(),
        read_notification_rx,
        clients_tx.clone(),
        expired_queue.clone(),
        max_shards,
        read_all_connected_client_notifications,
    ));

    let retry_notifications_task = tokio::spawn(retry_notifications_looper(
        redis_pool.clone(),
        clients_tx.clone(),
        expired_queue.clone(),
        max_shards,
        read_all_connected_client_notifications,
        Duration::from_millis(retry_delay_millis),
    ));

    let expire_notifications_task = tokio::spawn(expire_notifications_looper(
        redis_pool.clone(),
        expired_queue.clone(),
        Duration::from_millis(expired_cleanup_delay_millis),
    ));

    if read_all_connected_client_notifications {
        tokio::select!(
            res = rx_task => {
                error!("[Notification Service Error] - [CLIENT_RECIEVER_TASK] : {:?}", res);
            },
            res = retry_notifications_task => {
                error!("[Notification Service Error] - [RETRY_NOTIFICATION_TASK] : {:?}", res);
            },
            res = expire_notifications_task => {
                error!("[Notification Service Error] - [EXPIRE_NOTIFICATION_TASK] : {:?}", res);
            },
            _ = graceful_termination_signal_rx => {
                error!("[Notification Service Error] - [GRACEFUL_SHUT_DOWN]");
            }
        );
    } else {
        let active_notifications = tokio::spawn(active_notification_looper(
            redis_pool.clone(),
            clients_tx.clone(),
            expired_queue.clone(),
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
            res = expire_notifications_task => {
                error!("[Notification Service Error] - [EXPIRE_NOTIFICATION_TASK] : {:?}", res);
            },
            _ = graceful_termination_signal_rx => {
                error!("[Notification Service Error] - [GRACEFUL_SHUT_DOWN]");
            }
        );
    }
}
