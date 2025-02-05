/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
use crate::{
    redis::{commands::read_client_notification, types::NotificationData},
    NotificationPayload,
};
use chrono::{DateTime, Utc};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use shared::redis::types::RedisConnectionPool;
use std::sync::Arc;
use strum_macros::{Display, EnumIter, EnumString};
use tokio::sync::{mpsc::Sender, RwLock};
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

pub enum SenderType {
    ClientConnection((Option<SessionID>, ClientTx)),
    ClientDisconnection(Option<SessionID>),
    ClientAck((NotificationId, Option<SessionID>)),
}

#[derive(Clone, Debug)]
pub enum SessionMap {
    Single((ClientTx, Arc<RwLock<ActiveNotification>>)), // A single session with a (ClientTx, Counter)
    Multi(FxHashMap<SessionID, (ClientTx, Arc<RwLock<ActiveNotification>>)>), // Multiple sessions
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
