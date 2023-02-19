use std::{env, error::Error, time::Duration};

use grpc_rust::client_side_streaming::{
    client_stream_client::ClientStreamClient, Location, Message,
};
use sha256::digest;
use tokio::time;
use tonic::transport::Channel;

async fn send_message(
    client: &mut ClientStreamClient<Channel>,
    id: &String,
) -> Result<(), Box<dyn Error>> {
    let id = id.clone();
    let request = async_stream::stream! {
        let mut interval = time::interval(Duration::from_secs(3));
        loop {
            let time = interval.tick().await;
            let message = Message {
                id: "1".to_string(),
                client_id: id.to_string(),
                location : Some(Location {
                    lat : "1".to_string(),
                    long : "2".to_string(),
                })
            };

            yield message
        }
    };

    match client.send_message(request).await {
        Ok(response) => {
            println!("[DISCONNECTED] {:?}", response.into_inner())
        }
        Err(e) => eprintln!("[DISCONNECTED - ERROR] {:?}", e),
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let name = args[1].clone();
    let id = digest(name.as_str());

    // loop {
    let client_res = ClientStreamClient::connect("https://127.0.0.1:50051").await;

    match client_res {
        Ok(mut client) => {
            if let Err(e) = send_message(&mut client, &id).await {
                eprintln!("[ERROR] {:?}", e);
            }
        }
        Err(e) => {
            eprintln!("[ERROR] {}", e);
        }
    }
    // }
}
