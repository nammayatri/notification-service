use std::{env, error::Error};

use grpc_rust::server_side_streaming::{server_stream_client::ServerStreamClient, User};
use tonic::{transport::Channel, Request};
use tracing::{error, info};

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
        info!(?message);
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let _guard = grpc_rust::setup_tracing(std::env!("CARGO_BIN_NAME"));

    let args: Vec<String> = env::args().collect();
    let name = args[1].clone();
    let id = sha256::digest(name.as_str());

    loop {
        let client_res = ServerStreamClient::connect("https://127.0.0.1:50051").await;

        match client_res {
            Ok(mut client) => {
                if let Err(error) = receive_message(&mut client, &name, &id).await {
                    error!(%error);
                }
            }
            Err(error) => {
                error!(%error);
            }
        }
    }
}