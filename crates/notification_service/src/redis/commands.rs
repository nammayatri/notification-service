/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use super::{keys::*, types::NotificationData};
use crate::common::{
    types::*,
    utils::{abs_diff_utc_as_sec, decode_stream},
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use rustc_hash::FxHashMap;
use shared::redis::types::RedisConnectionPool;

pub async fn read_client_notifications(
    redis_pool: &RedisConnectionPool,
    clients_last_seen_notification_id: Vec<(ClientId, StreamEntry)>,
) -> Result<FxHashMap<String, Vec<NotificationData>>> {
    let (client_stream_keys, client_stream_ids): (Vec<String>, Vec<String>) =
        clients_last_seen_notification_id
            .into_iter()
            .map(|(ClientId(client_id), StreamEntry(last_entry))| (client_id, last_entry))
            .unzip();

    if !client_stream_keys.is_empty() {
        let notifications = redis_pool
            .xread(client_stream_keys.to_owned(), client_stream_ids.to_owned())
            .await?;
        Ok(decode_stream::<NotificationData>(notifications)?)
    } else {
        Ok(FxHashMap::default())
    }
}

pub async fn set_client_id(
    redis_pool: &RedisConnectionPool,
    auth_token_expiry: &u32,
    token: &str,
    ClientId(client_id): &ClientId,
) -> Result<()> {
    redis_pool
        .set_key_as_str(&set_client_id_key(token), client_id, *auth_token_expiry)
        .await?;
    Ok(())
}

pub async fn get_client_id(
    redis_pool: &RedisConnectionPool,
    token: &str,
) -> Result<Option<ClientId>> {
    Ok(redis_pool
        .get_key_as_str(&set_client_id_key(token))
        .await?
        .map(ClientId))
}

pub async fn set_notification_start_time(
    redis_pool: &RedisConnectionPool,
    notification_id: &str,
    notification_ttl: DateTime<Utc>,
) -> Result<()> {
    let now = Utc::now();
    redis_pool
        .set_key(
            &notification_duration_key(notification_id),
            Timestamp(now),
            abs_diff_utc_as_sec(now, notification_ttl) as u32,
        )
        .await?;
    Ok(())
}

pub async fn get_notification_start_time(
    redis_pool: &RedisConnectionPool,
    notification_id: &str,
) -> Result<Option<Timestamp>> {
    Ok(redis_pool
        .get_key::<Timestamp>(&notification_duration_key(notification_id))
        .await?)
}

pub async fn clean_up_notification(
    redis_pool: &RedisConnectionPool,
    client_id: &str,
    notification_id: &str,
) -> Result<()> {
    redis_pool
        .delete_key(&notification_duration_key(notification_id))
        .await?;
    redis_pool.xdel(client_id, notification_id).await?;
    Ok(())
}

pub async fn set_clients_last_sent_notification(
    redis_pool: &RedisConnectionPool,
    clients_last_seen_notification_id: Vec<(ClientId, StreamEntry)>,
    expiry_time: u32,
) -> Result<()> {
    for (ClientId(client_id), StreamEntry(last_seen_notification_id)) in
        clients_last_seen_notification_id
    {
        redis_pool
            .set_key(
                &set_last_sent_client_notification_key(&client_id),
                last_seen_notification_id,
                expiry_time,
            )
            .await?;
    }
    Ok(())
}

pub async fn get_client_last_sent_notification(
    redis_pool: &RedisConnectionPool,
    ClientId(client_id): &ClientId,
) -> Result<Option<StreamEntry>> {
    Ok(redis_pool
        .get_key::<StreamEntry>(&set_last_sent_client_notification_key(client_id))
        .await?)
}
