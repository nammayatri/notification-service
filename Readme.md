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

## Advantages

1. Since we would have full control of the messages, we can also retry the undelivered messages again before they expire.
2. We can introduce the right tooling and metrics to help us understand the message success rate and improve further.
3. On testing, we found that if the client's wifi is turned off then the messages get sent once the client comes back on in some time, so it handles the cases where bandwidth fluctuates and gives us some reliability.
4. We can also set a keep alive ping that can ask for acknowledgement from clients to be sure the client is not away for too long. If so then, we can forcefully terminate the connection.

## GRPC Server Side Streaming Architecture

![GRPC Streaming](https://user-images.githubusercontent.com/38260510/219075736-baca827e-6516-4d72-9013-f466fbcd7a13.png)

1. Backend service can keep pushing new messages for the client to the Redis Streams.
2. Notification servers can keep reading new messages from the streams and discover the GRPC server to which the client is connected to through service discovery and then send the message to the GRPC server.
3. GRPC server then has to stream the message to the client and return the Success or Failure response to the Notification server based on which the server can acknowledge that message in the stream and move to the next message.
4. In case if the client has lost network bandwidth, then the health check ping pong could fail and forcefully disconnect the client from the stream.
5. The messages that were consumed by notification servers but were unable to be sent/acknowledged from the clients. Would be added as Pending messages in the consumer group which could then be retriggered at some regular intervals.
6. On top of all we can have the right metrics that would help us in debugging the messages that were sent and not sent to the client.

## Setup

### GRPC Server

1. install prometheus and run `prometheus` in root directory, it will start prometheus server on **localhost:9090**.
2. run `cargo run --bin sss-server`, it starts server on **localhost:5051**.

### Client

1. run `cargo run --bin sss-client <name>`, client connects to server and the stream starts.
2. Also, client add it's id to the Redis Service Discovery (clientId : serverIp).

### Notification Server

1. run `cargo run --bin sss-notifier`, notifier starts listening to the new messages from redis-streams.
2. For every new message it identifies the grpc-server to which the client is connected to through redis service discovery.
3. It then send the message to the correct GRPC server, which then broadcasts it to the client down the stream.

### Redis Stream

1. create the required redis stream and it's consumer, `XGROUP CREATE chat-stream-1 chat-group-1 $ MKSTREAM`
2. add new message to the stream, `XADD chat-stream-1 * content helloworld to 4d5548b231641d024b901f321fe8c1265dcb38bd2d514d2ee23bc55ab124f676 ttl 36000`

## GRPC Client Side Streaming Architecture

1. The client connects to the GRPC server and keeps sending location updates to the server on the channel.
2. Server then send the location updates for that client to the Backend service via HTTP call to an Rest API.
3. Prometheus is integrated for continus metrics of the following parameters:
    - Total No. of Incoming Client Connections.
    - Total No. of Connected Clients.
    - Total No. of Disconnected Clients.
    - Status of Messages Recieved by the Clients.

## Setup

### GRPC Server

1. install prometheus and run `prometheus` in root directory, it will start prometheus server on **localhost:9090**.
2. run `cargo run --bin css-server`, it starts server on **localhost:5051**.
3. to run by overriding configs from terminal, `CONFIG_SERVER__PORT="50751" cargo run --bin css_server` this will override **server.port** from **config/Config.toml**.

### Android Client

https://user-images.githubusercontent.com/38260510/219955643-10f7221f-4741-411a-9b78-75f384e4f2e1.mp4

1. open `src/client-side-streaming/android-client` in Android Studio and run the application.
2. application will keep sending messages to client with a delay of 10 seconds.

### [OPTIONAL] Android Client

1. run `cargo run --bin css-client <name>`, client connects to server and starts streaming messages to the server every 10 seconds.

## Run In Docker

1. to run along with prometheus, `docker compose up client_stream_service prometheus`.
2. build the docker image and run the image `docker build -t client_stream_service . && docker run -p 50051:50051 -d client_stream_service`.
3. to generate a shareable tar file of docker image, `docker save -o ./css_server.tar client_stream_service`.

## Raising PR Guidelines
1. install nightly toolchain for rustc, `rustup install nightly`.
2. format code with, `rustup run nightly cargo fmt --all`.