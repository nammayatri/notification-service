/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

#[allow(dead_code)]
const CLIENT_ID: &str = "476c4daf-7480-4cf7-aa6a-27052a80a1df";

#[tokio::test]
async fn generate_and_add_notifications() -> anyhow::Result<()> {
    use chrono::Utc;
    use notification_service::common::utils::decode_stream;
    use notification_service::environment::{AppConfig, AppState};
    use notification_service::redis::types::NotificationData;
    use notification_service::{common::utils::hash_uuid, redis::keys::notification_client_key};
    use std::time::Duration;

    if let Ok(current_dir) = std::env::current_dir() {
        println!("Current working directory: {}", current_dir.display());
    } else {
        eprintln!("Failed to get the current working directory");
    }

    let dhall_config_path = "../../dhall-configs/dev/notification_service.dhall".to_string();
    let app_config = serde_dhall::from_file(dhall_config_path).parse::<AppConfig>()?;

    let app_state = AppState::new(app_config).await;

    let data = [
        ("entity.id", "181a66a5-749c-4c9f-aea5-a5418b981cf0"),
        ("entity.type", "SearchRequest"),
        ("entity.data", "{\"searchRequestValidTill\":\"2023-12-23T13:45:38.057846262Z\",\"searchRequestId\":\"181a66a5-749c-4c9f-aea5-a5418b981cf0\",\"startTime\":\"2022-08-15T13:43:30.713006Z\",\"baseFare\":100.99,\"distance\":6066,\"distanceToPickup\":316,\"fromLocation\":{\"area\":\"B-3, CA-1/99, Ganapathi Temple Rd, KHB Colony, 5th Block, Koramangala, Bengaluru, Karnataka 560095, India\",\"state\":null,\"createdAt\":\"2022-08-15T13:43:37.771311059Z\",\"country\":null,\"building\":null,\"door\":null,\"street\":null,\"lat\":12.9362698,\"city\":null,\"areaCode\":null,\"id\":\"ef9ff2e4-592b-4b00-bb07-e8d9c4965d84\",\"lon\":77.6177708,\"updatedAt\":\"2022-08-15T13:43:37.771311059Z\"},\"toLocation\":{\"area\":\"Level 8, Raheja towers, 23-24, Mahatma Gandhi Rd, Yellappa Chetty Layout, Sivanchetti Gardens, Bengaluru, Karnataka 560001, India\",\"state\":null,\"createdAt\":\"2022-08-15T13:43:37.771378308Z\",\"country\":null,\"building\":null,\"door\":null,\"street\":null,\"lat\":12.9730611,\"city\":null,\"areaCode\":null,\"id\":\"3780b236-715b-4822-b834-96bf0800c8d6\",\"lon\":77.61707299999999,\"updatedAt\":\"2022-08-15T13:43:37.771378308Z\"},\"durationToPickup\":139}"),
        ("id", &uuid::Uuid::new_v4().to_string()),
        ("category", "NEW_RIDE_AVAILABLE"),
        ("title", "New ride available for offering"),
        ("body", "A new ride for 15 Aug, 07:13 PM is available 316 meters away from you. Estimated base fare is 100 INR, estimated distance is 6066 meters"),
        ("show", "SHOW"),
        ("created_at", &Utc::now().format("%Y-%m-%dT%H:%M:%S%.fZ").to_string()),
        ("ttl", &(Utc::now() + Duration::from_secs(30)).format("%Y-%m-%dT%H:%M:%S%.fZ").to_string())
    ];

    app_state
        .redis_pool
        .xadd(
            &notification_client_key(CLIENT_ID, &(hash_uuid(CLIENT_ID) % app_state.max_shards)),
            data.to_vec(),
            1000,
        )
        .await?;

    let res = app_state
        .redis_pool
        .xread(
            [notification_client_key(
                CLIENT_ID,
                &(hash_uuid(CLIENT_ID) % app_state.max_shards),
            )]
            .to_vec(),
            ["0-0".to_string()].to_vec(),
            None,
        )
        .await?;

    println!("{:?}", decode_stream::<NotificationData>(res)?);

    Ok(())
}

#[tokio::test]
async fn connect_client_without_ack() -> anyhow::Result<()> {
    use chrono::Utc;

    let mut attempt_count = 0;

    loop {
        let result: anyhow::Result<()> = async {
            use std::str::FromStr;

            let mut client =
                notification_service::notification_client::NotificationClient::connect(
                    "http://127.0.0.1:50051",
                )
                .await?;

            let mut metadata = tonic::metadata::MetadataMap::new();
            metadata.insert(
                "token",
                tonic::metadata::MetadataValue::from_str(CLIENT_ID)?,
            );

            let (_tx, rx) = tokio::sync::mpsc::channel(100000);

            let mut request = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
            *request.metadata_mut() = metadata;
            let response = client.stream_payload(request).await?;
            let mut inbound = response.into_inner();

            while let Some(response) = tokio_stream::StreamExt::next(&mut inbound).await {
                let _notification = response?;
                println!("Time : {}", Utc::now().format("%Y-%m-%dT%H:%M:%S%.fZ"));
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

                // You may want to introduce a delay before the next attempt
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }

    Ok(())
}

#[tokio::test]
async fn connect_client_with_ack() -> anyhow::Result<()> {
    let mut attempt_count = 0;

    loop {
        let result: anyhow::Result<()> = async {
            use std::str::FromStr;

            let mut client =
                notification_service::notification_client::NotificationClient::connect(
                    "http://127.0.0.1:50051",
                )
                .await?;

            let mut metadata = tonic::metadata::MetadataMap::new();
            metadata.insert(
                "token",
                tonic::metadata::MetadataValue::from_str(CLIENT_ID)?,
            );

            let (tx, rx) = tokio::sync::mpsc::channel(100000);

            let mut request = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
            *request.metadata_mut() = metadata;
            let response = client.stream_payload(request).await?;
            let mut inbound = response.into_inner();

            while let Some(response) = tokio_stream::StreamExt::next(&mut inbound).await {
                let notification = response?;
                println!("{:?}", notification);
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

                // You may want to introduce a delay before the next attempt
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_select_behaviour() {
    let timer_1_delay = std::time::Duration::from_secs(1);
    let timer_2_delay = std::time::Duration::from_secs(3);

    let mut timer1 = tokio::time::interval(timer_1_delay);
    let mut timer2 = tokio::time::interval(timer_2_delay);

    let mut last_timer_1 = chrono::Utc::now() - timer_1_delay;
    let mut last_timer_2 = chrono::Utc::now() - timer_2_delay;

    let all_delay = std::time::Duration::from_secs(1);

    let (tx1, mut rx1) = tokio::sync::mpsc::channel(128);

    tokio::spawn(async move {
        loop {
            let _ = tx1.send(chrono::Utc::now()).await;
            tokio::time::sleep(all_delay).await;
        }
    });

    loop {
        tokio::select! {
            val = rx1.recv() => {
                if let Some(val) = val {
                    println!("Time Rx1 Delay : {}", (chrono::Utc::now() - val).num_milliseconds());
                }
            }
            _ = timer1.tick() => {
                if (chrono::Utc::now() - last_timer_1 - chrono::Duration::seconds(1)).num_milliseconds() < 0 {
                    continue;
                }
                println!("Time 1 Delay : {}", (chrono::Utc::now() - last_timer_1 - chrono::Duration::seconds(1)).num_milliseconds());
                tokio::time::sleep(all_delay).await;
                last_timer_1 = chrono::Utc::now();
            }
            _ = timer2.tick() => {
                if (chrono::Utc::now() - last_timer_2 - chrono::Duration::seconds(3)).num_milliseconds() < 0 {
                    continue;
                }
                println!("Time 2 Delay : {}", (chrono::Utc::now() - last_timer_2 - chrono::Duration::seconds(3)).num_milliseconds());
                tokio::time::sleep(all_delay).await;
                last_timer_2 = chrono::Utc::now();
            }
        }
    }
}

#[tokio::test]
async fn test_delay_measurement() {
    use std::{collections::HashMap, sync::Arc};
    use tokio::{sync::RwLock, time::sleep};

    let all_delay = std::time::Duration::from_secs(1);
    let shared_state: Arc<RwLock<HashMap<String, String>>> =
        Arc::new(RwLock::new(HashMap::default()));

    let (tx1, rx1) = tokio::sync::mpsc::channel(128);
    let tx1_clone = tx1.clone();

    tokio::spawn(task_1(all_delay, shared_state.clone()));
    tokio::spawn(task_2(all_delay, shared_state.clone()));
    tokio::spawn(task_rx(rx1, all_delay, shared_state.clone()));

    tokio::spawn(async move {
        loop {
            let _ = tx1_clone.send(chrono::Utc::now()).await;
            tokio::time::sleep(all_delay).await;
        }
    })
    .await
    .unwrap();

    async fn task_1(
        all_delay: std::time::Duration,
        shared_state: Arc<RwLock<HashMap<String, String>>>,
    ) {
        let delay = std::time::Duration::from_secs(1);
        let mut timer_delay = tokio::time::interval(delay);
        let mut last_timer_1 = chrono::Utc::now() - delay;
        loop {
            println!(
                "Time 1 Delay : {}",
                (chrono::Utc::now() - last_timer_1 - chrono::Duration::seconds(1))
                    .num_milliseconds()
            );
            let _ = shared_state.read().await;
            tokio::time::sleep(all_delay).await;
            last_timer_1 = chrono::Utc::now();
            timer_delay.tick().await;
        }
    }

    async fn task_2(
        all_delay: std::time::Duration,
        shared_state: Arc<RwLock<HashMap<String, String>>>,
    ) {
        let delay = std::time::Duration::from_secs(3);
        let mut last_timer_2 = chrono::Utc::now() - delay;

        loop {
            println!(
                "Time 2 Delay : {}",
                (chrono::Utc::now() - last_timer_2 - chrono::Duration::seconds(3))
                    .num_milliseconds()
            );
            let _ = shared_state.read().await;
            tokio::time::sleep(all_delay).await;
            last_timer_2 = chrono::Utc::now();
            sleep(delay).await
        }
    }

    async fn task_rx(
        mut rx1: tokio::sync::mpsc::Receiver<chrono::DateTime<chrono::Utc>>,
        all_delay: std::time::Duration,
        shared_state: Arc<RwLock<HashMap<String, String>>>,
    ) {
        loop {
            if let Some(val) = rx1.recv().await {
                println!(
                    "Time Rx1 Delay : {}",
                    (chrono::Utc::now() - val).num_milliseconds()
                );
                let _ = shared_state
                    .write()
                    .await
                    .insert("Hello".to_string(), "World".to_string());
                tokio::time::sleep(all_delay).await;
            }
        }
    }
}

#[tokio::test]
async fn test_hash_uuid_mod_shards() {
    use notification_service::common::utils::hash_uuid;

    let shards = 128;
    let uuid = uuid::Uuid::new_v4();
    println!("{}", hash_uuid(&uuid.to_string()) % shards);
}

#[tokio::test]
async fn test_time_diff() {
    use chrono::Utc;
    use notification_service::common::utils::abs_diff_utc_as_sec;
    use std::time::Duration;

    let old = Utc::now();
    let new = Utc::now() + Duration::from_secs(30);
    println!("{}", abs_diff_utc_as_sec(old, new) as u32)
}

#[tokio::test]
async fn test_while_loop() {
    let val: Option<bool> = None;

    while let Some(_val) = val {
        println!("I am possible!");
    }
    println!("I am impossible!");
}
