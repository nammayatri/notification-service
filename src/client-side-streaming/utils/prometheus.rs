use prometheus::{HistogramOpts, HistogramVec, IntGauge, Registry};

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
