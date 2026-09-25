/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use anyhow::Result;
use prometheus::{histogram_opts, opts, HistogramVec, IntCounterVec, IntGaugeVec, Registry};
use sim_common::metrics::Latencies;
use std::sync::Arc;

const LATENCY_BUCKETS: &[f64] = &[
    0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];

pub struct Metrics {
    pub registry: Registry,
    pub connected: IntGaugeVec,
    pub received: IntCounterVec,
    pub duplicates: IntCounterVec,
    pub reconnects: IntCounterVec,
    pub connect_failures: IntCounterVec,
    pub acks_sent: IntCounterVec,
    pub never_acked: IntCounterVec,
    pub ack_latency: HistogramVec,
    pub delivery_latency: HistogramVec,
    pub ack_percentiles: Latencies,
    pub delivery_percentiles: Latencies,
}

impl Metrics {
    pub fn new() -> Result<Arc<Self>> {
        let registry = Registry::new();

        let connected = IntGaugeVec::new(
            opts!(
                "sim_connected_clients",
                "Simulated clients holding a stream"
            ),
            &["endpoint"],
        )?;
        let received = IntCounterVec::new(
            opts!(
                "sim_notifications_received_total",
                "Notification payloads received by simulated clients"
            ),
            &["endpoint"],
        )?;
        let duplicates = IntCounterVec::new(
            opts!(
                "sim_duplicate_deliveries_total",
                "Notification ids seen more than once by the same simulated client"
            ),
            &["endpoint", "phase"],
        )?;
        let reconnects = IntCounterVec::new(
            opts!(
                "sim_reconnects_total",
                "Stream closures that forced a simulated client to reconnect"
            ),
            &["endpoint", "reason"],
        )?;
        let connect_failures = IntCounterVec::new(
            opts!(
                "sim_connect_failures_total",
                "Failed attempts to open a StreamPayload stream"
            ),
            &["endpoint"],
        )?;
        let acks_sent = IntCounterVec::new(
            opts!("sim_acks_sent_total", "Acks written to the request stream"),
            &["endpoint"],
        )?;
        let never_acked = IntCounterVec::new(
            opts!(
                "sim_notifications_never_acked_total",
                "Payloads deliberately left unacked by the never_ack_pct cohort"
            ),
            &["endpoint"],
        )?;
        let ack_latency = HistogramVec::new(
            histogram_opts!(
                "sim_ack_think_time_seconds",
                "Scripted client think-time from sim.dhall, NOT a server measurement",
                LATENCY_BUCKETS.to_vec()
            ),
            &["endpoint"],
        )?;
        let delivery_latency = HistogramVec::new(
            histogram_opts!(
                "sim_delivery_latency_seconds",
                "produced_at_ms in entity.data to payload receipt",
                LATENCY_BUCKETS.to_vec()
            ),
            &["endpoint"],
        )?;

        registry.register(Box::new(connected.clone()))?;
        registry.register(Box::new(received.clone()))?;
        registry.register(Box::new(duplicates.clone()))?;
        registry.register(Box::new(reconnects.clone()))?;
        registry.register(Box::new(connect_failures.clone()))?;
        registry.register(Box::new(acks_sent.clone()))?;
        registry.register(Box::new(never_acked.clone()))?;
        registry.register(Box::new(ack_latency.clone()))?;
        registry.register(Box::new(delivery_latency.clone()))?;

        Ok(Arc::new(Self {
            registry,
            connected,
            received,
            duplicates,
            reconnects,
            connect_failures,
            acks_sent,
            never_acked,
            ack_latency,
            delivery_latency,
            ack_percentiles: Latencies::new()?,
            delivery_percentiles: Latencies::new()?,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_metric_is_registered_exactly_once() {
        let metrics = Metrics::new().expect("metric definitions are valid");
        metrics.connected.with_label_values(&["e"]).set(1);
        metrics.received.with_label_values(&["e"]).inc();
        metrics
            .duplicates
            .with_label_values(&["e", "after_ack"])
            .inc();
        metrics
            .reconnects
            .with_label_values(&["e", "client_closed"])
            .inc();
        metrics.connect_failures.with_label_values(&["e"]).inc();
        metrics.acks_sent.with_label_values(&["e"]).inc();
        metrics.never_acked.with_label_values(&["e"]).inc();
        metrics.ack_latency.with_label_values(&["e"]).observe(0.1);
        metrics
            .delivery_latency
            .with_label_values(&["e"])
            .observe(0.1);

        let rendered = sim_common::metrics::render(&metrics.registry);
        for name in [
            "sim_connected_clients",
            "sim_notifications_received_total",
            "sim_duplicate_deliveries_total",
            "sim_reconnects_total",
            "sim_connect_failures_total",
            "sim_acks_sent_total",
            "sim_notifications_never_acked_total",
            "sim_ack_think_time_seconds",
            "sim_delivery_latency_seconds",
        ] {
            assert!(rendered.contains(name), "{name} missing from {rendered}");
        }
    }
}
