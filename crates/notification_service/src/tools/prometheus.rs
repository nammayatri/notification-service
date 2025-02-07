/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/
#![allow(clippy::expect_used)]

extern crate shared;

use actix_web_prom::PrometheusMetrics;
use prometheus::{
    histogram_opts, opts, register_histogram_vec, register_int_counter, register_int_gauge,
    HistogramVec, IntCounter, IntGauge,
};
pub use shared::tools::prometheus::*;

pub static NOTIFICATION_CLIENT_CONNECTION_DURATION: once_cell::sync::Lazy<HistogramVec> =
    once_cell::sync::Lazy::new(|| {
        register_histogram_vec!(
            histogram_opts!(
                "notification_client_connection_duration",
                "Notification Client Connection Duration",
                vec![
                    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0,
                    5.5, 6.0, 6.5, 7.0, 7.5, 8.0, 8.5, 9.0, 9.5, 10.0, 20.0, 30.0, 40.0, 50.0,
                    60.0, 70.0, 80.0, 90.0, 100.0, 200.0, 300.0, 400.0,
                ]
            ),
            &["status"]
        )
        .expect("Failed to register notification client connection duration")
    });

pub static INCOMING_API: once_cell::sync::Lazy<HistogramVec> = once_cell::sync::Lazy::new(|| {
    register_histogram_vec!(
        opts!("grpc_request_duration_seconds", "Incoming API requests").into(),
        &["method", "handler", "status_code", "code", "version"]
    )
    .expect("Failed to register incoming API metrics")
});

pub static NOTIFICATION_LATENCY: once_cell::sync::Lazy<HistogramVec> =
    once_cell::sync::Lazy::new(|| {
        register_histogram_vec!(
            opts!("notification_duration_seconds", "Notification Latency").into(),
            &["version", "ack"]
        )
        .expect("Failed to register notifiction latency metrics")
    });

pub static CHANNEL_DELAY: once_cell::sync::Lazy<HistogramVec> = once_cell::sync::Lazy::new(|| {
    register_histogram_vec!(
        opts!("channel_delay", "Channel Delay").into(),
        &["version", "name"]
    )
    .expect("Failed to register channel delay metrics")
});

pub static CONNECTED_CLIENTS: once_cell::sync::Lazy<IntGauge> = once_cell::sync::Lazy::new(|| {
    register_int_gauge!("connected_clients", "Connected Clients")
        .expect("Failed to register connected clients metrics")
});

pub static TOTAL_NOTIFICATIONS: once_cell::sync::Lazy<IntCounter> =
    once_cell::sync::Lazy::new(|| {
        register_int_counter!("total_notifications", "Total Notifications")
            .expect("Failed to register total notifications metrics")
    });

pub static DELIVERED_NOTIFICATIONS: once_cell::sync::Lazy<IntCounter> =
    once_cell::sync::Lazy::new(|| {
        register_int_counter!("delivered_notifications", "Delivered Notifications")
            .expect("Failed to register delivered notifications metrics")
    });

pub static RETRIED_NOTIFICATIONS: once_cell::sync::Lazy<IntCounter> =
    once_cell::sync::Lazy::new(|| {
        register_int_counter!("retried_notifications", "Retried Notifications")
            .expect("Failed to register retried notifications metrics")
    });

pub static EXPIRED_NOTIFICATIONS: once_cell::sync::Lazy<IntCounter> =
    once_cell::sync::Lazy::new(|| {
        register_int_counter!("expired_notifications", "Expired Notifications")
            .expect("Failed to register expired notifications metrics")
    });

pub static RWLOCK_DELAY: once_cell::sync::Lazy<HistogramVec> = once_cell::sync::Lazy::new(|| {
    register_histogram_vec!(
        histogram_opts!(
            "rwlock_delay",
            "Time taken to acquire RwLock (contention delay)"
        ),
        &["name", "type"] // Dimensions: Lock Name & Operation Type
    )
    .expect("Failed to register RwLock latency metric")
});

#[macro_export]
macro_rules! notification_client_connection_duration {
    ($status:expr, $start:expr) => {
        let duration = $start.elapsed().as_secs_f64();
        NOTIFICATION_CLIENT_CONNECTION_DURATION
            .with_label_values(&[$status])
            .observe(duration);
    };
}

#[macro_export]
macro_rules! incoming_api {
    ($method:expr, $endpoint:expr, $status:expr, $code:expr, $start:expr) => {
        let duration = $start.elapsed().as_secs_f64();
        let version = std::env::var("DEPLOYMENT_VERSION").unwrap_or("DEV".to_string());
        INCOMING_API
            .with_label_values(&[$method, $endpoint, $status, $code, version.as_str()])
            .observe(duration);
    };
}

#[macro_export]
macro_rules! notification_latency {
    ($start:expr, $ack:expr) => {
        let now = Utc::now();
        let duration = abs_diff_utc_as_sec($start, now);
        let version = std::env::var("DEPLOYMENT_VERSION").unwrap_or("DEV".to_string());
        NOTIFICATION_LATENCY
            .with_label_values(&[version.as_str(), $ack])
            .observe(duration);
    };
}

#[macro_export]
macro_rules! channel_delay {
    ($start:expr, $name:expr) => {
        let now = Utc::now();
        let duration = abs_diff_utc_as_sec($start, now);
        let version = std::env::var("DEPLOYMENT_VERSION").unwrap_or("DEV".to_string());
        CHANNEL_DELAY
            .with_label_values(&[version.as_str(), $name])
            .observe(duration);
    };
}

/// Initializes and returns a `PrometheusMetrics` instance configured for the application.
///
/// This function sets up Prometheus metrics for various application processes, including incoming and external API requests, queue counters, and queue drainer latencies.
/// It also provides an endpoint (`/metrics`) for Prometheus to scrape these metrics.
///
/// # Examples
///
/// ```norun
/// fn main() {
///     HttpServer::new(move || {
///         App::new()
///             .wrap(prometheus_metrics()) // Using the prometheus_metrics function
///     })
///     .bind("127.0.0.1:8080").unwrap()
///     .run();
/// }
/// ```
///
/// # Returns
///
/// * `PrometheusMetrics` - A configured instance that collects and exposes the metrics.
///
/// # Panics
///
/// * If there's a failure initializing metrics, registering metrics to the Prometheus registry, or any other unexpected error during the setup.
pub fn prometheus_metrics() -> PrometheusMetrics {
    let prometheus = init_prometheus_metrics();

    prometheus
        .registry
        .register(Box::new(NOTIFICATION_CLIENT_CONNECTION_DURATION.to_owned()))
        .expect("Failed to register notification client connection duration");

    prometheus
        .registry
        .register(Box::new(INCOMING_API.to_owned()))
        .expect("Failed to register incoming API metrics");

    prometheus
        .registry
        .register(Box::new(NOTIFICATION_LATENCY.to_owned()))
        .expect("Failed to register notification latency metrics");

    prometheus
        .registry
        .register(Box::new(CHANNEL_DELAY.to_owned()))
        .expect("Failed to register channel delay metrics");

    prometheus
        .registry
        .register(Box::new(CONNECTED_CLIENTS.to_owned()))
        .expect("Failed to register connected clients metrics");

    prometheus
        .registry
        .register(Box::new(RETRIED_NOTIFICATIONS.to_owned()))
        .expect("Failed to register total notifications metrics");

    prometheus
        .registry
        .register(Box::new(TOTAL_NOTIFICATIONS.to_owned()))
        .expect("Failed to register total notifications metrics");

    prometheus
        .registry
        .register(Box::new(DELIVERED_NOTIFICATIONS.to_owned()))
        .expect("Failed to register delivered notifications metrics");

    prometheus
        .registry
        .register(Box::new(EXPIRED_NOTIFICATIONS.to_owned()))
        .expect("Failed to register expired notifications metrics");

    prometheus
        .registry
        .register(Box::new(RWLOCK_DELAY.to_owned()))
        .expect("Failed to register rwlock delay metric");

    prometheus
}
