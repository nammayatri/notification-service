/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use std::{cmp::Ordering, str::FromStr};

use crate::{redis::types::NotificationData, tools::error::AppError, Entity, NotificationPayload};
use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use rustc_hash::FxHashMap;
use serde::de::DeserializeOwned;
use serde_json::json;
use uuid::Uuid;

use super::types::Timestamp;

pub fn decode_nested_json<T: DeserializeOwned>(payload: Vec<(String, String)>) -> Result<T> {
    let mut json_obj = json!({});
    for (key, value) in payload {
        let parts: Vec<&str> = key.split('.').collect();
        let mut current_obj = &mut json_obj;

        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                current_obj[part] = serde_json::Value::String(value.to_string());
            } else {
                if !current_obj
                    .as_object()
                    .ok_or_else(|| {
                        AppError::InternalError("Error in decode_nested_json.".to_string())
                    })?
                    .contains_key(*part)
                {
                    current_obj[part] = json!({});
                }
                current_obj = current_obj.get_mut(part).ok_or_else(|| {
                    AppError::InternalError("Error in decode_nested_json.".to_string())
                })?;
            }
        }
    }

    // Deserialize JSON into Rust type
    let payload = serde_json::from_value::<T>(json_obj)?;

    Ok(payload)
}

pub fn decode_stream<T>(
    notifications: FxHashMap<String, Vec<Vec<(String, String)>>>,
) -> Result<FxHashMap<String, Vec<T>>>
where
    T: DeserializeOwned,
{
    let mut result = FxHashMap::default();

    for (key, notifications) in notifications {
        let mut payloads = Vec::new();

        for notification in notifications {
            let payload = decode_nested_json::<T>(notification)?;
            payloads.push(payload);
        }

        result.insert(key, payloads);
    }

    Ok(result)
}

pub fn is_stream_id_less_or_eq(id1: &str, id2: &str) -> bool {
    // Split the stream IDs into timestamp and sequence parts
    let parts1: Vec<&str> = id1.split('-').collect();
    let parts2: Vec<&str> = id2.split('-').collect();

    // Parse timestamp and sequence as integers
    match (
        parts1.first().and_then(|&s| s.parse::<u64>().ok()),
        parts1.get(1).and_then(|&s| s.parse::<u64>().ok()),
        parts2.first().and_then(|&s| s.parse::<u64>().ok()),
        parts2.get(1).and_then(|&s| s.parse::<u64>().ok()),
    ) {
        (Some(ts1), Some(seq1), Some(ts2), Some(seq2)) => match ts1.cmp(&ts2) {
            Ordering::Less => true,
            Ordering::Equal => seq1 <= seq2,
            Ordering::Greater => false,
        },
        _ => false,
    }
}

pub fn abs_diff_utc_as_sec(old: DateTime<Utc>, new: DateTime<Utc>) -> u64 {
    new.signed_duration_since(old).num_seconds().abs_diff(0)
}

pub fn transform_notification_data_to_payload(
    notification: NotificationData,
) -> NotificationPayload {
    let entity = Entity {
        id: notification.entity.id,
        r#type: notification.entity._type,
        data: notification.entity.data,
    };
    NotificationPayload {
        id: notification.id.inner(),
        category: notification.category,
        title: notification.title,
        body: notification.body,
        show: notification.show,
        entity: Some(entity),
    }
}

#[allow(deprecated)]
pub fn get_timestamp_from_stream_id(stream_id: &str) -> Timestamp {
    stream_id
        .split('-')
        .next()
        .and_then(|timestamp_part| timestamp_part.parse::<i64>().ok())
        .map(|milliseconds| Utc.timestamp(milliseconds / 1000, 0))
        .map(Timestamp)
        .unwrap_or_else(|| Timestamp(Utc::now()))
}

pub fn hash_uuid(uuid_str: &str) -> u64 {
    let uuid = Uuid::from_str(uuid_str).unwrap_or_default();
    let (word1, word2) = uuid.as_u64_pair();
    word1 + word2
}
