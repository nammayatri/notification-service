/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use notification_service::common::utils::abs_diff_utc_as_sec;
    use notification_service::common::utils::decode_stream;
    use notification_service::common::utils::hash_uuid;
    use notification_service::environment::{AppConfig, AppState};
    use notification_service::redis::keys::notification_client_key;
    use notification_service::redis::types::NotificationData;
    use rand::Rng;
    use shared::tools::logger::{error, info, setup_tracing, LogLevel, LoggerConfig};
    use std::str::FromStr;
    use std::time::Duration;
    use std::{collections::HashMap, sync::Arc};
    use tokio::sync::RwLock;
    use tokio::time::sleep;

    #[tokio::test]
    async fn loadtest_with_ack() -> anyhow::Result<()> {
        let _guard = setup_tracing(LoggerConfig {
            level: LogLevel::INFO,
            log_to_file: false,
        });

        if let Ok(current_dir) = std::env::current_dir() {
            info!("Current working directory: {}", current_dir.display());
        } else {
            error!("Failed to get the current working directory");
        }

        let dhall_config_path = "../../dhall-configs/dev/notification_service.dhall".to_string();
        let app_config = serde_dhall::from_file(dhall_config_path).parse::<AppConfig>()?;

        let app_state = AppState::new(app_config).await;

        const TOTAL_CONNECTED_CLIENTS: usize = 100;

        let mut client_ids = Vec::with_capacity(TOTAL_CONNECTED_CLIENTS);

        for _ in 0..TOTAL_CONNECTED_CLIENTS {
            let client_id = uuid::Uuid::new_v4().to_string();
            client_ids.push(client_id.to_owned());
            tokio::spawn(connect_client_with_ack(client_id.to_string()));
        }

        loop {
            for client_id in client_ids.iter().take(TOTAL_CONNECTED_CLIENTS) {
                tokio::spawn(generate_and_add_notifications(
                    app_state.clone(),
                    client_id.to_string(),
                ));
            }
            sleep(Duration::from_millis(5000)).await;
        }
    }

    #[tokio::test]
    async fn loadtest_without_ack() -> anyhow::Result<()> {
        let _guard = setup_tracing(LoggerConfig {
            level: LogLevel::INFO,
            log_to_file: false,
        });

        if let Ok(current_dir) = std::env::current_dir() {
            info!("Current working directory: {}", current_dir.display());
        } else {
            error!("Failed to get the current working directory");
        }

        let dhall_config_path = "../../dhall-configs/dev/notification_service.dhall".to_string();
        let app_config = serde_dhall::from_file(dhall_config_path).parse::<AppConfig>()?;

        let app_state = AppState::new(app_config).await;

        const TOTAL_CONNECTED_CLIENTS: usize = 100;

        let mut client_ids = Vec::with_capacity(TOTAL_CONNECTED_CLIENTS);

        for _ in 0..TOTAL_CONNECTED_CLIENTS {
            let client_id = uuid::Uuid::new_v4().to_string();
            client_ids.push(client_id.to_owned());
            tokio::spawn(connect_client_without_ack(client_id.to_string()));
        }

        loop {
            for client_id in client_ids.iter().take(TOTAL_CONNECTED_CLIENTS) {
                tokio::spawn(generate_and_add_notifications(
                    app_state.clone(),
                    client_id.to_string(),
                ));
            }
            sleep(Duration::from_millis(5000)).await;
        }
    }

    #[tokio::test]
    async fn generate_notifications() -> anyhow::Result<()> {
        let _guard = setup_tracing(LoggerConfig {
            level: LogLevel::INFO,
            log_to_file: false,
        });

        if let Ok(current_dir) = std::env::current_dir() {
            info!("Current working directory: {}", current_dir.display());
        } else {
            error!("Failed to get the current working directory");
        }

        let dhall_config_path = "../../dhall-configs/dev/notification_service.dhall".to_string();
        let app_config = serde_dhall::from_file(dhall_config_path).parse::<AppConfig>()?;

        let app_state = AppState::new(app_config).await;

        let _ = generate_and_add_notifications(
            app_state.clone(),
            "5e4ad34f-a2be-45b7-9fb5-76aba43628ca".to_string(),
        )
        .await;

        Ok(())
    }

    async fn generate_and_add_notifications(
        app_state: AppState,
        client_id: String,
    ) -> anyhow::Result<()> {
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
                &notification_client_key(
                    &client_id,
                    &((hash_uuid(&client_id) % app_state.max_shards as u128) as u64),
                ),
                data.to_vec(),
                1000,
            )
            .await?;

        let res = app_state
            .redis_pool
            .xread(
                [notification_client_key(
                    &client_id,
                    &((hash_uuid(&client_id) % app_state.max_shards as u128) as u64),
                )]
                .to_vec(),
                ["0-0".to_string()].to_vec(),
                None,
            )
            .await?;

        info!("{:?}", decode_stream::<NotificationData>(res)?);

        Ok(())
    }

    async fn connect_client_without_ack(client_id: String) -> anyhow::Result<()> {
        let mut attempt_count = 0;
        const MAX_ATTEMPTS: i32 = 20;

        while attempt_count < MAX_ATTEMPTS {
            let result: anyhow::Result<()> = async {
                let mut client =
                    notification_service::notification_client::NotificationClient::connect(
                        "http://127.0.0.1:50051",
                    )
                    .await?;

                let mut metadata = tonic::metadata::MetadataMap::new();
                metadata.insert(
                    "token",
                    tonic::metadata::MetadataValue::from_str(&client_id)?,
                );

                let (_tx, rx) = tokio::sync::mpsc::channel(100000);

                let mut request =
                    tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
                *request.metadata_mut() = metadata;
                let response = client.stream_payload(request).await?;
                let mut inbound = response.into_inner();

                while let Some(response) = tokio_stream::StreamExt::next(&mut inbound).await {
                    let _notification = response?;
                    info!("Time : {}", Utc::now().format("%Y-%m-%dT%H:%M:%S%.fZ"));
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
                    error!("Connection attempt {} failed: {}", attempt_count, err);

                    // You may want to introduce a delay before the next attempt
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }

        Ok(())
    }

    async fn connect_client_with_ack(client_id: String) -> anyhow::Result<()> {
        let mut attempt_count = 0;

        loop {
            let result: anyhow::Result<()> = async {
                let mut client =
                    notification_service::notification_client::NotificationClient::connect(
                        "http://127.0.0.1:50051",
                    )
                    .await?;

                let mut metadata = tonic::metadata::MetadataMap::new();
                metadata.insert(
                    "token",
                    tonic::metadata::MetadataValue::from_str(&client_id)?,
                );

                let (tx, rx) = tokio::sync::mpsc::channel(100000);

                let mut request =
                    tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
                *request.metadata_mut() = metadata;
                let response = client.stream_payload(request).await?;
                let mut inbound = response.into_inner();

                while let Some(response) = tokio_stream::StreamExt::next(&mut inbound).await {
                    let notification = response?;
                    info!("{:?}", notification);
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
                    error!("Connection attempt {} failed: {}", attempt_count, err);

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
                        info!("Time Rx1 Delay : {}", (chrono::Utc::now() - val).num_milliseconds());
                    }
                }
                _ = timer1.tick() => {
                    if (chrono::Utc::now() - last_timer_1 - chrono::Duration::seconds(1)).num_milliseconds() < 0 {
                        continue;
                    }
                    info!("Time 1 Delay : {}", (chrono::Utc::now() - last_timer_1 - chrono::Duration::seconds(1)).num_milliseconds());
                    tokio::time::sleep(all_delay).await;
                    last_timer_1 = chrono::Utc::now();
                }
                _ = timer2.tick() => {
                    if (chrono::Utc::now() - last_timer_2 - chrono::Duration::seconds(3)).num_milliseconds() < 0 {
                        continue;
                    }
                    info!("Time 2 Delay : {}", (chrono::Utc::now() - last_timer_2 - chrono::Duration::seconds(3)).num_milliseconds());
                    tokio::time::sleep(all_delay).await;
                    last_timer_2 = chrono::Utc::now();
                }
            }
        }
    }

    #[tokio::test]
    async fn test_delay_measurement() {
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
                info!(
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
                info!(
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
                    info!(
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
        let shards = 128;
        let uuid = uuid::Uuid::from_str("f8f68652-c0ec-4d14-8060-2b327d709dca").unwrap();
        info!("{:?}", hash_uuid(&uuid.to_string()) % shards);
    }

    #[tokio::test]
    async fn test_time_diff() {
        let old = Utc::now();
        let new = Utc::now() + Duration::from_secs(30) + Duration::from_millis(1000);
        info!("{}", abs_diff_utc_as_sec(old, new))
    }

    #[tokio::test]
    async fn test_while_loop() {
        let val: Option<bool> = None;

        while let Some(_val) = val {
            info!("I am possible!");
        }
        info!("I am impossible!");
    }

    #[tokio::test]
    async fn test_thread_wait_in_loop() {
        let mut task_handles = vec![];

        for i in 0..100 {
            let handle = tokio::spawn(async move {
                loop {
                    sleep(Duration::from_secs(5)).await;
                    info!("Hello from thread : {}", i);
                }
            });
            task_handles.push(handle);
        }

        // Wait for all tasks to complete
        let _ = futures::future::join_all(task_handles).await;
    }

    #[tokio::test]
    async fn parallel_thread_sequence_with_tokio() {
        let _guard = setup_tracing(LoggerConfig {
            level: LogLevel::INFO,
            log_to_file: false,
        });

        let mut task_handles = vec![];
        let mut rng = rand::thread_rng();

        for i in 0..100 {
            let delay = rng.gen_range(0..200); // Random delay between 0 and 200 nanoseconds
            let handle = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                info!("Hello from thread : {}", i);
            });
            task_handles.push(handle);
        }

        // Wait for all tasks to complete
        let _ = futures::future::join_all(task_handles).await;
    }
}
