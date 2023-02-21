use std::{env, time::Duration};

use chrono::Utc;
use grpc_rust::client_side_streaming::{
    client_stream_client::ClientStreamClient, Location, Message,
};
use sha256::digest;
use tokio::time;
use tonic::{metadata::MetadataValue, transport::Channel, Request};

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let name = args[1].clone();
    let _id = digest(name.as_str());

    let channel = Channel::from_static("https://127.0.0.1:50051")
        .connect()
        .await
        .unwrap();

    let token: MetadataValue<_> = "none".parse().unwrap();

    let mut client = ClientStreamClient::with_interceptor(channel, move |mut req: Request<()>| {
        req.metadata_mut().insert("token", token.clone());
        Ok(req)
    });

    let request = async_stream::stream! {
        let mut interval = time::interval(Duration::from_secs(10));
        loop {
            let time = interval.tick().await;
            let message = Message {
                accuracy: 0.0,
                timestamp: Utc::now().to_rfc2822(),
                location : Some(Location {
                    lat : 1.0,
                    long : 1.0,
                })
            };

            yield message
        }
    };

    client.send_message(request).await.unwrap();
}
