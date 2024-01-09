/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use crate::common::types::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq)]
pub struct NotificationData {
    pub stream_id: StreamEntry,
    pub id: NotificationId,
    pub category: String,
    pub title: String,
    pub body: String,
    pub show: bool,
    pub created_at: DateTime<Utc>,
    pub ttl: DateTime<Utc>,
    pub entity: EntityData,
}

#[derive(Deserialize, Serialize, Clone, Debug, Eq, PartialEq)]
pub struct EntityData {
    pub id: String,
    #[serde(rename = "type")]
    pub _type: String,
    pub data: String,
}
