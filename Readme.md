# GRPC Notification Service

The major problem we are currently facing with the FCM is reliability. We don’t get any metrics regarding if the FCM’s were delivered or not. It’s just fire and forget. So in order to solve that issue we needed to come up with a system that is more reliable and tolerant to bandwidth fluctuations.

## Why GRPC ?

gRPC is a high-performance, widely adopted RPC framework with standardized implementations of client and server across many platforms and languages. The major reasons for moving towards gRPC are highlighted below:

1. **Bidirectional Streaming** gRPC has first-class support for bidirectional streaming, the acknowledgement can be sent over the same stream instantly without extra networking calls from the mobile client. This could significantly improve the acknowledgement reliability for our BPP Backend.
2. **QUIC/HTTP3** QUIC/HTTP3 essentially removes the head-of-line blocking by multiplexing streams, consistently and significantly improving mobile networking latency compared to HTTP2. It is based on UDP and is prone to connection migration.
3. **Some Fault Tolerance To Bandwidth Fluctuations** On testing, we observed that when a message is sent to grpc channel for a client and in case the client's wifi is off for a small amount of time then the messages are still sent and when the client’s net is back on they get delivered altogether offering some amount of fault tolerance to network failures.
4. **Keep-alive health check ping** It has a mechanism of frequent health checks on regular intervals of time so that in case if the bandwidth of the client is gone for long then we can close the connection.

## gRPC failure handling

1. Connection Backoff – When we do a connection to a backend which fails, it is typically desirable to not retry immediately (to avoid flooding the network or the server with requests) and instead do some form of exponential backoff.
2. Keepalive – The keepalive ping is a way to check if a channel is currently working by sending HTTP2 pings over the transport. It is sent periodically, and if the ping is not acknowledged by the peer within a certain timeout period, the transport is disconnected.

## GRPC Bi-Directional Streaming Architecture

**Components**
- Client: Frontend SDK listening for notifications.
- Notification Server: GRPC Server broadcasting the notifications to the client in Protobuf format over a Persistent QUIC connection.
- Redis Streams: Redis Pub-Sub layer for message queue.
- BAP/BPP: Application that sends notifications at various stages.
- FCM: Google’s firebase messaging service that sends push notifications to both Android & IOS.

<img width="889" alt="Screenshot 2024-01-04 at 10 56 36 AM" src="https://github.com/nammayatri/notification-service/assets/38260510/4ab233a4-6b52-4dab-b511-a11559438a61">

**Stage (i)** - Client connection to Notification Server
1. Client connects to the Notification Server through a persistent Bi-directional QUIC connection.

**Stage (ii)** - Application wants to send the Notification to a particular client
2. BAP/BPP sends the notification JSON payload to an external FCM service and also publishes it to Redis Stream for the clientId.

**Stage (iii)** - Push Notification to the connected client
3. Push the notification read from the Redis Stream to the connected Client by persistent GRPC bi-directional streaming connection in Protobuf format.

**Stage (iv)** - Notification received on client end
4. De-Duplicate the notification received by the same “Notification ID” from both FCM and GRPC server.
5. Send an Acknowledgement to the Notification Server for the received notification over the persistent GRPC stream connection.

**Stage (v)** - Acknowledge the received notification
6. Delete the acknowledged notification from Redis Stream to avoid retries for it.

## Advantages

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
