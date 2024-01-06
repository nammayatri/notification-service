/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
use super::types::*;
use crate::{common::types::*, tools::kafka::push_to_kafka};
use rdkafka::producer::FutureProducer;
use tracing::*;

#[allow(clippy::too_many_arguments)]
pub async fn kafka_stream_notification_updates(
    producer: &Option<FutureProducer>,
    topic: &str,
    client_id: &str,
    notification_id: String,
    retries: u32,
    status: NotificationStatus,
    created_at: Timestamp,
    picked_at: Timestamp,
    delivered_at: Option<Timestamp>,
) {
    let message = Notification {
        id: notification_id,
        retries,
        status,
        created_at,
        picked_at,
        delivered_at,
    };
    if let Err(err) = push_to_kafka(producer, topic, client_id, message).await {
        error!("Error occured in push_to_kafka => {:?}", err)
    }
}
