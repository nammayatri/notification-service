use std::{env, error::Error};

use sha256::digest;
use tonic::{transport::Channel, Request};

use self::chat::{chat_client::ChatClient, User};

pub mod chat {
    tonic::include_proto!("chat");
}

async fn recieve_message(
    client: &mut ChatClient<Channel>,
    name: &String,
    id: &String,
) -> Result<(), Box<dyn Error>> {
    let user = User {
        id: id.to_string(),
        name: name.to_string(),
    };

    let mut stream = client
        .recieve_message(Request::new(user))
        .await?
        .into_inner();

    while let Some(message) = stream.message().await? {
        println!("[MESSAGE] = {:?}", message);
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let name = args[1].clone();
    let id = digest(name.as_str());

    loop {
        let client_res = ChatClient::connect("https://127.0.0.1:5051").await;

        match client_res {
            Ok(mut client) => {
                if let Err(e) = recieve_message(&mut client, &name, &id).await {
                    eprintln!("[Error] {:?}", e);
                }
            }
            Err(e) => {
                eprintln!("[Error] {}", e);
            }
        }
    }
}
