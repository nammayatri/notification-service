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
            abs_diff_utc_as_sec, get_timestamp_from_stream_id, hash_uuid, is_stream_id_less_or_eq,
            max_stream_id, transform_notification_data_to_payload,
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
        CHANNEL_DELAY, CLEANUP_PUSH_SKIPPED, CLIENT_SLOT_EVENTS, CONNECTED_CLIENTS,
        EXPIRED_NOTIFICATIONS, MEASURE_DURATION, NOTIFICATION_LATENCY, PUBSUB_MESSAGES,
        RETRIED_NOTIFICATIONS, TOTAL_NOTIFICATIONS,
    },
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use dashmap::{mapref::entry::Entry, DashMap};
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

pub fn new_expired_queue() -> ExpiredQueue {
    Arc::new(DashMap::with_hasher(FxBuildHasher::default()))
}

fn try_push_expired(
    expired_queue: &ExpiredQueue,
    client_id: &ClientId,
    shard: u64,
    stream_id: String,
    meta: ExpiredMeta,
    origin: &'static str,
) {
    if let Some(entry) = expired_queue.get(client_id) {
        match entry.value().stream_ids.try_lock() {
            Some(mut guard) => {
                guard.entry(stream_id).or_insert(meta);
                return;
            }
            None => {
                CLEANUP_PUSH_SKIPPED.with_label_values(&[origin]).inc();
                return;
            }
        }
    }

    expired_queue
        .entry(client_id.clone())
        .or_insert_with(|| ExpiredEntry {
            shard,
            stream_ids: Mutex::new(FxHashMap::default()),
        })
        .stream_ids
        .lock()
        .entry(stream_id)
        .or_insert(meta);
}

fn push_delivered_cleanup(
    cleanup_queue: &ExpiredQueue,
    client_id: &ClientId,
    shard: u64,
    notification: &NotificationData,
) {
    cleanup_queue
        .entry(client_id.clone())
        .or_insert_with(|| ExpiredEntry {
            shard,
            stream_ids: Mutex::new(FxHashMap::default()),
        })
        .stream_ids
        .lock()
        .insert(
            notification.stream_id.inner(),
            ExpiredMeta {
                category: notification.category.clone(),
                reason: CleanupReason::Delivered,
            },
        );
}

#[allow(clippy::too_many_arguments)]
fn settle_push_round(
    policy: DeliveryPolicy,
    clients_tx: &Arc<ReaderMap>,
    cleanup_queue: &ExpiredQueue,
    client_id: &ClientId,
    shard: &Shard,
    active: &Arc<Mutex<ActiveNotification>>,
    notification: &NotificationData,
    pushed: bool,
) {
    if !pushed {
        active.lock().release_push(&notification.id);
        return;
    }
    if policy.guarantee.removes_on_push() {
        let (_, actives) = snapshot_session_actives(clients_tx, client_id);
        for session_active in actives {
            session_active.lock().acknowledge(&notification.id);
        }
        push_delivered_cleanup(cleanup_queue, client_id, shard.inner(), notification);
    }
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
    attempt: &'static str,
) -> Result<()> {
    client_tx_send(client_tx, &notification).await?;

    notification_latency!(
        get_timestamp_from_stream_id(&notification.stream_id.inner()).inner(),
        "NACK",
        source,
        attempt
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
        let (shard, drained) = match expired_queue.get(&client_id) {
            Some(entry) => {
                let shard = entry.value().shard;
                let mut guard = entry.value().stream_ids.lock();
                if guard.is_empty() {
                    continue;
                }
                let drained: Vec<(String, ExpiredMeta)> = guard.drain().collect();
                (shard, drained)
            }
            None => continue,
        };
        expired_queue.remove_if(&client_id, |_, e| e.stream_ids.lock().is_empty());

        let mut ids: Vec<String> = Vec::with_capacity(drained.len());
        for (stream_id, meta) in drained {
            if let CleanupReason::Expired(reason) = meta.reason {
                EXPIRED_NOTIFICATIONS
                    .with_label_values(&[&meta.category, reason.as_str()])
                    .inc();
            }
            ids.push(stream_id);
        }

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
    stream_token: StreamToken,
    stale_disconnect_guard: bool,
) {
    let start = tokio::time::Instant::now();

    let should_remove = if let Some(mut entry) = clients_tx.get_mut(client_id) {
        match &mut entry.value_mut().sessions {
            SessionMap::Single((owner, _, _)) if *owner == stream_token => {
                CLIENT_SLOT_EVENTS.with_label_values(&["removed"]).inc();
                true
            }
            SessionMap::Single(_) if stale_disconnect_guard => {
                CLIENT_SLOT_EVENTS
                    .with_label_values(&["stale_disconnect_ignored"])
                    .inc();
                false
            }
            SessionMap::Single(_) => {
                CLIENT_SLOT_EVENTS
                    .with_label_values(&["stale_disconnect_evicted"])
                    .inc();
                true
            }
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
        CLIENT_SLOT_EVENTS.with_label_values(&["not_found"]).inc();
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

#[derive(Debug, Clone, Copy)]
struct ReceiverOptions {
    max_shards: u64,
    delivery_mode: DeliveryMode,
    stale_disconnect_guard: bool,
    policy: DeliveryPolicy,
}

#[macros::measure_duration]
async fn client_reciever(
    redis_pool: Arc<RedisConnectionPool>,
    client_id: ClientId,
    client_req: SenderType,
    clients_tx: Arc<ReaderMap>,
    expired_queue: ExpiredQueue,
    options: ReceiverOptions,
) {
    let ReceiverOptions {
        max_shards,
        delivery_mode,
        stale_disconnect_guard,
        policy,
    } = options;
    match client_req {
        SenderType::ClientConnection((session_id, stream_token, client_tx)) => {
            info!("[Client Connected] : {:?}", client_id);
            CONNECTED_CLIENTS.inc();

            let shard = Shard((hash_uuid(&client_id.inner()) % max_shards as u128) as u64);

            let start = tokio::time::Instant::now();

            let active_notification = Arc::new(Mutex::new(ActiveNotification::default()));
            let client_tx_for_catchup = client_tx.clone();

            match clients_tx.entry(client_id.clone()) {
                Entry::Vacant(vacant) => {
                    CLIENT_SLOT_EVENTS.with_label_values(&["inserted"]).inc();
                    let sessions = match session_id {
                        Some(session_id) => {
                            let mut map = FxHashMap::default();
                            map.insert(session_id, (client_tx, active_notification));
                            SessionMap::Multi(map)
                        }
                        None => SessionMap::Single((stream_token, client_tx, active_notification)),
                    };
                    vacant.insert(ClientEntry {
                        shard: shard.clone(),
                        last_read_id: Mutex::new(StreamEntry::default()),
                        sessions,
                    });
                }
                Entry::Occupied(mut occupied) => {
                    match (&mut occupied.get_mut().sessions, session_id) {
                        (SessionMap::Multi(sessions), Some(session_id)) => {
                            sessions.insert(session_id, (client_tx, active_notification));
                        }
                        (sessions @ SessionMap::Multi(_), None) => {
                            *sessions =
                                SessionMap::Single((stream_token, client_tx, active_notification));
                        }
                        (sessions @ SessionMap::Single(_), Some(session_id)) => {
                            let mut map = FxHashMap::default();
                            map.insert(session_id, (client_tx, active_notification));
                            *sessions = SessionMap::Multi(map);
                        }
                        (sessions @ SessionMap::Single(_), None) => {
                            CLIENT_SLOT_EVENTS.with_label_values(&["replaced"]).inc();
                            *sessions =
                                SessionMap::Single((stream_token, client_tx, active_notification));
                        }
                    }
                }
            }

            measure_latency_duration!("client_reciever_clients_tx_write", start);

            if delivery_mode.needs_connect_catchup() {
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
                        policy,
                    )
                    .await;
                });
            }
        }
        SenderType::ClientDisconnection((session_id, stream_token)) => {
            warn!("[Client Disconnected] : {:?} : {:?}", client_id, session_id);
            CONNECTED_CLIENTS.dec();
            handle_client_disconnection_or_failure(
                clients_tx.clone(),
                &client_id,
                &session_id,
                stream_token,
                stale_disconnect_guard,
            )
            .await;
        }
    }
}

async fn client_reciever_looper(
    redis_pool: Arc<RedisConnectionPool>,
    mut read_notification_rx: Receiver<(ClientId, SenderType, DateTime<Utc>)>,
    clients_tx: Arc<ReaderMap>,
    expired_queue: ExpiredQueue,
    options: ReceiverOptions,
) {
    while let Some((client_id, client_tx, sent_at)) = read_notification_rx.recv().await {
        channel_delay!(sent_at, &client_tx.to_string());

        client_reciever(
            redis_pool.clone(),
            client_id,
            client_tx,
            clients_tx.clone(),
            expired_queue.clone(),
            options,
        )
        .await;
    }
    error!("[Notification Service Error] - read_notification_rx closed");
}

const RETRY_PER_CLIENT_CONCURRENCY: usize = 256;
const RETRY_PER_NOTIFICATION_CONCURRENCY: usize = 32;
const RETRY_PER_TARGET_CONCURRENCY: usize = 8;

fn ingest_backfill(
    clients_tx: &Arc<ReaderMap>,
    client_id: &ClientId,
    notifs: Vec<NotificationData>,
    policy: DeliveryPolicy,
) {
    let notifs = claim_read_batch(clients_tx, client_id, notifs, policy);
    if notifs.is_empty() {
        return;
    }
    let (_, actives) = snapshot_session_actives(clients_tx, client_id);
    if actives.is_empty() {
        return;
    }
    for active in actives {
        let mut guard = active.lock();
        for n in &notifs {
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
    policy: DeliveryPolicy,
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
                            ingest_backfill(&clients_tx, &client_id, notifs, policy);
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
    policy: DeliveryPolicy,
) {
    let mut work: Vec<PendingClientWork> = Vec::new();
    for entry in clients_tx.iter() {
        let shard = entry.value().shard.clone();
        let client_id = entry.key().clone();
        match &entry.value().sessions {
            SessionMap::Single((_, tx, active)) => work.push(PendingClientWork {
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
                                    try_push_expired(
                                        &expired_queue,
                                        &client_id,
                                        shard.inner(),
                                        notification.stream_id.inner(),
                                        ExpiredMeta {
                                            category: notification.category.clone(),
                                            reason: CleanupReason::Expired(reason),
                                        },
                                        "retry",
                                    );
                                }
                                active.lock().acknowledge(&notification.id);
                                return;
                            }

                            let attempt = {
                                let mut guard = active.lock();
                                if !guard.try_claim_push(&notification.id, policy.push_cap) {
                                    return;
                                }
                                if guard.try_claim_total(&notification) {
                                    TOTAL_NOTIFICATIONS
                                        .with_label_values(&[&notification.category])
                                        .inc();
                                }
                                if guard.try_claim_retry(&notification.id) {
                                    RETRIED_NOTIFICATIONS
                                        .with_label_values(&[&notification.category])
                                        .inc();
                                }
                                guard.attempt(&notification.id)
                            };

                            let pushed = stream::iter(txs.into_iter())
                                .map(|client_tx| {
                                    let active = active.clone();
                                    let notification = notification.clone();
                                    let notification_id = notification.id.clone();
                                    async move {
                                        match send_notification(
                                            &client_tx,
                                            notification,
                                            "retry",
                                            attempt,
                                        )
                                        .await
                                        {
                                            Ok(()) => {
                                                active
                                                    .lock()
                                                    .mark_sent(&notification_id, Utc::now());
                                                true
                                            }
                                            Err(err) => {
                                                warn!("[Send Failed] : {}", err);
                                                false
                                            }
                                        }
                                    }
                                })
                                .buffer_unordered(RETRY_PER_TARGET_CONCURRENCY)
                                .fold(false, |pushed, sent| async move { pushed || sent })
                                .await;

                            settle_push_round(
                                policy,
                                clients_tx,
                                &expired_queue,
                                &client_id,
                                &shard,
                                &active,
                                &notification,
                                pushed,
                            );
                        }
                    })
                    .await;
            }
        })
        .await;
}

#[macros::measure_duration]
async fn full_sweep(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    expired_queue: ExpiredQueue,
    max_shards: u64,
    policy: DeliveryPolicy,
) {
    backfill_new_entries(&redis_pool, &clients_tx, max_shards, policy).await;
    retry_pending_in_memory(&redis_pool, &clients_tx, &expired_queue, policy).await;
}

async fn sweep_looper(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    expired_queue: ExpiredQueue,
    max_shards: u64,
    delay: Duration,
    policy: DeliveryPolicy,
) {
    loop {
        full_sweep(
            redis_pool.clone(),
            clients_tx.clone(),
            expired_queue.clone(),
            max_shards,
            policy,
        )
        .await;
        sleep(delay).await;
    }
}

async fn retry_looper(
    redis_pool: Arc<RedisConnectionPool>,
    clients_tx: Arc<ReaderMap>,
    expired_queue: ExpiredQueue,
    delay: Duration,
    policy: DeliveryPolicy,
) {
    loop {
        retry_pending_in_memory(&redis_pool, &clients_tx, &expired_queue, policy).await;
        sleep(delay).await;
    }
}

async fn optional_task(task: Option<tokio::task::JoinHandle<()>>) -> String {
    match task {
        Some(handle) => format!("{:?}", handle.await),
        None => std::future::pending().await,
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
            SessionMap::Single((_, _, active)) => (Some(active.clone()), vec![active.clone()]),
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

fn claim_read_batch(
    clients_tx: &Arc<ReaderMap>,
    client_id: &ClientId,
    notifs: Vec<NotificationData>,
    policy: DeliveryPolicy,
) -> Vec<NotificationData> {
    if !policy.guarantee.removes_on_push() {
        advance_cursor(clients_tx, client_id, &notifs);
        return notifs;
    }
    let Some(entry) = clients_tx.get(client_id) else {
        return Vec::new();
    };
    let mut cursor = entry.value().last_read_id.lock();
    let claimed: Vec<NotificationData> = notifs
        .into_iter()
        .filter(|n| !is_stream_id_less_or_eq(&n.stream_id.inner(), &cursor.inner()))
        .collect();
    if let Some(max_claimed) = claimed
        .iter()
        .map(|n| n.stream_id.inner())
        .reduce(|a, b| max_stream_id(&a, &b))
    {
        *cursor = StreamEntry(max_claimed);
    }
    claimed
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
    policy: DeliveryPolicy,
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

    let notifications = claim_read_batch(clients_tx, client_id, notifications, policy);

    for active in all_actives {
        active.lock().update(notifications.to_owned());
    }

    primary_active.map(|active| (notifications, active))
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_and_send_notifications(
    redis_pool: &RedisConnectionPool,
    clients_tx: &Arc<ReaderMap>,
    expired_queue: &ExpiredQueue,
    client_id: &ClientId,
    shard: &Shard,
    target_client_txs: &[ClientTx],
    source: &'static str,
    policy: DeliveryPolicy,
) {
    let Some((notifications, active)) =
        active_notification_dispatch(redis_pool, clients_tx, client_id, shard, policy).await
    else {
        return;
    };

    for notification in notifications {
        let expired = notification.ttl.inner() < Utc::now();
        let (count_total, expiry_reason, attempt, push_claimed) = {
            let mut guard = active.lock();
            if !guard.0.contains_key(&notification.id) {
                continue;
            }
            let ct = guard.try_claim_total(&notification);
            let reason = if expired {
                guard.try_claim_expired_with_reason(&notification.id)
            } else {
                None
            };
            let attempt = guard.attempt(&notification.id);
            let push_claimed = !expired && guard.try_claim_push(&notification.id, policy.push_cap);
            (ct, reason, attempt, push_claimed)
        };

        if count_total {
            TOTAL_NOTIFICATIONS
                .with_label_values(&[&notification.category])
                .inc();
        }

        if expired {
            if let Some(reason) = expiry_reason {
                try_push_expired(
                    expired_queue,
                    client_id,
                    shard.inner(),
                    notification.stream_id.inner(),
                    ExpiredMeta {
                        category: notification.category.clone(),
                        reason: CleanupReason::Expired(reason),
                    },
                    "dispatch",
                );
            }
        } else if push_claimed {
            let mut pushed = false;
            for client_tx in target_client_txs {
                match send_notification(client_tx, notification.to_owned(), source, attempt).await {
                    Ok(()) => {
                        active.lock().mark_sent(&notification.id, Utc::now());
                        pushed = true;
                    }
                    Err(err) => warn!("[Send Failed] : {}", err),
                }
            }
            settle_push_round(
                policy,
                clients_tx,
                expired_queue,
                client_id,
                shard,
                &active,
                &notification,
                pushed,
            );
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
    policy: DeliveryPolicy,
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
                    SessionMap::Single((_, client_tx, _)) => vec![client_tx.clone()],
                    SessionMap::Multi(client) => {
                        client.values().map(|(tx, _)| tx.clone()).collect()
                    }
                };
                (Some(shard), txs)
            }
            None => (None, vec![]),
        };

        let Some(shard) = shard_opt else {
            PUBSUB_MESSAGES.with_label_values(&["foreign"]).inc();
            warn!(
                "Client ({:?}) entry does not exist, client got disconnected intermittently.",
                client_id
            );
            continue;
        };

        if !all_clients_tx.is_empty() {
            PUBSUB_MESSAGES.with_label_values(&["local"]).inc();
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
                policy,
            )
            .await;
        } else {
            PUBSUB_MESSAGES.with_label_values(&["no_session"]).inc();
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
    policy: DeliveryPolicy,
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
                    policy,
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

        full_sweep(
            redis_pool.clone(),
            clients_tx.clone(),
            expired_queue.clone(),
            max_shards,
            policy,
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
    sweep_delay_millis: u64,
    expired_cleanup_delay_millis: u64,
    max_shards: u64,
    delivery_mode: DeliveryMode,
    stale_disconnect_guard: bool,
    policy: DeliveryPolicy,
) {
    let expired_queue = new_expired_queue();

    info!(
        "[Notification Service] - delivery_mode: {}, delivery_guarantee: {}, push_cap: {:?}, sweep_delay: {}ms, retry_delay: {}ms, max_shards: {}, stale_disconnect_guard: {}",
        delivery_mode, policy.guarantee, policy.push_cap, sweep_delay_millis, retry_delay_millis, max_shards, stale_disconnect_guard
    );

    let rx_task = tokio::spawn(client_reciever_looper(
        redis_pool.clone(),
        read_notification_rx,
        clients_tx.clone(),
        expired_queue.clone(),
        ReceiverOptions {
            max_shards,
            delivery_mode,
            stale_disconnect_guard,
            policy,
        },
    ));

    let retry_task = delivery_mode.needs_independent_retry_loop().then(|| {
        tokio::spawn(retry_looper(
            redis_pool.clone(),
            clients_tx.clone(),
            expired_queue.clone(),
            Duration::from_millis(retry_delay_millis),
            policy,
        ))
    });

    let expire_notifications_task = tokio::spawn(expire_notifications_looper(
        redis_pool.clone(),
        expired_queue.clone(),
        Duration::from_millis(expired_cleanup_delay_millis),
    ));

    let sweep_task = tokio::spawn(sweep_looper(
        redis_pool.clone(),
        clients_tx.clone(),
        expired_queue.clone(),
        max_shards,
        Duration::from_millis(sweep_delay_millis),
        policy,
    ));

    let active_notification_task = (delivery_mode == DeliveryMode::Pubsub).then(|| {
        tokio::spawn(active_notification_looper(
            redis_pool.clone(),
            clients_tx.clone(),
            expired_queue.clone(),
            max_shards,
            policy,
        ))
    });

    tokio::select!(
        res = rx_task => {
            error!("[Notification Service Error] - [CLIENT_RECIEVER_TASK] : {:?}", res);
        },
        res = optional_task(retry_task) => {
            error!("[Notification Service Error] - [RETRY_TASK] : {}", res);
        },
        res = expire_notifications_task => {
            error!("[Notification Service Error] - [EXPIRE_NOTIFICATION_TASK] : {:?}", res);
        },
        res = sweep_task => {
            error!("[Notification Service Error] - [SWEEP_TASK] : {:?}", res);
        },
        res = optional_task(active_notification_task) => {
            error!("[Notification Service Error] - [ACTIVE_NOTIFICATION_TASK] : {}", res);
        },
        _ = graceful_termination_signal_rx => {
            error!("[Notification Service Error] - [GRACEFUL_SHUT_DOWN]");
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redis::types::EntityData;
    use std::num::NonZeroU32;

    fn notification(stream_id: &str) -> NotificationData {
        NotificationData {
            stream_id: StreamEntry(stream_id.to_string()),
            id: NotificationId(format!("id-{stream_id}")),
            category: "TEST".to_string(),
            title: String::new(),
            body: String::new(),
            show: String::new(),
            created_at: Utc::now(),
            ttl: Ttl(Utc::now() + chrono::Duration::minutes(5)),
            entity: EntityData {
                id: String::new(),
                _type: String::new(),
                data: String::new(),
            },
        }
    }

    fn reader_map_with_client(client_id: &ClientId) -> Arc<ReaderMap> {
        reader_map_owned_by(client_id, StreamToken::next())
    }

    fn reader_map_owned_by(client_id: &ClientId, owner: StreamToken) -> Arc<ReaderMap> {
        let clients_tx: Arc<ReaderMap> = Arc::new(DashMap::with_hasher(FxBuildHasher::default()));
        let (client_tx, _client_rx) = sync::mpsc::channel(1);
        clients_tx.insert(
            client_id.clone(),
            ClientEntry {
                shard: Shard(0),
                last_read_id: Mutex::new(StreamEntry::default()),
                sessions: SessionMap::Single((
                    owner,
                    client_tx,
                    Arc::new(Mutex::new(ActiveNotification::default())),
                )),
            },
        );
        clients_tx
    }

    #[tokio::test]
    async fn disconnect_from_the_owning_stream_removes_the_client() {
        let client_id = ClientId("c1".to_string());
        let owner = StreamToken::next();
        let clients_tx = reader_map_owned_by(&client_id, owner);

        handle_client_disconnection_or_failure(clients_tx.clone(), &client_id, &None, owner, true)
            .await;

        assert!(clients_tx.get(&client_id).is_none());
    }

    #[tokio::test]
    async fn stale_disconnect_is_ignored_when_guarded() {
        let client_id = ClientId("c1".to_string());
        let stale = StreamToken::next();
        let clients_tx = reader_map_owned_by(&client_id, StreamToken::next());

        handle_client_disconnection_or_failure(clients_tx.clone(), &client_id, &None, stale, true)
            .await;

        assert!(clients_tx.get(&client_id).is_some());
    }

    #[tokio::test]
    async fn stale_disconnect_still_evicts_when_unguarded() {
        let client_id = ClientId("c1".to_string());
        let stale = StreamToken::next();
        let clients_tx = reader_map_owned_by(&client_id, StreamToken::next());

        handle_client_disconnection_or_failure(clients_tx.clone(), &client_id, &None, stale, false)
            .await;

        assert!(clients_tx.get(&client_id).is_none());
    }

    fn stream_ids(notifs: &[NotificationData]) -> Vec<String> {
        notifs.iter().map(|n| n.stream_id.inner()).collect()
    }

    #[test]
    fn at_most_once_hands_each_entry_to_one_racing_read() {
        let client_id = ClientId("c1".to_string());
        let clients_tx = reader_map_with_client(&client_id);
        let policy = DeliveryPolicy::new(DeliveryGuarantee::AtMostOnce, None);
        let stale_read = vec![notification("1-0"), notification("2-0")];

        let first = claim_read_batch(&clients_tx, &client_id, stale_read.clone(), policy);
        let second = claim_read_batch(
            &clients_tx,
            &client_id,
            [stale_read, vec![notification("3-0")]].concat(),
            policy,
        );

        assert_eq!(stream_ids(&first), vec!["1-0", "2-0"]);
        assert_eq!(stream_ids(&second), vec!["3-0"]);
        assert_eq!(read_cursor(&clients_tx, &client_id).inner(), "3-0");
    }

    #[test]
    fn at_least_once_passes_racing_reads_through() {
        let client_id = ClientId("c1".to_string());
        let clients_tx = reader_map_with_client(&client_id);
        let policy = DeliveryPolicy::new(DeliveryGuarantee::AtLeastOnce, None);
        let stale_read = vec![notification("1-0"), notification("2-0")];

        claim_read_batch(&clients_tx, &client_id, stale_read.clone(), policy);
        let second = claim_read_batch(&clients_tx, &client_id, stale_read, policy);

        assert_eq!(stream_ids(&second), vec!["1-0", "2-0"]);
        assert_eq!(read_cursor(&clients_tx, &client_id).inner(), "2-0");
    }

    #[test]
    fn push_cap_follows_guarantee() {
        let at_most_once = DeliveryPolicy::new(DeliveryGuarantee::AtMostOnce, NonZeroU32::new(5));
        let capped = DeliveryPolicy::new(DeliveryGuarantee::AtLeastOnce, NonZeroU32::new(3));
        let uncapped = DeliveryPolicy::new(DeliveryGuarantee::AtLeastOnce, None);

        assert_eq!(at_most_once.push_cap, Some(1));
        assert_eq!(capped.push_cap, Some(3));
        assert_eq!(uncapped.push_cap, None);
    }

    #[test]
    fn push_claim_is_capped_and_released_on_failed_round() {
        let n = notification("1-0");
        let mut active = ActiveNotification::default();
        active.update(vec![n.clone()]);

        assert!(active.try_claim_push(&n.id, Some(1)));
        assert!(!active.try_claim_push(&n.id, Some(1)));
        active.release_push(&n.id);
        assert!(active.try_claim_push(&n.id, Some(1)));

        assert!(active.try_claim_push(&n.id, None));
        assert!(!active.try_claim_push(&NotificationId("missing".to_string()), None));
    }
}
