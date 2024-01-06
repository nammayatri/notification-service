/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

pub fn set_client_id_key(token: &str) -> String {
    format!("notification:client_id:{token}")
}

pub fn set_last_sent_client_notification_key(client_id: &str) -> String {
    format!("notification:last_sent_client_notification:{client_id}")
}

pub fn notification_duration_key(notification_id: &str) -> String {
    format!("notifition:duration:{}", notification_id)
}
