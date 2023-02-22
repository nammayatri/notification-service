use chrono::Utc;
use futures::TryStreamExt;
use grpc_rust::client_side_streaming::{client_stream_server::ClientStream, Empty, Messages};
use hyper::header::CONTENT_TYPE;
use tonic::{Request, Response, Status};
use tracing::{error, info};
use utils::prometheus;
use uuid::Uuid;

use crate::{types, utils};

#[derive(Debug)]
pub struct ClientStreamService {
    config: types::config::AppConfig,
}

impl ClientStreamService {
    pub fn new(config: types::config::AppConfig) -> Self {
        Self { config }
    }
}

#[tonic::async_trait]
impl ClientStream for ClientStreamService {
    async fn send_message(
        &self,
        request: Request<tonic::Streaming<Messages>>,
    ) -> Result<Response<Empty>, Status> {
        let metadata = request.metadata().clone();
        let token = metadata
            .get("token")
            .expect("[token] is missing in metadata")
            .to_str()
            .unwrap();
        let client_version = metadata
            .get("x-client-version")
            .expect("[x-client-version] is missing in metadata")
            .to_str()
            .unwrap();
        let bundle_version = metadata
            .get("x-bundle-version")
            .expect("[x-bundle-version] is missing in metadata")
            .to_str()
            .unwrap();

        prometheus::CONNECTED_CLIENTS.inc();
        info!("[CONNECTED]");

        prometheus::INCOMING_REQUESTS.with_label_values(&[token, Utc::now().to_rfc2822().as_str()]);

        let mut stream = request.into_inner();

        while let Ok(Some(messages)) = stream.try_next().await {
            let mut body: Vec<types::api::APIRequest> = vec![];
            for message in messages.messages.iter() {
                match message.clone().location {
                    Some(location) => {
                        body.push(types::api::APIRequest {
                            pt: types::api::Location {
                                lat: location.lat,
                                lon: location.long,
                            },
                            ts: message.clone().timestamp,
                        });
                    }
                    None => error!(tag = "[MESSAGE - ERROR]", message = "location is missing"),
                };
            }

            info!(tag = "[API REQUEST BODY]", ?body);

            if self.config.api.enabled && body.len() > 0 {
                let request_id = Uuid::new_v4();
                let client = reqwest::Client::new();
                let response = client
                    .post(format!("{}/driver/location", self.config.api.base_url))
                    .header(CONTENT_TYPE, "application/json")
                    .header("x-request-id", request_id.to_string())
                    .header("x-grpc-id", request_id.to_string())
                    .header("x-client-version", client_version.to_string())
                    .header("x-bundle-version", bundle_version.to_string())
                    .header("token", token.to_string())
                    .json(&body)
                    .send()
                    .await;

                info!(tag = "[API RESPONSE]", ?response);

                if let Ok(response) = response {
                    prometheus::CLIENT_MESSAGE_STATUS_COLLECTOR.with_label_values(&[
                        request_id.to_string().as_str(),
                        token,
                        response.status().as_str(),
                        Utc::now().to_rfc2822().as_str(),
                    ]);

                    match response.status() {
                        reqwest::StatusCode::OK => {
                            match response.json::<types::api::APIResponse>().await {
                                Ok(response) => {
                                    info!(tag = "[API RESPONSE DECODE - SUCCESS]", ?response);
                                }
                                Err(error) => {
                                    error!(tag = "[API RESPONSE DECODE - ERROR]", %error);
                                    return Err(Status::unavailable(error.to_string()));
                                }
                            };
                        }
                        status => {
                            error!(tag = "[API RESPONSE STATUS CODE - ERROR]", %status);
                            return Err(Status::unavailable(status.to_string()));
                        }
                    };
                }
            }
        }

        prometheus::CONNECTED_CLIENTS.dec();
        info!("[DISCONNECTED]");

        Ok(Response::new(Empty::default()))
    }
}

pub fn check_auth(req: Request<()>) -> Result<Request<()>, Status> {
    match req.metadata().get("token") {
        Some(_t) if true => Ok(req),
        _ => Err(Status::unauthenticated(
            "invalid auth token, please try again with valid token",
        )),
    }
}
