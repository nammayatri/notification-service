use std::{env, error::Error};

use grpc_rust::server_side_streaming::{server_stream_client::ServerStreamClient, User};
use tonic::{transport::Channel, Request};

async fn receive_message(
    client: &mut ServerStreamClient<Channel>,
    name: &str,
    id: &str,
) -> Result<(), Box<dyn Error>> {
    let user = User::new(id, name);
    let mut stream = client
        .receive_message(Request::new(user))
        .await?
        .into_inner();

    while let Some(message) = stream.message().await? {
        println!("[MESSAGE] = {message:?}");
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let name = args[1].clone();
    let id = sha256::digest(name.as_str());

    loop {
        let client_res = ServerStreamClient::connect("https://127.0.0.1:5051").await;

        match client_res {
            Ok(mut client) => {
                if let Err(e) = receive_message(&mut client, &name, &id).await {
                    eprintln!("[Error] {e:?}");
                }
            }
            Err(e) => {
                eprintln!("[Error] {e}");
            }
        }
    }
}
