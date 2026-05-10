/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
use crate::{redis::types::NotificationData, tools::prometheus::RWLOCK_DELAY, NotificationPayload};

use chrono::{DateTime, Utc};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use strum_macros::{Display, EnumIter, EnumString};
use tokio::sync::{mpsc::Sender, RwLock};
use tokio::sync::{RwLockReadGuard, RwLockWriteGuard, TryLockError};
use tokio::time::Instant;
use tonic::Status;

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

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq)]
pub struct NotificationMeta {
    pub ttl: Ttl,
    pub category: String,
    pub stream_id: StreamEntry,
    pub retry_counted: bool,
}

#[derive(Debug, Clone)]
pub struct AcknowledgedNotification {
    pub category: String,
    pub stream_id: StreamEntry,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum NotificationObservation {
    FirstSighting,
    FirstRetry,
    AlreadyCounted,
}

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq, Default)]
#[macros::impl_getter]
pub struct ActiveNotification(pub FxHashMap<NotificationId, NotificationMeta>);

impl ActiveNotification {
    pub fn update(&mut self, notifications: Vec<NotificationData>) {
        for notification in notifications {
            self.0.entry(notification.id).or_insert(NotificationMeta {
                ttl: notification.ttl,
                category: notification.category,
                stream_id: notification.stream_id,
                retry_counted: false,
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
                category: meta.category,
                stream_id: meta.stream_id,
            })
    }

    pub fn observe_for_retry_metric(
        &mut self,
        notification: &NotificationData,
    ) -> NotificationObservation {
        match self.0.get_mut(&notification.id) {
            None => {
                self.0.insert(
                    notification.id.clone(),
                    NotificationMeta {
                        ttl: notification.ttl.clone(),
                        category: notification.category.clone(),
                        stream_id: notification.stream_id.clone(),
                        retry_counted: false,
                    },
                );
                NotificationObservation::FirstSighting
            }
            Some(meta) if !meta.retry_counted => {
                meta.retry_counted = true;
                NotificationObservation::FirstRetry
            }
            Some(_) => NotificationObservation::AlreadyCounted,
        }
    }

    pub fn refresh(&mut self) {
        let now = Utc::now();
        self.0.retain(|_, meta| meta.ttl.inner() >= now);
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

    #[strum(to_string = "ClientAck")]
    ClientAck((NotificationId, Option<SessionID>)),
}

#[derive(Clone, Debug)]
pub enum SessionMap {
    Single((ClientTx, Arc<MonitoredRwLock<ActiveNotification>>)), // A single session with a (ClientTx, Counter)
    Multi(FxHashMap<SessionID, (ClientTx, Arc<MonitoredRwLock<ActiveNotification>>)>), // Multiple sessions
}

pub type ReaderMap = FxHashMap<ClientId, SessionMap>;

#[derive(
    Debug, Clone, EnumString, EnumIter, Display, Serialize, Deserialize, Eq, Hash, PartialEq,
)]
pub enum TokenOrigin {
    DriverApp,
    RiderApp,
    DriverDashboard,
    RiderDashboard,
}

#[derive(Debug)]
pub enum RwLockName {
    ActiveNotificationStore,
    ClientTxStore,
}

impl RwLockName {
    fn as_str(&self) -> &'static str {
        match self {
            RwLockName::ActiveNotificationStore => "active-notification-store",
            RwLockName::ClientTxStore => "client-tx-store",
        }
    }
}

#[derive(Debug)]
pub enum RwLockOperation {
    Read,
    Write,
    TryRead,
    TryWrite,
}

impl RwLockOperation {
    fn as_str(&self) -> &'static str {
        match self {
            RwLockOperation::Read => "read",
            RwLockOperation::Write => "write",
            RwLockOperation::TryRead => "try_read",
            RwLockOperation::TryWrite => "try_write",
        }
    }
}

#[derive(Debug)]
pub struct MonitoredRwLock<T> {
    lock: RwLock<T>,
}

impl<T> MonitoredRwLock<T> {
    pub fn new(data: T) -> Self {
        Self {
            lock: RwLock::new(data),
        }
    }

    pub async fn read(
        &self,
        name: RwLockName,
        operation_type: RwLockOperation,
    ) -> RwLockReadGuard<'_, T> {
        let start = Instant::now();
        let guard = self.lock.read().await;
        let duration = start.elapsed().as_secs_f64();
        RWLOCK_DELAY
            .with_label_values(&[name.as_str(), operation_type.as_str()])
            .observe(duration);
        guard
    }

    pub async fn write(
        &self,
        name: RwLockName,
        operation_type: RwLockOperation,
    ) -> RwLockWriteGuard<'_, T> {
        let start = Instant::now();
        let guard = self.lock.write().await;
        let duration = start.elapsed().as_secs_f64();
        RWLOCK_DELAY
            .with_label_values(&[name.as_str(), operation_type.as_str()])
            .observe(duration);
        guard
    }

    pub fn try_read(
        &self,
        name: RwLockName,
        operation_type: RwLockOperation,
    ) -> Result<RwLockReadGuard<'_, T>, TryLockError> {
        let start = Instant::now();

        match self.lock.try_read() {
            Ok(guard) => {
                let duration = start.elapsed().as_secs_f64();
                RWLOCK_DELAY
                    .with_label_values(&[name.as_str(), operation_type.as_str()])
                    .observe(duration);
                Ok(guard)
            }
            Err(err) => Err(err),
        }
    }

    pub fn try_write(
        &self,
        name: RwLockName,
        operation_type: RwLockOperation,
    ) -> Result<RwLockWriteGuard<'_, T>, TryLockError> {
        let start = Instant::now();

        match self.lock.try_write() {
            Ok(guard) => {
                let duration = start.elapsed().as_secs_f64();
                RWLOCK_DELAY
                    .with_label_values(&[name.as_str(), operation_type.as_str()])
                    .observe(duration);
                Ok(guard)
            }
            Err(err) => Err(err),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationMessage {
    pub stream_id: String,
    pub timestamp: DateTime<Utc>,
}
