/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
use std::time::Duration;

use rdkafka::{
    producer::{FutureProducer, FutureRecord},
    util::Timeout,
};
use serde::Serialize;

#[macros::add_error]
pub enum KafkaError {
    SerializationError(String),
    PushFailed(String),
}

pub async fn push_to_kafka<T>(
    producer: &Option<FutureProducer>,
    topic: &str,
    key: &str,
    message: T,
) -> Result<(), KafkaError>
where
    T: Serialize,
{
    let message = serde_json::to_string(&message)
        .map_err(|err| KafkaError::SerializationError(err.to_string()))?;

    match producer {
        Some(producer) => {
            producer
                .send(
                    FutureRecord::to(topic).key(key).payload(&message),
                    Timeout::After(Duration::from_secs(1)),
                )
                .await
                .map_err(|err| KafkaError::PushFailed(err.0.to_string()))?;

            Ok(())
        }
        None => Err(KafkaError::PushFailed(
            "[Kafka] Producer is None, unable to send message".to_string(),
        )),
    }
}
