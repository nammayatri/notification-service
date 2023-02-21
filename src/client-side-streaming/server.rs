use std::{
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use config::{Config, File, FileFormat};
use futures::{
    future::{self, Either},
    TryFutureExt, TryStreamExt,
};
use grpc_rust::client_side_streaming::{
    client_stream_server::{ClientStream, ClientStreamServer},
    Empty, Message, Messages,
};
use hyper::{header::CONTENT_TYPE, http, service::make_service_fn, Server};
use prometheus::{HistogramOpts, HistogramVec, IntCounter, IntGauge, Registry};
use serde::{Deserialize, Serialize};
use tonic::{transport::Server as TonicServer, Request, Response, Status};
use tower::Service;
use tracing::{error, info};
use uuid::Uuid;
use warp::{Filter, Rejection, Reply};

#[allow(clippy::expect_used)]
pub static INCOMING_REQUESTS: once_cell::sync::Lazy<IntCounter> =
    once_cell::sync::Lazy::new(|| {
        IntCounter::new("incoming_requests", "Incoming Requests")
            .expect("incoming requests metric couldn't be created")
    });
#[allow(clippy::expect_used)]
pub static CONNECTED_CLIENTS: once_cell::sync::Lazy<IntGauge> = once_cell::sync::Lazy::new(|| {
    IntGauge::new("connected_clients", "Connected Clients")
        .expect("connected clients metric couldn't be created")
});
#[allow(clippy::expect_used)]
pub static CLIENT_MESSAGE_STATUS_COLLECTOR: once_cell::sync::Lazy<HistogramVec> =
    once_cell::sync::Lazy::new(|| {
        HistogramVec::new(
            HistogramOpts::new("client_message_status", "Client Messages Status"),
            &["requestid", "status"],
        )
        .expect("client message collector metric couldn't be created")
    });
pub static REGISTRY: once_cell::sync::Lazy<Registry> = once_cell::sync::Lazy::new(Registry::new);

fn register_custom_metrics() {
    #[allow(clippy::expect_used)]
    REGISTRY
        .register(Box::new(INCOMING_REQUESTS.clone()))
        .expect("`INCOMING_REQUESTS` collector couldn't be registered");

    #[allow(clippy::expect_used)]
    REGISTRY
        .register(Box::new(CONNECTED_CLIENTS.clone()))
        .expect("`CONNECTED_CLIENTS` collector couldn't be registered");

    #[allow(clippy::expect_used)]
    REGISTRY
        .register(Box::new(CLIENT_MESSAGE_STATUS_COLLECTOR.clone()))
        .expect("`CLIENT_MESSAGE_STATUS_COLLECTOR` collector couldn't be registered");
}

#[derive(Debug)]
struct ClientStreamService {
    config: AppConfig,
}

impl ClientStreamService {
    fn new(config: AppConfig) -> Self {
        Self { config }
    }
}

#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    lat: f64,
    lon: f64,
}

#[derive(Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct APIRequest {
    pt: Location,
    ts: String,
}

#[derive(Default, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct APIResponse {
    _result: String,
}

#[tonic::async_trait]
impl ClientStream for ClientStreamService {
    async fn send_message(
        &self,
        request: Request<tonic::Streaming<Messages>>,
    ) -> Result<Response<Empty>, Status> {
        INCOMING_REQUESTS.inc();
        CONNECTED_CLIENTS.inc();
        info!("[CONNECTED]");

        let request_id = Uuid::new_v4();
        let metadata = request.metadata().clone();
        let token = metadata.get("token").unwrap().to_str().unwrap();
        let client_version = metadata.get("x-client-version").unwrap().to_str().unwrap();
        let bundle_version = metadata.get("x-bundle-version").unwrap().to_str().unwrap();

        let mut stream = request.into_inner();

        while let Ok(Some(messages)) = stream.try_next().await {
            let body: Vec<APIRequest> = messages
                .messages
                .into_iter()
                .map(|message| APIRequest {
                    pt: Location {
                        lat: message.clone().location.map_or(0.0, |loc| loc.lat),
                        lon: message.clone().location.map_or(0.0, |loc| loc.long),
                    },
                    ts: message.clone().timestamp,
                })
                .collect();

            info!(tag = "[API REQUEST BODY]", ?body);

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
                .await
                .unwrap();

            info!(tag = "[API RESPONSE]", ?response);

            CLIENT_MESSAGE_STATUS_COLLECTOR
                .with_label_values(&[request_id.to_string().as_str(), response.status().as_str()]);

            match response.status() {
                reqwest::StatusCode::OK => {
                    match response.json::<APIResponse>().await {
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

        CONNECTED_CLIENTS.dec();
        info!("[DISCONNECTED]");

        Ok(Response::new(Empty::default()))
    }
}

async fn metrics_handler() -> Result<impl Reply, Rejection> {
    use prometheus::Encoder;
    let encoder = prometheus::TextEncoder::new();

    let mut buffer = Vec::new();
    if let Err(error) = encoder.encode(&REGISTRY.gather(), &mut buffer) {
        error!(%error, "could not encode custom metrics");
    };
    let mut res = match String::from_utf8(buffer.clone()) {
        Ok(v) => v,
        Err(error) => {
            error!(%error, "custom metrics could not be converted from bytes");
            String::default()
        }
    };
    buffer.clear();

    let mut buffer = Vec::new();
    if let Err(error) = encoder.encode(&prometheus::gather(), &mut buffer) {
        error!(%error, "could not encode prometheus metrics");
    };
    let res_custom = match String::from_utf8(buffer.clone()) {
        Ok(v) => v,
        Err(error) => {
            error!(%error, "prometheus metrics could not be converted from bytes");
            String::default()
        }
    };
    buffer.clear();

    res.push_str(&res_custom);
    Ok(res)
}

fn check_auth(req: Request<()>) -> Result<Request<()>, Status> {
    match req.metadata().get("token") {
        Some(_t) if true => Ok(req),
        _ => Err(Status::unauthenticated("No valid auth token")),
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default)]
struct ServerConfig {
    host_ip: String,
    port: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default)]
struct ApiConfig {
    base_url: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(default)]
struct AppConfig {
    server: ServerConfig,
    api: ApiConfig,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::builder()
        .add_source(File::new(
            "config/client-side-streaming/Config.toml",
            FileFormat::Toml,
        ))
        .build()
        .expect("failed in constructing application config");

    let config: AppConfig =
        serde_path_to_error::deserialize(config).expect("failed in decoding application config");

    let _guard = grpc_rust::setup_tracing(std::env!("CARGO_BIN_NAME"));

    register_custom_metrics();

    let addr = format!("{}:{}", config.server.host_ip, config.server.port)
        .parse()
        .expect("socket address couldn't be parsed");
    info!(tag = "[SERVER LISTENING]", "{addr}");

    let mut warp = warp::service(warp::path("metrics").and_then(metrics_handler));

    Server::bind(&addr)
        .http2_keep_alive_interval(Some(Duration::from_secs(5)))
        .http2_keep_alive_timeout(Duration::from_secs(20))
        .serve(make_service_fn(move |_| {
            let config = config.clone();
            future::ok::<_, Infallible>(tower::service_fn(
                move |req: hyper::Request<hyper::Body>| match req.uri().path() {
                    "/metrics" => Either::Left(
                        warp.call(req)
                            .map_ok(|res| res.map(EitherBody::Left))
                            .map_err(Error::from),
                    ),
                    _ => {
                        let svc = ClientStreamServer::with_interceptor(
                            ClientStreamService::new(config.clone()),
                            check_auth,
                        );
                        let mut tonic = TonicServer::builder().add_service(svc).into_service();

                        Either::Right(
                            tonic
                                .call(req)
                                .map_ok(|res| res.map(EitherBody::Right))
                                .map_err(Error::from),
                        )
                    }
                },
            ))
        }))
        .await?;

    Ok(())
}

type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

enum EitherBody<A, B> {
    Left(A),
    Right(B),
}

impl<A, B> http_body::Body for EitherBody<A, B>
where
    A: http_body::Body + Send + Unpin,
    B: http_body::Body<Data = A::Data> + Send + Unpin,
    A::Error: Into<Error>,
    B::Error: Into<Error>,
{
    type Data = A::Data;
    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Left(b) => b.is_end_stream(),
            Self::Right(b) => b.is_end_stream(),
        }
    }

    fn poll_data(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Self::Data, Self::Error>>> {
        match self.get_mut() {
            Self::Left(b) => Pin::new(b).poll_data(cx).map(map_option_err),
            Self::Right(b) => Pin::new(b).poll_data(cx).map(map_option_err),
        }
    }

    fn poll_trailers(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<http::HeaderMap>, Self::Error>> {
        match self.get_mut() {
            Self::Left(b) => Pin::new(b).poll_trailers(cx).map_err(Into::into),
            Self::Right(b) => Pin::new(b).poll_trailers(cx).map_err(Into::into),
        }
    }
}

fn map_option_err<T, U: Into<Error>>(err: Option<Result<T, U>>) -> Option<Result<T, Error>> {
    err.map(|e| e.map_err(Into::into))
}
