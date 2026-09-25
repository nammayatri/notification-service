/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use anyhow::Result;
use prometheus::{histogram_opts, opts, Gauge, HistogramVec, IntCounterVec, Registry};
use sim_common::metrics::Latencies;
use std::sync::Arc;

const LATENCY_BUCKETS: &[f64] = &[
    0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

pub struct Metrics {
    pub registry: Registry,
    pub search_requests: IntCounterVec,
    pub notifications: IntCounterVec,
    pub publishes: IntCounterVec,
    pub errors: IntCounterVec,
    pub write_latency: HistogramVec,
    pub write_percentiles: Latencies,
}

impl Metrics {
    pub fn new(target_rate: f64) -> Result<Arc<Self>> {
        let registry = Registry::new();

        let search_requests = IntCounterVec::new(
            opts!(
                "sim_producer_search_requests_total",
                "Search requests fanned out by the simulated backend"
            ),
            &["burst"],
        )?;
        let notifications = IntCounterVec::new(
            opts!(
                "sim_producer_notifications_total",
                "XADD + EXPIRE transactions committed"
            ),
            &["locality"],
        )?;
        let publishes = IntCounterVec::new(
            opts!(
                "sim_producer_publishes_total",
                "Messages published to the active-notification channel"
            ),
            &["locality"],
        )?;
        let errors = IntCounterVec::new(
            opts!(
                "sim_producer_errors_total",
                "Failed Redis operations issued by the simulated backend"
            ),
            &["op"],
        )?;
        let write_latency = HistogramVec::new(
            histogram_opts!(
                "sim_producer_write_seconds",
                "Duration of the XADD + EXPIRE transaction",
                LATENCY_BUCKETS.to_vec()
            ),
            &["op"],
        )?;
        let target_rate_gauge = Gauge::new(
            "sim_producer_target_rate",
            "Configured fleet-wide notifications per second",
        )?;
        target_rate_gauge.set(target_rate);

        registry.register(Box::new(search_requests.clone()))?;
        registry.register(Box::new(notifications.clone()))?;
        registry.register(Box::new(publishes.clone()))?;
        registry.register(Box::new(errors.clone()))?;
        registry.register(Box::new(write_latency.clone()))?;
        registry.register(Box::new(target_rate_gauge.clone()))?;

        Ok(Arc::new(Self {
            registry,
            search_requests,
            notifications,
            publishes,
            errors,
            write_latency,
            write_percentiles: Latencies::new()?,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_metric_is_registered_exactly_once() {
        let metrics = Metrics::new(401.0).expect("metric definitions are valid");
        metrics.search_requests.with_label_values(&["burst"]).inc();
        metrics.notifications.with_label_values(&["local"]).inc();
        metrics.publishes.with_label_values(&["remote"]).inc();
        metrics.errors.with_label_values(&["xadd_expire"]).inc();
        metrics
            .write_latency
            .with_label_values(&["xadd_expire"])
            .observe(0.001);

        let rendered = sim_common::metrics::render(&metrics.registry);
        for name in [
            "sim_producer_search_requests_total",
            "sim_producer_notifications_total",
            "sim_producer_publishes_total",
            "sim_producer_errors_total",
            "sim_producer_write_seconds",
        ] {
            assert!(rendered.contains(name), "{name} missing from {rendered}");
        }
        assert!(
            rendered.contains("sim_producer_target_rate 401"),
            "{rendered}"
        );
    }
}
