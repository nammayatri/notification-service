/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use super::{keys::*, types::NotificationData};
use crate::{
    common::{
        types::*,
        utils::{abs_diff_utc_as_sec, decode_stream},
    },
    tools::prometheus::MEASURE_DURATION,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use regex::Regex;
use shared::measure_latency_duration;
use shared::redis::{error::RedisError, types::RedisConnectionPool};
use std::cmp::{max, min};
use tracing::*;

#[macros::measure_duration]
pub async fn set_client_id(
    redis_pool: &RedisConnectionPool,
    auth_token_expiry: &u32,
    token: &str,
    ClientId(client_id): &ClientId,
) -> Result<()> {
    redis_pool
        .set_key_as_str(&client_details_key(token), client_id, *auth_token_expiry)
        .await?;
    Ok(())
}

#[macros::measure_duration]
pub async fn get_client_id(
    redis_pool: &RedisConnectionPool,
    token: &str,
) -> Result<Option<ClientId>> {
    Ok(redis_pool
        .get_key_as_str(&client_details_key(token))
        .await?
        .map(ClientId))
}

#[macros::measure_duration]
pub async fn read_client_notification(
    redis_pool: &RedisConnectionPool,
    client_id: &ClientId,
    Shard(shard): &Shard,
) -> Result<Vec<NotificationData>> {
    let notifications = redis_pool
        .xread(
            vec![notification_client_key(&client_id.inner(), shard)],
            vec![StreamEntry::default().inner()],
            None,
        )
        .await?;

    Ok(decode_stream::<NotificationData>(notifications)?
        .into_values()
        .flatten()
        .collect())
}

#[macros::measure_duration]
pub async fn read_client_notifications(
    redis_pool: &RedisConnectionPool,
    client_ids: Vec<ClientId>,
    Shard(shard): &Shard,
) -> Result<Vec<(ClientId, Vec<NotificationData>)>> {
    if client_ids.is_empty() {
        return Ok(Vec::default());
    }

    let (client_stream_keys, client_stream_ids): (Vec<String>, Vec<String>) = client_ids
        .into_iter()
        .map(|ClientId(client_id)| {
            (
                notification_client_key(&client_id, shard),
                StreamEntry::default().inner(),
            )
        })
        .unzip();

    let notifications = redis_pool
        .xread(
            client_stream_keys.to_owned(),
            client_stream_ids.to_owned(),
            None,
        )
        .await?;
    let notifications = decode_stream::<NotificationData>(notifications)?;

    let regex = match Regex::new(r"N([0-9a-fA-F-]+)\{") {
        Ok(regex) => regex,
        Err(err) => {
            error!("Regex Parsing Failed: {}", err);
            return Err(err.into());
        }
    };

    let mut result = Vec::default();
    for (key, val) in notifications {
        match regex
            .captures(&key)
            .and_then(|captures| captures.get(1).map(|m| m.as_str()))
        {
            Some(client_id) => result.push((ClientId(client_id.to_string()), val)),
            None => {
                error!("Regex Match Failed For Key: {}, Shard: {}", key, shard);
                continue;
            }
        }
    }

    Ok(result)
}

#[macros::measure_duration]
pub async fn set_notification_stream_id(
    redis_pool: &RedisConnectionPool,
    notification_id: &str,
    notification_stream_id: &str,
    notification_ttl: DateTime<Utc>,
) -> Result<(), RedisError> {
    let now = Utc::now();
    redis_pool
        .set_key_as_str(
            &notification_stream_key(notification_id),
            notification_stream_id,
            abs_diff_utc_as_sec(min(now, notification_ttl), max(now, notification_ttl)) as u32 + 60, // Extra 60 seconds buffer
        )
        .await?;
    Ok(())
}

#[macros::measure_duration]
pub async fn get_notification_stream_id(
    redis_pool: &RedisConnectionPool,
    notification_id: &str,
) -> Result<Option<StreamEntry>> {
    Ok(redis_pool
        .get_key_as_str(&notification_stream_key(notification_id))
        .await?
        .map(StreamEntry))
}

#[macros::measure_duration]
pub async fn clean_up_notification(
    redis_pool: &RedisConnectionPool,
    client_id: &str,
    notification_stream_id: &str,
    Shard(shard): &Shard,
) -> Result<()> {
    redis_pool
        .xdel(
            &notification_client_key(client_id, shard),
            notification_stream_id,
        )
        .await?;
    Ok(())
}

#[macros::measure_duration]
pub async fn remove_notification_by_id(
    redis_pool: &RedisConnectionPool,
    client_id: &str,
    notification_id: &str,
    Shard(shard): &Shard,
) -> Result<bool> {
    // First, get the notification stream ID for this notification ID
    if let Some(notification_stream_id) =
        get_notification_stream_id(redis_pool, notification_id).await?
    {
        // Remove from the client's notification stream
        redis_pool
            .xdel(
                &notification_client_key(client_id, shard),
                &notification_stream_id.inner(),
            )
            .await?;

        // Clean up the notification stream ID mapping
        redis_pool
            .delete_key(&notification_stream_key(notification_id))
            .await?;

        Ok(true)
    } else {
        // Notification ID not found
        Ok(false)
    }
}
