/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
use super::types::*;
use crate::{
    common::{kafka::push_to_kafka, types::*},
    domain::types::ui::location::UpdateDriverLocationRequest,
};
use chrono::Utc;
use log::error;
use rdkafka::producer::FutureProducer;

#[allow(clippy::too_many_arguments)]
pub async fn kafka_stream_notification_updates(
    producer: &Option<FutureProducer>,
    topic: &str,
    locations: Vec<UpdateDriverLocationRequest>,
    merchant_id: MerchantId,
    ride_id: Option<RideId>,
    ride_status: Option<RideStatus>,
    driver_mode: DriverMode,
    ClientId(key): &ClientId,
) {
    let ride_status = match ride_status {
        Some(RideStatus::NEW) => DriverRideStatus::OnPickup,
        Some(RideStatus::INPROGRESS) => DriverRideStatus::OnRide,
        _ => DriverRideStatus::IDLE,
    };

    for loc in locations {
        let message = LocationUpdate {
            rid: ride_id.to_owned(),
            mid: merchant_id.to_owned(),
            ts: loc.ts,
            st: TimeStamp(Utc::now()),
            lat: loc.pt.lat,
            lon: loc.pt.lon,
            speed: loc.v.unwrap_or(SpeedInMeterPerSecond(0.0)),
            acc: loc.acc.unwrap_or(Accuracy(0.0)),
            ride_status: ride_status.to_owned(),
            on_ride: ride_status != DriverRideStatus::IDLE,
            active: true,
            mode: driver_mode.to_owned(),
        };
        if let Err(err) = push_to_kafka(producer, topic, key.as_str(), message).await {
            error!("Error occured in push_to_kafka => {}", err.message())
        }
    }
}
