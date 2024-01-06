/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
use crate::common::types::*;
use serde::Serialize;

#[derive(Serialize, Clone, PartialEq)]
pub enum NotificationStatus {
    DELIVERED,
    EXPIRED,
}

#[derive(Serialize)]
pub struct Notification {
    pub id: String,
    pub retries: u32,
    pub status: NotificationStatus,
    pub created_at: Timestamp,
    pub picked_at: Timestamp,
    pub delivered_at: Option<Timestamp>,
}