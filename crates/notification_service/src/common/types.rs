/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
use crate::NotificationPayload;
use chrono::{DateTime, Utc};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumIter, EnumString};
use tokio::sync::mpsc::Sender;
use tonic::Status;

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq)]
#[macros::impl_getter]
pub struct Token(pub String);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, Hash, PartialEq)]
#[macros::impl_getter]
pub struct ClientId(pub String);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, Hash, PartialEq)]
#[macros::impl_getter]
pub struct Shard(pub u64);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq, Hash)]
#[macros::impl_getter]
pub struct NotificationId(pub String);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[macros::impl_getter]
pub struct Timestamp(pub DateTime<Utc>);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq)]
#[macros::impl_getter]
pub struct StreamEntry(pub String);

#[derive(Deserialize, Serialize, Clone, Debug, Eq, Hash, PartialEq)]
#[macros::impl_getter]
pub struct ActiveNotificationCounter(pub u64);

impl ActiveNotificationCounter {
    pub fn increment(&mut self, num: u64) {
        self.0 += num;
    }
    pub fn decrement(&mut self, num: u64) {
        self.0 -= num;
    }
    pub fn reset(&mut self) {
        self.0 = 0;
    }
}

impl Default for StreamEntry {
    fn default() -> Self {
        Self("0-0".to_string())
    }
}

pub type ClientTx = Sender<Result<NotificationPayload, Status>>;

pub enum SenderType {
    ClientConnection(Option<ClientTx>),
    ClientAck,
}

pub type ReaderMap = FxHashMap<ClientId, (ActiveNotificationCounter, ClientTx)>;

#[derive(
    Debug, Clone, EnumString, EnumIter, Display, Serialize, Deserialize, Eq, Hash, PartialEq,
)]
pub enum TokenOrigin {
    DriverApp,
    RiderApp,
    DriverDashboard,
    RiderDashboard,
}
