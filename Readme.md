# **Push Notification Service**

## **Introduction**

Driver Ride Request Popup is a critical part in our ride booking journey and FCM delays, latencies, downtime could affect our overall application performance and UX. Also, it doesn't give us clear metrics on whether the FCM was delivered to the client on time or were there any drops or delays.

### Various FCM request failures observed during the day
<img width="1485" alt="Screenshot 2024-01-11 at 3 28 29 AM" src="https://github.com/nammayatri/notification-service/assets/38260510/5930bd6d-41ba-461a-8057-6b395221ed63">

### Higher FCM API latencies gets added to the overall time it takes for the message to get delivered to client
<img width="1491" alt="Screenshot 2024-01-11 at 3 28 50 AM" src="https://github.com/nammayatri/notification-service/assets/38260510/41df6477-c821-4f71-ad38-94113b71f2e8">

So, we this Push Notification service aims to be more reliable and scalable by having better metrics.

## **Metrics**

Notifications sent during the day : 68,78,663 ~**69 Lakhs**

Notification payload size : **4 KB**

Notifications sent per second : 6878663 / (24 \* 60 \* 60) ~**80 TPS**

Number of clients connected during the day : **56,974**

Average TP99 latency : **697 ms**

## **SLAs**

Notifications sent during the day : **1M**

Notification payload size : **4 KB**

Notifications sent per second : **1000 TPS**

Number of clients connected : **1 Lakh**

Average TP99 latency : **10 ms**

## **Comparison between different communication protocols**

### MQTT

It works on TCP/IP protocol, and is lightweight to work with IoT devices like smart bulbs, sensors etc. It works on the Pub/Sub model and the connection is not persistent. MQTT client will connect and disconnect to read/write messages like ping. MQTT provides Quality of Service (QoS)level, which is an agreement between the sender of a message and the receiver of a message that defines the guarantee of delivery for a specific message.

There are 3 QoS levels in MQTT:

- At most once (0) at most once.
- At least once (1)at least once.
- Exactly once (2) exactly once.


In MQTT, when a client subscribes to a topic, it typically receives only the messages that are published after the subscription is made. However, MQTT provides a feature called "retained messages" which allows a client to receive the last message that was published to a topic, even if it was published before the client subscribed. If a new retained message is published to the topic, it replaces the previous one. When a new client subscribes to a topic that has a retained message, the MQTT broker immediately sends this retained message to the client. This helps the client to get the last known good value.

AWS MQTT Broker Price ~ **16,757 Rs**

### WebSocket

It is based on HTTP 1.1 protocol and came to support bi-directional communication which was not supported with plain HTTP 1.1 protocol. Websockets maintain persistent connection. Message delivery order will be maintained and there are many libraries and frameworks to support websocket and pub/sub over websockets.

### GRPC

This would look similar to websockets but underlying difference is it works on HTTP2 protocol and the data format for request response would be bound to Protobuf, cannot use JSON or XML. But protobuf is more compact and light weight than the latter. The connection would be persistent and the client can invoke the methods in the remote server through the connection as needed. It offers 4 types of method call, traditional request/response model, server-side streaming, client side streaming and bi-directional streaming.
[_ **UBER** _](https://www.uber.com/en-IN/blog/ubers-next-gen-push-platform-on-grpc/) uses GRPC for sending reliable and high performance push notifications to its clients.

<img width="1522" alt="Screenshot 2024-01-11 at 3 29 53 AM" src="https://github.com/nammayatri/notification-service/assets/38260510/821744c6-8c52-49fc-9bde-eea7f084bd53">

## **Load Testing**

On conducting load testing for GRPC, Websockets, and MQTT using Rust, transitioning from Node.js due to its limitations in handling concurrent client connections being single-threaded in nature. Rust implementation GitHub repository at [https://github.com/khuzema786/grpc-mqtt-websockets](https://github.com/khuzema786/grpc-mqtt-websockets).

The following are the summarized results of load test performed for MQTT/GRPC/Websockets, which measures the average round trip duration for (1, 10, 100, 1000) messages of size (1.5, 4 KB) sent from the server and the acknowledgements received from clients for each message along with the serialization and deserialization of the message payloads as JSON/Protobuf. This test was conducted with clients (1, 100) concurrently connected to the server/broker, measuring duration of the complete cycle of message transmission and response. I have taken an average of durations across 10 samples for message round trips across a client.

### 1 client connected to notification server allocated 1 Core CPU and 1GB Memory

| **Type** | **MQTT** | **Websockets** | **GRPC** |
| --- | --- | --- | --- |
| 1 Message (1.5 KB - 10 samples) | 9.5 ms | 3.1 ms | 2.7 ms |
| 10 Messages (1.5 KB - 10 samples) | 30.6 ms | 11 ms | 9.4 ms |
| 100 Messages (1.5 KB - 10 samples) | 128.5 ms | 53 ms | 72.3 ms |
| 1000 Messages (1.5 KB - 10 samples) | 674.2 ms | 444.3 ms | 501.2 ms |
| 1 Messages (4 KB - 10 samples) | 12.7 ms | 5.1 ms | 4 ms |
| 10 Messages (4 KB - 10 samples) | 25 ms | 15.4 ms | 10.7 ms |
| 100 Messages (4 KB - 10 samples) | 114.4 ms | 67.8 ms | 71.7 ms |
| 1000 Messages (4 KB - 10 samples) | 685.1 ms | 511.3 ms | 514.7 ms |

### 100 concurrent clients connected to notification server allocated 1 Core CPU and 1GB Memory

| **Type** | **MQTT** | **Websockets** | **GRPC** |
| --- | --- | --- | --- |
| 1 Message (1.5 KB - 10 samples) | 62.8 ms | 7 ms | 8.6 ms |
| 10 Messages (1.5 KB - 10 samples) | 339.6 ms | 55.5 ms | 61.3 ms |
| 100 Messages (1.5 KB - 10 samples) | 3,155.9 ms ~ 3 s | 491 ms | 614.3 ms |
| 1000 Messages (1.5 KB - 10 samples) | 31,101.9 ms ~ 31 s | 4,878.5 ms ~ 5 s | 6,219.4 ms ~ 6 s |
| 1 Messages (4 KB - 10 samples) | 70.2 ms | 10 ms | 10.3 ms |
| 10 Messages (4 KB - 10 samples) | 354.2 ms | 72 ms | 62.3 ms |
| 100 Messages (4 KB - 10 samples) | 3,274.9 ms ~ 3 s | 694.9 ms | 631.1 ms |
| 1000 Messages (4 KB - 10 samples) | 32,833.4 ms ~ 33 s | 6,910.7 ms ~ 7 s | 6,391 ms ~ 6 s |

### 1,000 open connections with 100 concurrent clients at rate of 2 seconds sending messages connected to notification server allocated 1 Core CPU and 1GB Memory

| **Type** | **MQTT** | **Websockets** | **GRPC** |
| --- | --- | --- | --- |
| 1 Message (1.5 KB - 2 samples) | - | - | - |
| 1 Message (4 KB - 2 samples) | - | - | - |

### 10,000 open connections with 100 concurrent clients at rate of 2 seconds sending messages connected to notification server allocated 1 Core CPU and 1GB Memory

| **Type** | **MQTT** | **Websockets** | **GRPC** |
| --- | --- | --- | --- |
| 1 Message (1.5 KB - 10 samples) | - | 8.75 ms (2 Failures) | - |
| 1 Message (4 KB - 10 samples) | - | 10.3 ms (1 Failure) | 10.3 ms (1 Failure) |

### 1,00,000 open connections with 100 concurrent clients at rate of 2 seconds sending messages connected to notification server allocated 1 Core CPU and 1GB Memory

| **Type** | **MQTT** | **Websockets** | **GRPC** |
| --- | --- | --- | --- |
| 1 Message (1.5 KB - 2 samples) | - | - | 12,790 Success, Rest Failures |
| 1 Message (4 KB - 2 samples) | - | - | - |

**Estimations**

- In GRPC, 1 open connection requires 60 KB. For 50,000 online drivers. Memory required for handling 50,000 open connections is 3 GB.
- In Websockets, 1 open connection requires 30 KB. For 50,000 online drivers. Memory required for handling 50,000 open connections is 1.5 GB.

**Observations**

- In Websockets & GRPC performance looks almost comparable for a smaller payload size.
- In MQTT the duration increases a lot when 100 connections are open concurrently and exchange that many messages concurrently. This increase could be because of the eclipse-mosquitto broker that I am using locally but in order to have a scalable and reliable broker If we use AWS managed service, it may increase infra cost which I calculated to be around 21,000 Rs per Month but not sure if I estimated it correctly.
- In MQTT if a client disconnects from the broker and the notification is published to the topic on the broker by the server then it gets dropped. Inorder to keep track of messages and handle retries we may need to have same redis stream queue based implementation even in case of MQTT as well similar to GRPC or Websockets.
- In case of GRPC/Websockets, upon client connection the server can lookup in redis stream for pending messages to be sent but in case of MQTT publisher wouldn't get to know if the client is connected to the topic so handling message retries from publisher could be a little tricky.

**gRPC is a high-performance, widely adopted RPC framework with standardized implementations of client and server across many platforms and languages. The major reasons for moving towards gRPC are highlighted below:**

- **Bidirectional Streaming** gRPC has first-class support for bidirectional streaming, the acknowledgement can be sent over the same stream instantly without extra network calls from the mobile client. This could significantly improve the acknowledgement reliability and metrics on our Backend.
- **Some Fault Tolerance To Bandwidth Fluctuations** On testing, we observed that when a message is sent to grpc channel for a client and in case the client's wifi is off for a small amount of time then the messages are still sent and when the client’s net is back on they get delivered altogether offering some amount of fault tolerance to network failures.
- **Keep-alive health check ping** It has a mechanism of frequent health checks on regular intervals of time so that in case if the bandwidth of the client is gone for long then we can close the connection.

## **Proposed Flow - (i)**

**Components**

- **Client** : Frontend SDK listening for notifications.
- **Notification Server** : GRPC Server broadcasting the notifications to the client in Protobuf format over a Persistent QUIC connection.
- **Redis** : To map the client connection to the notification server IP in the StatefulSet cluster.
- **BAP/BPP** : Application that sends notifications at various stages.
- **FCM** : Google's firebase messaging service that sends push notifications to both Android & IOS.

<img width="1588" alt="Screenshot 2024-01-11 at 3 30 38 AM" src="https://github.com/nammayatri/notification-service/assets/38260510/b1fe40c5-e6a8-43b3-b774-f99c7263a6bb">

**Stage (i) - Client connection to Notification Server**

1. _ **Client** _ connects to the _ **Notification Server** _ through a persistent QUIC connection.
2. _ **Notification Server** _maps its IP Address for that clientId in _ **Redis** _.

**Stage (ii) - Application wants to send the Notification to a particular client**

1. _ **BAP/BPP** _ sends the notification JSON payload to both FCM and Notification Server as below:
  1. Over an external API call to _ **FCM** _ service.
  2. Over an Internal HTTP call to _ **Notification Server** _ service on the IP address found from _ **Redis** _ for that clientId.

**Stage (iii) - Push Notification to the connected client**

1. Push the notification to the connected _ **Client** _ by persistent GRPC bi-directional streaming connection in Protobuf format.

**Stage (iv) - Notification received on client end**

1. De-Duplicate the notification received by the same "Notification ID" from both FCM and GRPC server.

One drawback of this architecture is that it doesn't account for notification failures and retries due to client disconnections and reconnections to a different server. To handle the reliability and guarantee for notifications delivery we can look into the below flow proposal.

## **Proposed Flow - (ii)**

**Components**

- **Client** : Frontend SDK listening for notifications.
- **Notification Server** : GRPC Server broadcasting the notifications to the client in Protobuf format over a Persistent QUIC connection.
- **Redis Streams** : Redis Pub-Sub layer for message queue.
- **BAP/BPP** : Application that sends notifications at various stages.
- **FCM** : Google's firebase messaging service that sends push notifications to both Android & IOS.

<img width="1300" alt="Screenshot 2024-01-11 at 3 31 11 AM" src="https://github.com/nammayatri/notification-service/assets/38260510/0984fdcf-1f4d-418b-9a56-2a8b206e79c8">

**Stage (i) - Client connection to Notification Server**

1. _ **Client** _ connects to the _ **Notification Server** _ through a persistent Bi-directional QUIC connection.

**Stage (ii) - Application wants to send the Notification to a particular client**

1. _ **BAP/BPP** _ sends the notification JSON payload to an external _ **FCM** _ service and also publishes it to _ **Redis Stream** _ for the clientId.

**Stage (iii) - Push Notification to the connected client**

1. Push the notification read from the _ **Redis Stream** _ to the connected _ **Client** _ by persistent GRPC bi-directional streaming connection in Protobuf format.

**Stage (iv) - Notification received on client end**

1. De-Duplicate the notification received by the same "Notification ID" from both FCM and GRPC server.
2. Send an Acknowledgement to the _ **Notification Server** _ for the received notification over the persistent GRPC stream connection.

**Stage (v) - Acknowledge the received notification**

1. Delete the acknowledged notification from Redis Stream to avoid retries for it.

## **Advantages**

1. Since this will offer us more control over the notification sent to the client, we can also retry the undelivered messages again before they expire.
2. We can track the status of each notification in clickhouse for analytics and also add push metrics to prometheus.
3. On testing, we found that if the client's wifi is turned off then the messages get sent once the client comes back on in some time, so it handles the cases where bandwidth fluctuates and offers some amount of reliability to Network failures.
4. We can also set a keep alive ping that can ask for acknowledgement from clients to be sure the client is not away for too long. If so then, we can forcefully terminate the connection and ask the client to reconnect.

## **Architecture: `read_all_connected_client_notifications` flag**

The notification reader (`crates/notification_service/src/reader.rs`) supports two operating modes, selected by the `read_all_connected_client_notifications` boolean in `notification_service.dhall`. Both modes share the same gRPC ingress, per-shard `ReaderMap` (`Arc<Vec<MonitoredRwLock<ReaderMap>>>`) keyed by `ClientId`, Redis Streams as the durable per-client queue, and the `send_notification → set_notification_stream_id → client_tx.send → clean_up_notification` send path. They differ in how the server learns that a notification is ready.

### Shared components

- **gRPC `StreamPayload`** — bi-directional stream between client and `Notification Server`; carries notifications server→client and ACKs client→server.
- **`client_reciever_looper`** — handles `ClientConnection`, `ClientDisconnection`, and `ClientAck` events, mutating the shard map.
- **`retry_notifications_looper`** — runs every `retry_delay_millis`, sweeps shards, and re-pushes notifications still in the Redis stream.
- **`ActiveNotification`** — per-session in-memory tracker of unacked `stream_id`s, used to decide whether a client still has pending work.
- **Redis Streams** — durable queue per client (`XADD`/`XREAD`), entries cleaned up via `clean_up_notification` on ACK or send.
- **Redis Pub/Sub channel** (`pubsub_channel_key`) — fan-out signal that "client X has a new entry on its stream"; only used when the flag is `False`.

### Mode A — `read_all_connected_client_notifications = false` (event-driven)

```
BAP/BPP --XADD-->  Redis Stream (per client)
        --PUBLISH--> Redis Pub/Sub channel
                            |
                            v
                  active_notification_looper
                            |
                            v   (read_client_notification + send)
Client <==gRPC stream==  Notification Server <==periodic retry sweep== Redis
                            ^
                            | only clients with ActiveNotification.count() > 0
```

Behavior:
1. **On connect** — `client_reciever` preloads pending entries via `ActiveNotification::new(redis_pool, client_id, shard)` so a reconnecting client immediately sees what it missed.
2. **On new notification** — publisher `XADD`s the stream and `PUBLISH`es on the pub/sub channel. `active_notification_looper` subscribes to that channel; for each message it reads the client's stream and pushes to all live `client_tx`s for that `ClientId` (single or multi-session), updating the per-session `ActiveNotification`.
3. **On ACK** — `ClientAck` calls `active_notification.acknowledge(notification_id)`, decrementing the unacked count.
4. **Retry sweep** — `retry_notifications` only collects `client_id`s whose `ActiveNotification.count() > 0` (i.e. has unacked entries), then issues a batched `read_client_notifications` for that subset and resends. Resends increment `RETRIED_NOTIFICATIONS`.
5. **Counters** — first delivery via the pub/sub path increments `TOTAL_NOTIFICATIONS`; retry resends increment `RETRIED_NOTIFICATIONS`.

Trade-offs: minimal Redis I/O per tick (only clients with known unacked work are queried), low push latency (pub/sub fans out immediately), but requires the publisher and Redis to keep pub/sub healthy and requires per-session `ActiveNotification` state to stay accurate.

### Mode B — `read_all_connected_client_notifications = true` (poll-all)

```
BAP/BPP --XADD-->  Redis Stream (per client)

                  retry_notifications_looper  (every retry_delay_millis)
                            |
                            v   read_client_notifications(ALL connected client_ids)
Client <==gRPC stream==  Notification Server
```

Behavior:
1. **On connect** — `ActiveNotification` is initialized **empty** (`ActiveNotification::default()`); the connect-time Redis read is skipped because the retry loop will pick the client up on the next tick anyway.
2. **No pub/sub path** — `active_notification_looper` is not spawned; `tokio::select!` runs only `client_reciever_looper`, `retry_notifications_looper`, and the graceful shutdown receiver.
3. **Retry sweep** — `retry_notifications` ignores `ActiveNotification.count()` and returns *every* connected `client_id` per shard. A single batched `read_client_notifications` per shard fetches whatever is currently on each stream and pushes it.
4. **Counters** — every delivery is counted as `TOTAL_NOTIFICATIONS` (there is no separate "retry" notion in this mode); `RETRIED_NOTIFICATIONS` is not incremented.

Trade-offs: simpler invariants (no pub/sub dependency, no per-session unacked bookkeeping needed for correctness, recovers automatically after pub/sub gaps), at the cost of one Redis stream read per connected client per `retry_delay_millis`. Push latency is bounded by `retry_delay_millis` rather than pub/sub fan-out time. This is the default in `dhall-configs/dev/notification_service.dhall`.

### Choosing between modes

| Aspect | `false` (event-driven) | `true` (poll-all) |
|---|---|---|
| New-message latency | ~pub/sub RTT | up to `retry_delay_millis` |
| Redis ops per tick | O(clients with unacked) | O(connected clients) |
| Connect-time Redis read | yes (preload) | no (retry tick handles it) |
| Pub/Sub dependency | required | none |
| Active-notification tracking | load-bearing | bypassed in retry path |
| Default in dev dhall | — | ✅ |

## Setting up development environment

### Nix

1. [Install **Nix**](https://nixos.asia/en/install)
1. Run `nix run nixpkgs#nix-health` and make sure that everything is green (✅)
1. Setup the Nix **binary cache** (to avoid compiling locally for hours):
    ```sh
    nix run nixpkgs#cachix use nammayatri
    ```
    - For this command to succeed, you should add yourself to the `trusted-users` list of `nix.conf` and then restart the Nix daemon using `sudo pkill nix-daemon`.
1. Install **home-manager**[^hm] and setup **nix-direnv** and **starship** by following the instructions [in this home-manager template](https://github.com/juspay/nix-dev-home).[^direnv] You want this to facilitate a nice Nix develoment environment. Read more about direnv [here](https://nixos.asia/en/direnv).

[^hm]: Unless you are using NixOS in which case home-manager is not strictly needed.
[^direnv]: Not strictly required to develop nammayatri. If you do not use `direnv` however you would have to remember to manually restart the `nix develop` shell, and know when exactly to do this each time.

### Rust

`cargo` is available in the Nix develop shell. You can also use one of the `just` commands (shown in Nix shell banner) to invoke cargo indirectly.

### VSCode

The necessary extensions are configured in `.vscode/`. See [nammayatri README](https://github.com/nammayatri/nammayatri/tree/main/Backend#visual-studio-code) for complete instructions.

### Autoformatting

Run `just fmt` (or `treefmt`) to auto-format the project tree. The CI checks for it.

### pre-commit

pre-commit hooks will be installed in the Nix devshell. Run the `pre-commit` command to manually run them. You can also run `pre-commit run -a` to run pre-commit hooks on *all* files (modified or not).

### Services

Run `just services` to run the service dependencies (example: redis-server) using [services-flake](https://github.com/juspay/services-flake).

## Usage / Installing

Run `nix build` in the project which produces a `./result` symlink. You can also run `nix run` to run the program immediately after build.

## Upstream Dependent Service

Clone and Run [dynamic-driver-offer-app](https://github.com/nammayatri/nammayatri) as it will be used for Internal Authentication and Testing postman flow.

## Postman Collection

Import the [Postman Collection](./Location%20Tracking%20Service%20Dev.postman_collection.json) in postman to test the API flow or Run `newman run LocationTrackingService.postman_collection.json --delay-request 2000`.

## Contributing PRs

Run `nix run nixpkgs#nixci` locally to make sure that the project builds. The CI runs the same.

## Profiling

1. In `crates/notification_service/src/main.rs` add following line in starting for tracing `console_subscriber::init();`.
2. Comment out `let _guard = setup_tracing(app_config.logger_cfg);` in `crates/notification_service/src/server.rs`.
3. Instead of `cargo run`. Do, `RUSTFLAGS="--cfg tokio_unstable" cargo run`.

## Debugging

To connect to GRPC server use below curl command in `nix develop` shell :

```
grpcurl -plaintext -d '{"id": "1234567890-0","created_at":"2024-01-21T13:45:38.057846262Z"}' -import-path ./crates/notification_service/protos -proto notification_service.proto -H "token: 27da7b7e-dd80-4dfe-9f66-4e961a96e99e" localhost:50051 notification_service.Notification/StreamPayload
```

## Loadtest

To loadtest follow the below intstructions :

1. To get eligible driver tokens and mobile numbers, run the below sql query.
```
SELECT
  p.merchant_id,
  moc.city,
  v.variant,
  ARRAY_AGG(r.token) AS tokens,
  ARRAY_AGG(p.unencrypted_mobile_number) AS mobile_numbers
FROM
  atlas_driver_offer_bpp.person p
  JOIN atlas_driver_offer_bpp.vehicle v ON p.id = v.driver_id
  JOIN atlas_driver_offer_bpp.registration_token r ON r.entity_id = p.id
  JOIN atlas_driver_offer_bpp.merchant_operating_city moc ON moc.id = p.merchant_operating_city_id
WHERE
  p.unencrypted_mobile_number IS NOT NULL
  AND p.id IN (
    SELECT
      driver_id
    FROM
      atlas_driver_offer_bpp.driver_information
    WHERE
      enabled = true
      AND auto_pay_status = 'ACTIVE'
      AND subscribed = true
  )
GROUP BY
  p.merchant_id,
  moc.city,
  v.variant;
```
2. After running loadtest `loadtest_ridebooking_flow` we can tally the count of total notifications with search request for drivers entries creation.
```
select
  r.token,
  d.driver_id,
  count(*)
from
  atlas_driver_offer_bpp.search_request_for_driver as d
  join atlas_driver_offer_bpp.registration_token as r on r.entity_id = d.driver_id
where
  d.search_request_id IN (
    select
      id
    from
      atlas_driver_offer_bpp.search_request
    where
      transaction_id IN (
        '7ef6e807-2547-4fc5-a4bd-b489987a768f',
        'e6965e66-71ef-43e4-9a0c-952735afb75f',
        '76302199-ac7e-43f1-a939-0bb242c473b6',
        '49a8daea-8185-4657-b69c-cd8c9c99a25a',
        'b535830f-a9f8-4eb4-97d8-04a2c9e17f85'
      )
  )
  and d.driver_id IN (
    select
      id
    from
      atlas_driver_offer_bpp.person
    where
      unencrypted_mobile_number IN (
        '9642420000',
        '9876544447',
        '9876544457',
        '9344676990',
        '9876544449',
        '8123456780',
        '8123456789',
        '9491839513',
        '9876544448',
        '9876544459'
      )
  )
group by
  d.driver_id,
  r.token;
```
3. To loadtest for connections we can generate redis stream commands using `loadtest_xadd_key_generator` and run them in redis. Once that is done we can run `loadtest_connections` to run load test for concurrent connections. We can get driver ids and tokens using below query.
```
SELECT
  p.merchant_id,
  moc.city,
  v.variant,
  ARRAY_AGG(DISTINCT(p.id)) AS driver_ids,
  ARRAY_AGG(DISTINCT(r.token)) AS tokens,
  ARRAY_AGG(DISTINCT(p.unencrypted_mobile_number)) AS mobile_numbers
FROM
  atlas_driver_offer_bpp.person p
  JOIN atlas_driver_offer_bpp.vehicle v ON p.id = v.driver_id
  JOIN atlas_driver_offer_bpp.registration_token r ON r.entity_id = p.id
  JOIN atlas_driver_offer_bpp.merchant_operating_city moc ON moc.id = p.merchant_operating_city_id
WHERE
  p.unencrypted_mobile_number IS NOT NULL
  AND r.token_expiry >= 365 AND r.created_at >= CURRENT_DATE - INTERVAL '364 day'
GROUP BY
  p.merchant_id,
  moc.city,
  v.variant;
```