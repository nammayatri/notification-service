/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
use crate::{
    redis::{commands::read_client_notification, types::NotificationData},
    tools::prometheus::RWLOCK_DELAY,
    NotificationPayload,
};

use chrono::{DateTime, Utc};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use shared::redis::types::RedisConnectionPool;
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
#[macros::impl_getter]
pub struct ActiveNotification(pub FxHashMap<NotificationId, Ttl>);

impl ActiveNotification {
    pub async fn new(
        redis_pool: &RedisConnectionPool,
        client_id: &ClientId,
        shard: &Shard,
    ) -> Self {
        let mut counter = FxHashMap::default();
        if let Ok(notifications) = read_client_notification(redis_pool, client_id, shard).await {
            for notification in notifications {
                counter.insert(notification.id, notification.ttl);
            }
        }
        Self(counter)
    }

    pub fn update(&self, notifications: Vec<NotificationData>) -> Self {
        let mut counter = FxHashMap::default();
        for notification in notifications {
            counter.insert(notification.id, notification.ttl);
        }
        Self(counter)
    }

    pub fn count(&self) -> usize {
        self.inner().len()
    }

    pub fn acknowledge(&self, notification_id: &NotificationId) {
        self.inner().remove(notification_id);
    }

    pub fn refresh(&self) {
        let now = Utc::now();

        let expired_notifications: Vec<NotificationId> = self
            .inner()
            .iter()
            .filter(|(_, ttl)| (**ttl).inner() < now)
            .map(|(id, _)| id.to_owned())
            .collect();

        for id in expired_notifications {
            self.inner().remove(&id);
        }
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
