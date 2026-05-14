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
use std::sync::Arc;
use strum_macros::{Display, EnumIter, EnumString};
use tokio::sync::mpsc::Sender;
use tonic::Status;

pub type FxBuildHasher = BuildHasherDefault<FxHasher>;

#[derive(Debug, Clone)]
pub struct ExpiredMeta {
    pub category: String,
    pub reason: &'static str,
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
    pub stream_id: StreamEntry,
    pub sent_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
#[macros::impl_getter]
pub struct ActiveNotification(pub FxHashMap<NotificationId, NotificationMeta>);

impl ActiveNotification {
    pub fn update(&mut self, notifications: Vec<NotificationData>) {
        for notification in notifications {
            self.0
                .entry(notification.id.clone())
                .or_insert(NotificationMeta {
                    data: notification,
                    sent_at: None,
                    total_counted: false,
                    retry_counted: false,
                    expired_counted: false,
                });
        }
    }

    pub fn count(&self) -> usize {
        self.0.len()
    }

    pub fn acknowledge(
        &mut self,
        notification_id: &NotificationId,
    ) -> Option<AcknowledgedNotification> {
        self.0
            .remove(notification_id)
            .map(|meta| AcknowledgedNotification {
                category: meta.data.category,
                stream_id: meta.data.stream_id,
                sent_at: meta.sent_at,
            })
    }

    pub fn try_claim_total(&mut self, notification: &NotificationData) -> bool {
        match self.0.get_mut(&notification.id) {
            None => {
                self.0.insert(
                    notification.id.clone(),
                    NotificationMeta {
                        data: notification.clone(),
                        sent_at: None,
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
        if let Some(meta) = self.0.get_mut(notification_id) {
            meta.sent_at = Some(now);
        }
    }

    pub fn try_claim_retry(&mut self, notification_id: &NotificationId) -> bool {
        match self.0.get_mut(notification_id) {
            Some(meta) if meta.total_counted && !meta.retry_counted => {
                meta.retry_counted = true;
                true
            }
            _ => false,
        }
    }

    pub fn try_claim_expired(&mut self, notification_id: &NotificationId) -> bool {
        match self.0.get_mut(notification_id) {
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
        match self.0.get_mut(notification_id) {
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
        self.0.retain(|_, meta| meta.data.ttl.inner() >= now);
    }

    pub fn pending_redelivery(&self) -> Vec<NotificationData> {
        self.0.values().map(|m| m.data.clone()).collect()
    }
}

impl Default for StreamEntry {
    fn default() -> Self {
        Self("0-0".to_string())
    }
}

pub type ClientTx = Sender<Result<NotificationPayload, Status>>;

#[derive(Display)]
pub enum SenderType {
    #[strum(to_string = "ClientConnection")]
    ClientConnection((Option<SessionID>, ClientTx)),

    #[strum(to_string = "ClientDisconnection")]
    ClientDisconnection(Option<SessionID>),
}

#[derive(Clone, Debug)]
pub enum SessionMap {
    Single((ClientTx, Arc<Mutex<ActiveNotification>>)),
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
    DriverDashboard,
    RiderDashboard,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationMessage {
    pub stream_id: String,
    pub timestamp: DateTime<Utc>,
}
