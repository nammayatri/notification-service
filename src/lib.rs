pub mod server_side_streaming {
    tonic::include_proto!("server_side_streaming");

    impl User {
        pub fn new(id: &str, name: &str) -> Self {
            Self {
                id: id.to_owned(),
                name: name.to_owned(),
            }
        }
    }
}

pub mod client_side_streaming {
    tonic::include_proto!("client_side_streaming");
}

#[derive(Debug)]
pub struct TracingGuard {
    _log_guard: tracing_appender::non_blocking::WorkerGuard,
}

pub fn setup_tracing(binary_name: &'static str) -> TracingGuard {
    use tracing::Level;
    use tracing_subscriber::{
        filter, fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
    };

    // Create logging layer with non-blocking stdout writer
    let (console_writer, guard) = tracing_appender::non_blocking(std::io::stdout());
    let logging_layer = fmt::layer()
        .with_timer(fmt::time())
        .pretty()
        .with_writer(console_writer);

    // Set log/trace level for specific crates, with default level as `WARN`
    let crate_filter = filter::Targets::new()
        .with_default(Level::WARN)
        .with_target(std::env!("CARGO_PKG_NAME"), Level::TRACE)
        .with_target(binary_name, Level::TRACE);

    tracing_subscriber::
        // fmt().json()
        registry()
    .with(
        EnvFilter::builder()
            .with_default_directive(Level::TRACE.into())
            .from_env_lossy(),
    )
    .with(logging_layer.with_filter(crate_filter))
    .init();

    TracingGuard { _log_guard: guard }
}
