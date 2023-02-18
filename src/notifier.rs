use grpc_rust::chat::{chat_client::ChatClient, Message};
use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry};
use redis::{
    streams::{StreamReadOptions, StreamReadReply},
    Client, Commands,
};
use tonic::Request;
use warp::{Filter, Rejection, Reply};

#[allow(clippy::expect_used)]
pub static CLIENT_MESSAGE_COLLECTOR: once_cell::sync::Lazy<IntCounterVec> =
    once_cell::sync::Lazy::new(|| {
        IntCounterVec::new(
            Opts::new("client_message", "Client Messages"),
            &["clientid", "messageid"],
        )
        .expect("client message collector metric couldn't be created")
    });
#[allow(clippy::expect_used)]
pub static CLIENT_MESSAGE_STATUS_COLLECTOR: once_cell::sync::Lazy<HistogramVec> =
    once_cell::sync::Lazy::new(|| {
        HistogramVec::new(
            HistogramOpts::new("client_message_status", "Client Messages Status"),
            &["clientid", "messageid", "status"],
        )
        .expect("client message collector metric couldn't be created")
    });
pub static REGISTRY: once_cell::sync::Lazy<Registry> = once_cell::sync::Lazy::new(Registry::new);

fn register_custom_metrics() {
    #[allow(clippy::expect_used)]
    REGISTRY
        .register(Box::new(CLIENT_MESSAGE_COLLECTOR.clone()))
        .expect("CLIENT_MESSAGE_COLLECTOR could not be registered");

    #[allow(clippy::expect_used)]
    REGISTRY
        .register(Box::new(CLIENT_MESSAGE_STATUS_COLLECTOR.clone()))
        .expect("CLIENT_MESSAGE_STATUS_COLLECTOR could not be registered");
}

async fn metrics_handler() -> Result<impl Reply, Rejection> {
    use prometheus::Encoder;
    let encoder = prometheus::TextEncoder::new();

    let mut buffer = Vec::new();
    if let Err(e) = encoder.encode(&REGISTRY.gather(), &mut buffer) {
        eprintln!("could not encode custom metrics: {}", e);
    };
    let mut res = match String::from_utf8(buffer.clone()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("custom metrics could not be from_utf8'd: {}", e);
            String::default()
        }
    };
    buffer.clear();

    let mut buffer = Vec::new();
    if let Err(e) = encoder.encode(&prometheus::gather(), &mut buffer) {
        eprintln!("could not encode prometheus metrics: {}", e);
    };
    let res_custom = match String::from_utf8(buffer.clone()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("prometheus metrics could not be from_utf8'd: {}", e);
            String::default()
        }
    };
    buffer.clear();

    res.push_str(&res_custom);
    Ok(res)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let redis_client = Client::open("redis://127.0.0.1/")?;
    println!("[REDIS CONNECTION]: redis://127.0.0.1/");

    register_custom_metrics();

    tokio::spawn(async move {
        loop {
            let mut redis_conn = redis_client.get_connection().unwrap();

            let stream_name = "chat-stream-1";
            let consumer_group_name = "chat-group-1";
            let consumer_name = "chat-consumer-1";

            let opts = StreamReadOptions::default()
                .group(consumer_group_name, consumer_name)
                .block(0);
            let result: StreamReadReply = redis_conn
                .xread_options(&[&stream_name], &[">"], &opts)
                .unwrap();

            for stream_key in result.keys {
                for stream in stream_key.ids {
                    let message = Message {
                        id: stream.clone().id,
                        to: stream.get("to").unwrap(),
                        content: stream.get("content").unwrap(),
                        ttl: stream.get("ttl").unwrap(),
                    };

                    CLIENT_MESSAGE_COLLECTOR.with_label_values(&[
                        stream.get::<String>("to").unwrap().as_str(),
                        message.id.as_str(),
                    ]);

                    println!(
                        "[XREAD] message-id : {}, stream-name : {}, consumer-group-name : {}, consumer-name : {}",
                        &message.id,
                        stream_name,
                        consumer_group_name,
                        consumer_name
                    );

                    let server_ip: String =
                        redis_conn.get(stream.get::<String>("to").unwrap()).unwrap();

                    println!(
                        "[SERVER - CLIENT] {} : {}",
                        stream.get::<String>("to").unwrap(),
                        server_ip
                    );

                    let mut server = ChatClient::connect(server_ip).await.unwrap();
                    match server.send_message(Request::new(message.clone())).await {
                        Ok(_) => {
                            println!("[SENT TO CLIENT - SUCCESS] message-id : {}, stream-name : {}, consumer-group-name : {}, consumer-name : {}", &message.id, stream_name, consumer_group_name, consumer_name);

                            let ack = redis_conn.xack::<_, _, _, redis::Value>(
                                &stream_name,
                                "chat-group",
                                &[&message.id],
                            );

                            CLIENT_MESSAGE_STATUS_COLLECTOR.with_label_values(&[
                                &stream.get::<String>("to").unwrap(),
                                message.id.as_str(),
                                "delivered",
                            ]);

                            match ack {
                                Ok(_) => {
                                    println!("[ACK - SUCCESS] message-id : {}, stream-name : {}, consumer-group-name : {}, consumer-name : {}", &message.id, stream_name, consumer_group_name, consumer_name);
                                }
                                Err(e) => {
                                    eprintln!("[ACK - ERROR] message-id : {}, stream-name : {}, consumer-group-name : {}, consumer-name : {}, error-message : {}", &message.id, stream_name, consumer_group_name, consumer_name, e);
                                }
                            }

                            let del =
                                redis_conn.xdel::<_, _, redis::Value>(&stream_name, &[&message.id]);

                            match del {
                                Ok(_) => {
                                    println!("[DEL - SUCCESS] message-id : {}, stream-name : {}, consumer-group-name : {}, consumer-name : {}", &message.id, stream_name, consumer_group_name, consumer_name);
                                }
                                Err(e) => {
                                    eprintln!("[DEL - ERROR] message-id : {}, stream-name : {}, consumer-group-name : {}, consumer-name : {}, error-message : {}", &message.id, stream_name, consumer_group_name, consumer_name, e);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[SENT TO CLIENT - ERROR] message-id : {}, stream-name : {}, consumer-group-name : {}, consumer-name : {}, error-message : {}", &message.id, stream_name, consumer_group_name, consumer_name, e);

                            CLIENT_MESSAGE_STATUS_COLLECTOR.with_label_values(&[
                                stream.get::<String>("to").unwrap().as_str(),
                                message.id.as_str(),
                                "pending",
                            ]);

                            let claim = redis_conn.xclaim::<_, _, _, _, _, redis::Value>(
                                &stream_name,
                                &consumer_group_name,
                                &consumer_name,
                                0,
                                &[&message.id],
                            );

                            match claim {
                                Ok(_) => {
                                    println!("[CLAIM - SUCCESS] message-id : {}, stream-name : {}, consumer-group-name : {}, consumer-name : {}", &message.id, stream_name, consumer_group_name, consumer_name);
                                }
                                Err(e) => {
                                    eprintln!("[CLAIM - ERROR] message-id : {}, stream-name : {}, consumer-group-name : {}, consumer-name : {}, error-message : {}", &message.id, stream_name, consumer_group_name, consumer_name, e);
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    warp::serve(warp::path!("metrics").and_then(metrics_handler))
        .run(([0, 0, 0, 0], 5050))
        .await;

    Ok(())
}
