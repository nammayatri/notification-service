/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
use crate::{redis::types::NotificationData, NotificationPayload};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use parking_lot::Mutex;
use rustc_hash::{FxHashMap, FxHasher};
use serde::{Deserialize, Serialize};
use std::hash::BuildHasherDefault;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use strum_macros::{Display, EnumIter, EnumString};
use tokio::sync::mpsc::Sender;
use tonic::Status;

pub type FxBuildHasher = BuildHasherDefault<FxHasher>;

#[derive(Debug, Clone, Copy)]
pub enum CleanupReason {
    Expired(ExpiryReason),
    Delivered,
}

#[derive(Debug, Clone)]
pub struct ExpiredMeta {
    pub category: String,
    pub reason: CleanupReason,
}

#[derive(Debug)]
pub struct ExpiredEntry {
    pub shard: u64,
    pub stream_ids: Mutex<FxHashMap<String, ExpiredMeta>>,
}

pub type ExpiredQueue = Arc<DashMap<ClientId, ExpiredEntry, FxBuildHasher>>;

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq)]
#[macros::impl_getter]
pub struct Token(pub String);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, Hash, PartialEq)]
#[macros::impl_getter]
pub struct ClientId(pub String);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, Hash, PartialEq)]
#[macros::impl_getter]
pub struct SessionID(pub String);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, Hash, PartialEq)]
#[macros::impl_getter]
pub struct Shard(pub u64);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq, Hash)]
#[macros::impl_getter]
pub struct NotificationId(pub String);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[macros::impl_getter]
pub struct Timestamp(pub DateTime<Utc>);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[macros::impl_getter]
pub struct Ttl(pub DateTime<Utc>);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq)]
#[macros::impl_getter]
pub struct StreamEntry(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationMeta {
    pub data: NotificationData,
    pub sent_at: Option<DateTime<Utc>>,
    pub push_count: u32,
    pub total_counted: bool,
    pub retry_counted: bool,
    pub expired_counted: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum ExpiryReason {
    NoConsumer,
    Timeout,
}

impl ExpiryReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ExpiryReason::NoConsumer => "no_consumer",
            ExpiryReason::Timeout => "timeout",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AcknowledgedNotification {
    pub category: String,
    pub stream_id_to_delete: Option<StreamEntry>,
    pub sent_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AwaitingAck {
    pub category: String,
    pub sent_at: DateTime<Utc>,
    pub ttl: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ActiveNotification {
    pending: FxHashMap<NotificationId, NotificationMeta>,
    awaiting_ack: FxHashMap<NotificationId, AwaitingAck>,
}

impl ActiveNotification {
    pub fn update(&mut self, notifications: Vec<NotificationData>) {
        for notification in notifications {
            self.pending
                .entry(notification.id.clone())
                .or_insert(NotificationMeta {
                    data: notification,
                    sent_at: None,
                    push_count: 0,
                    total_counted: false,
                    retry_counted: false,
                    expired_counted: false,
                });
        }
    }

    pub fn count(&self) -> usize {
        self.pending.len()
    }

    pub fn contains(&self, notification_id: &NotificationId) -> bool {
        self.pending.contains_key(notification_id)
    }

    pub fn acknowledge(
        &mut self,
        notification_id: &NotificationId,
    ) -> Option<AcknowledgedNotification> {
        if let Some(meta) = self.pending.remove(notification_id) {
            return Some(AcknowledgedNotification {
                category: meta.data.category,
                stream_id_to_delete: Some(meta.data.stream_id),
                sent_at: meta.sent_at,
            });
        }
        self.awaiting_ack
            .remove(notification_id)
            .map(|awaiting| AcknowledgedNotification {
                category: awaiting.category,
                stream_id_to_delete: None,
                sent_at: Some(awaiting.sent_at),
            })
    }

    pub fn discard(&mut self, notification_id: &NotificationId) {
        self.pending.remove(notification_id);
    }

    pub fn await_ack(&mut self, notification_id: &NotificationId) {
        if let Some(meta) = self.pending.remove(notification_id) {
            self.awaiting_ack.insert(
                notification_id.clone(),
                AwaitingAck {
                    category: meta.data.category,
                    sent_at: meta.sent_at.unwrap_or_else(Utc::now),
                    ttl: meta.data.ttl.inner(),
                },
            );
        }
    }

    pub fn expire_awaiting_ack(&mut self, now: DateTime<Utc>) -> Vec<String> {
        let mut expired = Vec::new();
        self.awaiting_ack.retain(|_, awaiting| {
            if awaiting.ttl < now {
                expired.push(awaiting.category.clone());
                false
            } else {
                true
            }
        });
        expired
    }

    pub fn drain_awaiting_ack(&mut self) -> Vec<String> {
        self.awaiting_ack
            .drain()
            .map(|(_, awaiting)| awaiting.category)
            .collect()
    }

    pub fn try_claim_total(&mut self, notification: &NotificationData) -> bool {
        match self.pending.get_mut(&notification.id) {
            None => {
                self.pending.insert(
                    notification.id.clone(),
                    NotificationMeta {
                        data: notification.clone(),
                        sent_at: None,
                        push_count: 0,
                        total_counted: true,
                        retry_counted: false,
                        expired_counted: false,
                    },
                );
                true
            }
            Some(meta) if !meta.total_counted => {
                meta.total_counted = true;
                true
            }
            Some(_) => false,
        }
    }

    pub fn mark_sent(&mut self, notification_id: &NotificationId, now: DateTime<Utc>) {
        if let Some(meta) = self.pending.get_mut(notification_id) {
            meta.sent_at = Some(now);
        }
    }

    pub fn try_claim_push(
        &mut self,
        notification_id: &NotificationId,
        push_cap: Option<u32>,
    ) -> bool {
        match self.pending.get_mut(notification_id) {
            Some(meta) if push_cap.is_none_or(|cap| meta.push_count < cap) => {
                meta.push_count += 1;
                true
            }
            _ => false,
        }
    }

    pub fn release_push(&mut self, notification_id: &NotificationId) {
        if let Some(meta) = self.pending.get_mut(notification_id) {
            meta.push_count = meta.push_count.saturating_sub(1);
        }
    }

    pub fn attempt(&self, notification_id: &NotificationId) -> &'static str {
        match self.pending.get(notification_id) {
            Some(meta) if meta.sent_at.is_some() => "repeat",
            _ => "first",
        }
    }

    pub fn try_claim_retry(&mut self, notification_id: &NotificationId) -> bool {
        match self.pending.get_mut(notification_id) {
            Some(meta) if meta.sent_at.is_some() && !meta.retry_counted => {
                meta.retry_counted = true;
                true
            }
            _ => false,
        }
    }

    pub fn try_claim_expired(&mut self, notification_id: &NotificationId) -> bool {
        match self.pending.get_mut(notification_id) {
            Some(meta) if !meta.expired_counted => {
                meta.expired_counted = true;
                true
            }
            _ => false,
        }
    }

    pub fn try_claim_expired_with_reason(
        &mut self,
        notification_id: &NotificationId,
    ) -> Option<ExpiryReason> {
        match self.pending.get_mut(notification_id) {
            Some(meta) if !meta.expired_counted => {
                meta.expired_counted = true;
                Some(if meta.sent_at.is_some() {
                    ExpiryReason::Timeout
                } else {
                    ExpiryReason::NoConsumer
                })
            }
            _ => None,
        }
    }

    pub fn refresh(&mut self) {
        let now = Utc::now();
        self.pending.retain(|_, meta| meta.data.ttl.inner() >= now);
    }

    pub fn pending_redelivery(&self) -> Vec<NotificationData> {
        self.pending.values().map(|m| m.data.clone()).collect()
    }
}

impl Default for StreamEntry {
    fn default() -> Self {
        Self("0-0".to_string())
    }
}

pub type ClientTx = Sender<Result<NotificationPayload, Status>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamToken(pub u64);

impl StreamToken {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Display)]
pub enum SenderType {
    #[strum(to_string = "ClientConnection")]
    ClientConnection((Option<SessionID>, StreamToken, ClientTx)),

    #[strum(to_string = "ClientDisconnection")]
    ClientDisconnection((Option<SessionID>, StreamToken)),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstanceId(pub String);

impl InstanceId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientConnectMessage {
    pub client_id: ClientId,
    pub instance_id: InstanceId,
    pub connected_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub enum SessionMap {
    Single(
        (
            StreamToken,
            DateTime<Utc>,
            ClientTx,
            Arc<Mutex<ActiveNotification>>,
        ),
    ),
    Multi(FxHashMap<SessionID, (ClientTx, Arc<Mutex<ActiveNotification>>)>),
}

#[derive(Debug)]
pub struct ClientEntry {
    pub shard: Shard,
    pub last_read_id: Mutex<StreamEntry>,
    pub sessions: SessionMap,
}

pub type ReaderMap = DashMap<ClientId, ClientEntry, FxBuildHasher>;

#[derive(
    Debug, Clone, EnumString, EnumIter, Display, Serialize, Deserialize, Eq, Hash, PartialEq,
)]
pub enum TokenOrigin {
    DriverApp,
    RiderApp,
    Dashboard,
}

#[derive(Debug, Clone, Copy, Display, Serialize, Deserialize, Eq, PartialEq)]
pub enum DeliveryMode {
    Sweep,
    Pubsub,
}

impl DeliveryMode {
    pub fn needs_connect_catchup(&self) -> bool {
        matches!(self, DeliveryMode::Pubsub)
    }

    pub fn needs_independent_retry_loop(&self) -> bool {
        matches!(self, DeliveryMode::Pubsub)
    }
}

#[derive(Debug, Clone, Copy, Display, Serialize, Deserialize, Eq, PartialEq)]
pub enum DeliveryGuarantee {
    AtMostOnce,
    AtLeastOnce,
}

impl DeliveryGuarantee {
    pub fn removes_on_push(&self) -> bool {
        matches!(self, DeliveryGuarantee::AtMostOnce)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DeliveryPolicy {
    pub guarantee: DeliveryGuarantee,
    pub push_cap: Option<u32>,
}

impl DeliveryPolicy {
    pub fn new(guarantee: DeliveryGuarantee, max_delivery_attempts: Option<NonZeroU32>) -> Self {
        let push_cap = match guarantee {
            DeliveryGuarantee::AtMostOnce => Some(1),
            DeliveryGuarantee::AtLeastOnce => max_delivery_attempts.map(NonZeroU32::get),
        };
        DeliveryPolicy {
            guarantee,
            push_cap,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationMessage {
    pub stream_id: String,
    pub timestamp: DateTime<Utc>,
}
