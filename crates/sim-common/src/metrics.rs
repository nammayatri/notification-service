/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use anyhow::Result;
use axum::{routing::get, Router};
use hdrhistogram::Histogram;
use parking_lot::Mutex;
use prometheus::{Encoder, Registry, TextEncoder};
use std::{fmt, net::SocketAddr};

const HISTOGRAM_MAX_MICROS: u64 = 300_000_000;
const HISTOGRAM_SIGNIFICANT_DIGITS: u8 = 3;

pub struct Snapshot {
    pub count: u64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "n={} p50={:.1}ms p90={:.1}ms p99={:.1}ms max={:.1}ms",
            self.count, self.p50_ms, self.p90_ms, self.p99_ms, self.max_ms
        )
    }
}

pub struct Latencies {
    window: Mutex<Histogram<u64>>,
    total: Mutex<Histogram<u64>>,
}

impl Latencies {
    pub fn new() -> Result<Self> {
        Ok(Self {
            window: Mutex::new(Histogram::new_with_bounds(
                1,
                HISTOGRAM_MAX_MICROS,
                HISTOGRAM_SIGNIFICANT_DIGITS,
            )?),
            total: Mutex::new(Histogram::new_with_bounds(
                1,
                HISTOGRAM_MAX_MICROS,
                HISTOGRAM_SIGNIFICANT_DIGITS,
            )?),
        })
    }

    pub fn record_micros(&self, micros: u64) {
        let clamped = micros.max(1);
        let _ = self.window.lock().record(clamped);
        let _ = self.total.lock().record(clamped);
    }

    pub fn drain_window(&self) -> Snapshot {
        let mut guard = self.window.lock();
        let snapshot = snapshot_of(&guard);
        guard.reset();
        snapshot
    }

    pub fn total(&self) -> Snapshot {
        snapshot_of(&self.total.lock())
    }
}

fn snapshot_of(histogram: &Histogram<u64>) -> Snapshot {
    Snapshot {
        count: histogram.len(),
        p50_ms: histogram.value_at_quantile(0.50) as f64 / 1000.0,
        p90_ms: histogram.value_at_quantile(0.90) as f64 / 1000.0,
        p99_ms: histogram.value_at_quantile(0.99) as f64 / 1000.0,
        max_ms: histogram.max() as f64 / 1000.0,
    }
}

pub async fn serve(registry: Registry, port: u16) -> Result<()> {
    let app = Router::new().route(
        "/metrics",
        get(move || {
            let registry = registry.clone();
            async move { render(&registry) }
        }),
    );
    axum::Server::bind(&SocketAddr::from(([0, 0, 0, 0], port)))
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

pub fn render(registry: &Registry) -> String {
    let mut buffer = Vec::new();
    let encoder = TextEncoder::new();
    match encoder.encode(&registry.gather(), &mut buffer) {
        Ok(()) => String::from_utf8(buffer).unwrap_or_default(),
        Err(err) => {
            tracing::error!(
                "[Notification Service Error] - metrics encode failed : {}",
                err
            );
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prometheus::{opts, IntCounterVec};

    #[test]
    fn percentiles_are_reported_in_milliseconds() {
        let latencies = Latencies::new().expect("histogram bounds are valid");
        for millis in 1..=1000 {
            latencies.record_micros(millis * 1000);
        }

        let snapshot = latencies.total();
        assert_eq!(snapshot.count, 1000);
        assert!((snapshot.p50_ms - 500.0).abs() < 1.0, "{snapshot}");
        assert!((snapshot.max_ms - 1000.0).abs() < 1.0, "{snapshot}");
    }

    #[test]
    fn draining_the_window_leaves_the_running_total_intact() {
        let latencies = Latencies::new().expect("histogram bounds are valid");
        latencies.record_micros(5_000);
        latencies.record_micros(15_000);

        assert_eq!(latencies.drain_window().count, 2);
        assert_eq!(latencies.drain_window().count, 0);
        assert_eq!(latencies.total().count, 2);
    }

    #[test]
    fn sub_microsecond_samples_are_recorded_rather_than_dropped() {
        let latencies = Latencies::new().expect("histogram bounds are valid");
        latencies.record_micros(0);
        assert_eq!(latencies.total().count, 1);
    }

    #[test]
    fn render_emits_the_registered_series() {
        let registry = Registry::new();
        let counter = IntCounterVec::new(opts!("sim_test_total", "test"), &["label"])
            .expect("valid metric definition");
        registry
            .register(Box::new(counter.clone()))
            .expect("first registration");
        counter.with_label_values(&["a"]).inc();

        let rendered = render(&registry);
        assert!(
            rendered.contains("sim_test_total{label=\"a\"} 1"),
            "{rendered}"
        );
    }
}
