use prometheus::{HistogramOpts, HistogramVec, IntGauge, Registry};
use tracing::error;

#[allow(clippy::expect_used)]
pub static INCOMING_REQUESTS: once_cell::sync::Lazy<HistogramVec> =
    once_cell::sync::Lazy::new(|| {
        HistogramVec::new(
            HistogramOpts::new("incoming_requests", "Incoming Requests"),
            &["clientid", "timestamp"],
        )
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
            &["requestid", "clientid", "status", "timestamp"],
        )
        .expect("client message collector metric couldn't be created")
    });
pub static REGISTRY: once_cell::sync::Lazy<Registry> = once_cell::sync::Lazy::new(Registry::new);

pub fn register_custom_metrics() {
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

pub async fn metrics_handler() -> Result<String, String> {
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
