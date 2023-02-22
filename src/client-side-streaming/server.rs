mod services;
mod types;
mod utils;

use std::time::Duration;

use config::{Config, Environment, File, FileFormat};
use grpc_rust::client_side_streaming::client_stream_server::ClientStreamServer;
use services::client_stream::{check_auth, ClientStreamService};
use tonic::transport::Server;
use tracing::info;
use utils::{hybrid, prometheus};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::builder()
        .add_source(File::new(
            "config/client-side-streaming/Config.toml",
            FileFormat::Toml,
        ))
        .add_source(
            Environment::with_prefix("CONFIG")
                .prefix_separator("_")
                .separator("__")
                .keep_prefix(false),
        )
        .build()
        .expect("failed in constructing application config");

    let config: types::config::AppConfig =
        serde_path_to_error::deserialize(config).expect("failed in decoding application config");

    let _guard = grpc_rust::setup_tracing(std::env!("CARGO_BIN_NAME"));

    prometheus::register_custom_metrics();

    let addr = format!("{}:{}", config.server.host_ip, config.server.port)
        .parse()
        .expect("socket address couldn't be parsed");
    info!(tag = "[SERVER LISTENING]", "{addr}");

    let web_service = axum::Router::new()
        .route("/metrics", axum::routing::get(prometheus::metrics_handler))
        .into_make_service();

    let svc =
        ClientStreamServer::with_interceptor(ClientStreamService::new(config.clone()), check_auth);
    let grpc_service = Server::builder().add_service(svc).into_service();

    let hybrid_make_service = hybrid::hybrid(web_service, grpc_service);

    hyper::Server::bind(&addr)
        .http2_keep_alive_interval(Some(Duration::from_secs(5)))
        .http2_keep_alive_timeout(Duration::from_secs(20))
        .serve(hybrid_make_service)
        .await?;

    Ok(())
}
