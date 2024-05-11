/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

#[tokio::test]
async fn loadtest() {
    use std::{collections::HashMap, sync::Arc, time::Duration};

    use anyhow::anyhow;
    use chrono::Utc;
    use lazy_static::lazy_static;
    use rand::{distributions::Alphanumeric, rngs::StdRng, Rng, SeedableRng};
    use reqwest::Client;
    use serde_json::{json, Value};
    use tokio::sync::RwLock;

    lazy_static! {
        static ref NOTIFICATION_DETAILS: Arc<RwLock<Vec<(String, String, String)>>> =
            Arc::new(RwLock::new(Vec::new()));
    }

    const GRPC_BASE_URL: &str = "https://beta.beckn.uat.juspay.net:50051";
    const DRIVER_BASE_URL: &str = "https://api.sandbox.beckn.juspay.in/dev/dobpp/ui";
    const RIDER_BASE_URL: &str = "https://api.sandbox.beckn.juspay.in/dev/app/v2";
    const DRIVER_MOBILE_NUMBERS: [&str; 10] = [
        "9642420000",
        "9876544447",
        "9876544457",
        "9344676990",
        "9876544449",
        "8123456780",
        "8123456789",
        "9491839513",
        "9876544448",
        "9876544459",
    ];

    fn generate_random_string(length: usize, numeric: bool) -> String {
        // Initialize the random number generator
        let rng = StdRng::from_entropy();

        // Define the character set based on the format
        let charset = if numeric {
            "0123456789"
        } else {
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
        };

        // Generate a random string of the specified length
        String::from_utf8(
            rng.sample_iter(Alphanumeric)
                .filter(|c| charset.contains(*c as char))
                .take(length)
                .collect::<Vec<u8>>(),
        )
        .unwrap()
    }

    async fn driver_auth(mobile_number: String) -> anyhow::Result<String> {
        let client = Client::new();

        let request_data = json!({
            "mobileNumber": mobile_number,
            "mobileCountryCode": "+91",
            "merchantId": "7f7896dd-787e-4a0b-8675-e9e6fe93bb8f"
        });

        let response = client
            .post(DRIVER_BASE_URL.to_string() + "/auth")
            .header("Content-Type", "application/json")
            .json(&request_data)
            .send()
            .await?;

        if response.status().is_success() {
            let response_body: Value = response.json().await?;

            let auth_id = response_body["authId"]
                .to_string()
                .trim_matches('"')
                .to_string();

            // println!("Got authId = {}", auth_id);

            let request_data = json!({
                "otp": "7891",
                "deviceToken": generate_random_string(35, false)
            });

            let response = client
                .post(DRIVER_BASE_URL.to_string() + &format!("/auth/{}/verify", auth_id))
                .header("Content-Type", "application/json")
                .json(&request_data)
                .send()
                .await?;

            if response.status().is_success() {
                let response_body: Value = response.json().await?;
                Ok(response_body["token"]
                    .to_string()
                    .trim_matches('"')
                    .to_string())
            } else {
                let response_body: Value = response.json().await?;
                Err(anyhow!("Request failed with error: {}", response_body))
            }
        } else {
            let response_body: Value = response.json().await?;
            Err(anyhow!("Request failed with error: {}", response_body))
        }
    }

    async fn set_online(token: String) -> anyhow::Result<()> {
        let client = Client::new();

        let response = client
            .post(DRIVER_BASE_URL.to_string() + "/driver/setActivity?active=true&mode=%22ONLINE%22")
            .header("Content-Type", "application/json")
            .header("token", token)
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let response_body: Value = response.json().await?;
            Err(anyhow!("Request failed with error: {}", response_body))
        }
    }

    async fn set_driver_location(token: String) -> anyhow::Result<()> {
        let client = Client::new();

        let request_data = json!([{
            "pt": {
                 "lat": 12.93725051371234,
                 "lon": 77.62683560765991
            },
            "ts": &(Utc::now()).format("%Y-%m-%dT%H:%M:%S%.fZ").to_string(),
            "acc": 1,
            "v": 5
        }]);

        let response = client
            .post(DRIVER_BASE_URL.to_string() + "/driver/location")
            .header("Content-Type", "application/json")
            .header("vt", "AUTO_RICKSHAW")
            .header("mId", "7f7896dd-787e-4a0b-8675-e9e6fe93bb8f")
            .header("dm", "ONLINE")
            .header("token", token)
            .json(&request_data)
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let response_body: Value = response.json().await?;
            Err(anyhow!("Request failed with error: {}", response_body))
        }
    }

    async fn rider_auth() -> anyhow::Result<String> {
        let client = Client::new();

        let request_data = json!({
            "mobileNumber": generate_random_string(10, true),
            "mobileCountryCode": "+91",
            "merchantId": "NAMMA_YATRI"
        });

        let response = client
            .post(RIDER_BASE_URL.to_string() + "/auth")
            .header("Content-Type", "application/json")
            .json(&request_data)
            .send()
            .await?;

        if response.status().is_success() {
            let response_body: Value = response.json().await?;

            let auth_id = response_body["authId"]
                .to_string()
                .trim_matches('"')
                .to_string();

            // println!("Got authId = {}", auth_id);

            let request_data = json!({
                "otp": "7891",
                "deviceToken": generate_random_string(35, false)
            });

            let response = client
                .post(RIDER_BASE_URL.to_string() + &format!("/auth/{}/verify", auth_id))
                .header("Content-Type", "application/json")
                .json(&request_data)
                .send()
                .await?;

            if response.status().is_success() {
                let response_body: Value = response.json().await?;
                Ok(response_body["token"]
                    .to_string()
                    .trim_matches('"')
                    .to_string())
            } else {
                let response_body: Value = response.json().await?;
                Err(anyhow!("Request failed with error: {}", response_body))
            }
        } else {
            let response_body: Value = response.json().await?;
            Err(anyhow!("Request failed with error: {}", response_body))
        }
    }

    async fn ride_search(token: String) -> anyhow::Result<String> {
        let client = Client::new();

        let request_data = json!({
            "fareProductType": "ONE_WAY",
            "contents": {
                "startTime": &(Utc::now()).format("%Y-%m-%dT%H:%M:%S%.fZ").to_string(),
                "origin": {
                    "address": {
                        "area": "8th Block Koramangala",
                        "areaCode": "560047",
                        "building": "Juspay Buildings",
                        "city": "Bangalore",
                        "country": "India",
                        "door": "#444",
                        "street": "18th Main",
                        "state": "Karnataka"
                    },
                    "gps": {
                            "lat": 12.93725051371234,
                            "lon": 77.62683560765991
                        }
                },
                "destination": {
                    "address": {
                        "area": "6th Block Koramangala",
                        "areaCode": "560047",
                        "building": "Juspay Apartments",
                        "city": "Bangalore",
                        "country": "India",
                        "door": "#444",
                        "street": "18th Main",
                        "state": "Karnataka"
                    },
                    "gps": {
                            "lat": 13.193900216321593,
                            "lon": 77.69868705070557
                        }
                }
            }
        });

        let response = client
            .post(RIDER_BASE_URL.to_string() + "/rideSearch")
            .header("Content-Type", "application/json")
            .header("token", token)
            .json(&request_data)
            .send()
            .await?;

        if response.status().is_success() {
            let response_body: Value = response.json().await?;
            Ok(response_body["searchId"]
                .to_string()
                .trim_matches('"')
                .to_string())
        } else {
            let response_body: Value = response.json().await?;
            Err(anyhow!("Request failed with error: {}", response_body))
        }
    }

    async fn get_estimate_id(token: String, search_id: String) -> anyhow::Result<String> {
        let client = Client::new();

        let response = client
            .get(RIDER_BASE_URL.to_string() + &format!("/rideSearch/{}/results", search_id))
            .header("Content-Type", "application/json")
            .header("token", token)
            .send()
            .await?;

        if response.status().is_success() {
            let response_body: Value = response.json().await?;
            Ok(response_body["estimates"][0]["id"]
                .to_string()
                .trim_matches('"')
                .to_string())
        } else {
            let response_body: Value = response.json().await?;
            Err(anyhow!("Request failed with error: {}", response_body))
        }
    }

    async fn select_estimate(token: String, estimate_id: String) -> anyhow::Result<()> {
        let client = Client::new();

        let request_data = json!({ "autoAssignEnabledV2" : true, "autoAssignEnabled" : true });

        let response = client
            .post(RIDER_BASE_URL.to_string() + &format!("/estimate/{}/select2", estimate_id))
            .header("Content-Type", "application/json")
            .header("token", token)
            .json(&request_data)
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            let response_body: Value = response.json().await?;
            Err(anyhow!("Request failed with error: {}", response_body))
        }
    }

    async fn driver_notification_reciever(token: String) -> anyhow::Result<()> {
        let mut attempt_count = 0;

        loop {
            let result: anyhow::Result<()> = async {
                use std::str::FromStr;

                let mut client =
                    notification_service::notification_client::NotificationClient::connect(
                        GRPC_BASE_URL,
                    )
                    .await?;

                let mut metadata = tonic::metadata::MetadataMap::new();
                metadata.insert("token", tonic::metadata::MetadataValue::from_str(&token)?);

                let (tx, rx) = tokio::sync::mpsc::channel(100000);

                let mut request =
                    tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
                *request.metadata_mut() = metadata;
                let response = client.stream_payload(request).await?;
                let mut inbound = response.into_inner();

                while let Some(response) = tokio_stream::StreamExt::next(&mut inbound).await {
                    let notification = response?;
                    let entity =
                        serde_json::from_str::<Value>(&notification.clone().entity.unwrap().data)
                            .unwrap();
                    let current_time = Utc::now();
                    NOTIFICATION_DETAILS.write().await.push((
                        token.to_string(),
                        (current_time).format("%Y-%m-%dT%H:%M:%S%.fZ").to_string(),
                        entity["startTime"]
                            .to_string()
                            .trim_matches('"')
                            .to_string(),
                    ));
                    tx.send(notification_service::NotificationAck {
                        id: notification.id,
                    })
                    .await?;
                }

                Ok(())
            }
            .await;

            match result {
                Ok(()) => {
                    // Connection succeeded, break out of the loop
                    break;
                }
                Err(err) => {
                    attempt_count += 1;
                    eprintln!("Connection attempt {} failed: {}", attempt_count, err);
                }
            }
        }

        Ok(())
    }

    let mut search_ids = Vec::new();

    for mobile_number in DRIVER_MOBILE_NUMBERS {
        let token = driver_auth(mobile_number.to_string()).await.unwrap();
        // println!("Logged in driver with token = {}", token);

        tokio::spawn(driver_notification_reciever(token.to_string()));

        set_online(token.to_string()).await.unwrap();
        // println!("Made driver online");

        set_driver_location(token.to_string()).await.unwrap();
        // println!("Set driver location");
    }

    tokio::time::sleep(Duration::from_secs(10)).await;

    for _ in 0..5 {
        let token = rider_auth().await.unwrap();
        // println!("Logged in rider with token = {}", token);

        let search_id = ride_search(token.to_string()).await.unwrap();
        // println!("Ride search created with searchId = {}", search_id);
        search_ids.push(search_id.to_string());

        tokio::time::sleep(Duration::from_secs(2)).await;

        let estimate_id = get_estimate_id(token.to_string(), search_id).await.unwrap();
        // println!("Got estimate with estimateId = {}", estimate_id);

        let _ = select_estimate(token.to_string(), estimate_id).await;
        // println!("Selected the estimate");
    }

    tokio::time::sleep(Duration::from_secs(200)).await;

    println!("searchIds : {}", search_ids.join("', '"));

    let notifications = NOTIFICATION_DETAILS.read().await;

    println!("Total Notifcations : {}", notifications.len());

    let token_counts: HashMap<String, usize> =
        notifications
            .iter()
            .fold(HashMap::new(), |mut acc, (token, _, _)| {
                *acc.entry(token.to_string()).or_insert(0) += 1;
                acc
            });

    for (token, count) in token_counts.iter() {
        println!("Token: {}, Count: {}", token, count);
    }
}
