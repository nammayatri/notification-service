/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use uuid::Uuid;

pub const CATEGORY: &str = "NEW_RIDE_AVAILABLE";
const ENTITY_TYPE: &str = "SearchRequest";
const PAD_SCAFFOLD_BYTES: usize = r#","pad":"""#.len();

pub fn iso8601(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

pub struct Offer {
    pub notification_id: String,
    pub fields: Vec<(String, String)>,
}

pub fn build_offer(
    search_request_id: &Uuid,
    created_at: DateTime<Utc>,
    ttl_seconds: i64,
    payload_bytes: usize,
) -> Offer {
    let notification_id = Uuid::new_v4().to_string();
    let ttl = created_at + Duration::seconds(ttl_seconds);

    let fields = vec![
        ("id".to_string(), notification_id.clone()),
        ("category".to_string(), CATEGORY.to_string()),
        (
            "title".to_string(),
            "New ride available for offering".to_string(),
        ),
        (
            "body".to_string(),
            format!(
                "A new ride for {} is available 316 meters away, estimated fare 137",
                created_at.format("%d %b, %I:%M %p")
            ),
        ),
        ("show".to_string(), "SHOW".to_string()),
        ("created_at".to_string(), iso8601(created_at)),
        ("ttl".to_string(), iso8601(ttl)),
        ("entity.type".to_string(), ENTITY_TYPE.to_string()),
        ("entity.id".to_string(), search_request_id.to_string()),
        (
            "entity.data".to_string(),
            entity_data(search_request_id, created_at, ttl, payload_bytes),
        ),
    ];

    Offer {
        notification_id,
        fields,
    }
}

fn entity_data(
    search_request_id: &Uuid,
    created_at: DateTime<Utc>,
    ttl: DateTime<Utc>,
    payload_bytes: usize,
) -> String {
    let mut data = json!({
        "searchRequestId": search_request_id.to_string(),
        "searchRequestValidTill": iso8601(ttl),
        "startTime": iso8601(created_at + Duration::seconds(420)),
        "baseFare": 137.0,
        "distance": 5400,
        "distanceToPickup": 316,
        "durationToPickup": 121,
        "fromLocation": {
            "lat": 12.9351929,
            "lon": 77.6244807,
            "area": "Koramangala",
            "city": "Bangalore",
            "full_address": "1st Main Rd, 4th Block, Koramangala, Bengaluru, Karnataka 560034",
        },
        "toLocation": {
            "lat": 12.9715987,
            "lon": 77.5945627,
            "area": "Vidhana Soudha",
            "city": "Bangalore",
            "full_address": "Dr Ambedkar Veedhi, Sampangi Rama Nagara, Bengaluru, Karnataka 560001",
        },
        "produced_at_ms": created_at.timestamp_millis(),
    });

    let base_len = data.to_string().len();
    let pad_len = payload_bytes.saturating_sub(base_len + PAD_SCAFFOLD_BYTES);
    if pad_len > 0 {
        data["pad"] = json!("x".repeat(pad_len));
    }
    data.to_string()
}

pub fn pubsub_message(client_id: &str, timestamp: DateTime<Utc>) -> String {
    json!({ "streamId": client_id, "timestamp": iso8601(timestamp) }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use notification_service::{
        common::types::NotificationMessage, redis::types::NotificationData,
    };

    fn decode(fields: Vec<(String, String)>) -> NotificationData {
        let mut with_stream_id = vec![("stream_id".to_string(), "1734512893001-0".to_string())];
        with_stream_id.extend(fields);
        notification_service::common::utils::decode_nested_json::<NotificationData>(with_stream_id)
            .expect("service must be able to decode the produced fields")
    }

    #[test]
    fn fields_deserialize_into_the_service_notification_data() {
        let search_request_id = Uuid::new_v4();
        let created_at = Utc::now();
        let offer = build_offer(&search_request_id, created_at, 30, 1024);

        let decoded = decode(offer.fields);
        assert_eq!(decoded.id.0, offer.notification_id);
        assert_eq!(decoded.category, CATEGORY);
        assert_eq!(decoded.entity._type, ENTITY_TYPE);
        assert_eq!(decoded.entity.id, search_request_id.to_string());
        assert_eq!(
            decoded.ttl.0.timestamp_millis(),
            (created_at + Duration::seconds(30)).timestamp_millis()
        );
    }

    #[test]
    fn entity_data_reaches_the_requested_size() {
        let offer = build_offer(&Uuid::new_v4(), Utc::now(), 30, 2048);
        let data = offer
            .fields
            .iter()
            .find(|(key, _)| key == "entity.data")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        assert_eq!(data.len(), 2048);
    }

    #[test]
    fn pubsub_message_matches_the_service_shape() {
        let client_id = "59f9c690-caa0-5595-aa99-8d80f824e223";
        let raw = pubsub_message(client_id, Utc::now());
        let decoded: NotificationMessage =
            serde_json::from_str(&raw).expect("service must be able to decode the pubsub message");
        assert_eq!(decoded.stream_id, client_id);
    }
}
