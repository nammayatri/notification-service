/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use anyhow::Result;
use futures::Stream;
use notification_service::notification_server::{Notification, NotificationServer};
use notification_service::utils::logger::setup_tracing;
use notification_service::{environment::AppConfig, utils::logger::*};
use notification_service::{NotificationAck, NotificationPayload};
use std::env::var;
use std::pin::Pin;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};

#[derive(Default)]
pub struct NotificationService {}

#[tonic::async_trait]
impl Notification for NotificationService {
    type StreamPayloadStream =
        Pin<Box<dyn Stream<Item = Result<NotificationPayload, Status>> + Send + Sync>>;

    async fn stream_payload(
        &self,
        request: Request<tonic::Streaming<NotificationAck>>,
    ) -> Result<Response<Self::StreamPayloadStream>, Status> {
        let (_tx, rx) = mpsc::channel(100000);

        println!("Connection Successful");

        tokio::spawn(async move {
            let mut stream = request.into_inner();

            for _ in 0..1000 {
                // tx.send(Ok(NotificationPayload {
                //     message: format!("{}", "x".repeat(payload_size)),
                // }))
                // .await
                // .unwrap();
                if let Ok(Some(message)) = stream.message().await {
                    println!("GOT A MESSAGE: {:?}", message); // Acknowledgment from the client
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 1)] // Adjust the number of worker threads as needed
async fn main() -> Result<()> {
    let dhall_config_path = var("DHALL_CONFIG")
        .unwrap_or_else(|_| "./dhall-configs/dev/notification_service.dhall".to_string());
    let app_config = serde_dhall::from_file(dhall_config_path).parse::<AppConfig>()?;

    let _guard = setup_tracing(app_config.logger_cfg);

    std::panic::set_hook(Box::new(|panic_info| {
        let payload = panic_info
            .payload()
            .downcast_ref::<&str>()
            .unwrap_or(&"Unknown panic");
        error!("Panic Occured : {payload}");
    }));

    let addr = format!("[::1]:{}", app_config.port).parse()?;
    let notification_service = NotificationService::default();

    Server::builder()
        .add_service(NotificationServer::new(notification_service))
        .serve(addr)
        .await?;

    Ok(())
}
