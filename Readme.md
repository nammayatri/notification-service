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

In cargo.toml add :

```
[profile.dev]
debug = true
debug-assertions = false

[profile.release]
debug = true
debug-assertions = false
```

## Debugging

To connect to GRPC server use below curl command in `nix develop` shell :

```
grpcurl -plaintext -d '{"id": "1234567890-0","created_at":"2024-01-21T13:45:38.057846262Z"}' -import-path ./crates/notification_service/protos -proto notification_service.proto -H "token: 27da7b7e-dd80-4dfe-9f66-4e961a96e99e" localhost:50051 notification_service.Notification/StreamPayload
```