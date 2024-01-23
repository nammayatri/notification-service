/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

pub fn notification_client_key(client_id: &str, shard: u64) -> String {
    format!("notification:client-{}:{{{shard}}}", client_id)
}

pub fn client_details_key(token: &str) -> String {
    format!("notification:client_details:{token}")
}

pub fn last_sent_client_notification_key(client_id: &str) -> String {
    format!("notification:last_sent_client_notification:{client_id}")
}

pub fn notification_stream_key(notification_id: &str) -> String {
    format!("notifition:stream:{}", notification_id)
}
