/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use anyhow::{anyhow, Result};
use rand::Rng;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub struct AckDelayCurve {
    pub p50_ms: u64,
    pub p90_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
}

impl AckDelayCurve {
    pub fn new(p50_ms: u64, p90_ms: u64, p95_ms: u64, max_ms: u64) -> Result<Self> {
        if !(p50_ms <= p90_ms && p90_ms <= p95_ms && p95_ms <= max_ms) {
            return Err(anyhow!(
                "ack percentiles must satisfy p50 <= p90 <= p95 <= max, got {p50_ms}/{p90_ms}/{p95_ms}/{max_ms}"
            ));
        }
        Ok(Self {
            p50_ms,
            p90_ms,
            p95_ms,
            max_ms,
        })
    }

    pub fn millis_at(&self, quantile: f64) -> u64 {
        let points = [
            (0.0, 0.0),
            (0.5, self.p50_ms as f64),
            (0.9, self.p90_ms as f64),
            (0.95, self.p95_ms as f64),
            (1.0, self.max_ms as f64),
        ];

        let quantile = quantile.clamp(0.0, 1.0);
        for pair in points.windows(2) {
            let (q_lo, v_lo) = pair[0];
            let (q_hi, v_hi) = pair[1];
            if quantile <= q_hi {
                return (v_lo + interpolate(quantile, q_lo, q_hi) * (v_hi - v_lo)).round() as u64;
            }
        }
        self.max_ms
    }

    pub fn sample(&self) -> Duration {
        Duration::from_millis(self.millis_at(rand::thread_rng().gen_range(0.0..1.0)))
    }
}

#[derive(Clone, Debug)]
pub struct LifetimeCurve {
    points: Vec<(f64, f64)>,
}

impl LifetimeCurve {
    pub fn from_buckets(buckets: &[(f64, f64)]) -> Result<Self> {
        if buckets.is_empty() {
            return Err(anyhow!("reconnect_lifetime_buckets is empty"));
        }

        let mut previous = (0.0f64, 0.0f64);
        for (bound, weight) in buckets {
            if *bound <= previous.0 || *weight < previous.1 {
                return Err(anyhow!(
                    "reconnect_lifetime_buckets needs ascending upto_seconds and non-decreasing cumulative_count, got {bound}:{weight} after {}:{}",
                    previous.0,
                    previous.1
                ));
            }
            previous = (*bound, *weight);
        }

        let total = previous.1;
        if total <= 0.0 {
            return Err(anyhow!(
                "reconnect_lifetime_buckets final cumulative_count must be positive, got {total}"
            ));
        }

        Ok(Self {
            points: buckets
                .iter()
                .map(|(bound, weight)| (*bound, weight / total))
                .collect(),
        })
    }

    pub fn at(&self, quantile: f64) -> Duration {
        let quantile = quantile.clamp(0.0, 1.0);
        let mut lower = (0.0f64, 0.0f64);
        for (bound, fraction) in &self.points {
            if quantile <= *fraction {
                let seconds =
                    lower.0 + interpolate(quantile, lower.1, *fraction) * (bound - lower.0);
                return Duration::from_secs_f64(seconds);
            }
            lower = (*bound, *fraction);
        }
        Duration::from_secs_f64(lower.0)
    }

    pub fn sample(&self) -> Duration {
        self.at(rand::thread_rng().gen_range(0.0..1.0))
    }

    pub fn bucket_count(&self) -> usize {
        self.points.len()
    }
}

fn interpolate(value: f64, low: f64, high: f64) -> f64 {
    let span = high - low;
    if span > 0.0 {
        (value - low) / span
    } else {
        0.0
    }
}

pub fn in_never_ack_cohort(index: u64, never_ack_pct: u64) -> bool {
    never_ack_pct > 0 && index.wrapping_mul(never_ack_pct) % 100 < never_ack_pct
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prod_ack_curve() -> AckDelayCurve {
        AckDelayCurve::new(800, 2071, 2326, 5000).expect("prod percentiles are ordered")
    }

    fn prod_lifetime_curve() -> LifetimeCurve {
        crate::config::tests::load().reconnect_lifetimes
    }

    #[test]
    fn ack_distribution_reproduces_the_percentiles_it_was_given() {
        let curve = prod_ack_curve();
        assert_eq!(curve.millis_at(0.5), 800);
        assert_eq!(curve.millis_at(0.9), 2071);
        assert_eq!(curve.millis_at(0.95), 2326);
        assert_eq!(curve.millis_at(0.0), 0);
        assert_eq!(curve.millis_at(1.0), 5000);
    }

    #[test]
    fn ack_distribution_is_monotonic_and_bounded() {
        let curve = prod_ack_curve();
        let mut previous = 0;
        for step in 0..=1000 {
            let sample = curve.millis_at(step as f64 / 1000.0);
            assert!(sample >= previous, "not monotonic at {step}");
            assert!(sample <= curve.max_ms);
            previous = sample;
        }
    }

    #[test]
    fn ack_distribution_puts_half_the_mass_above_p50() {
        let curve = prod_ack_curve();
        let over = (0..10_000)
            .filter(|step| curve.millis_at(*step as f64 / 10_000.0) > curve.p50_ms)
            .count();
        assert!((4_900..=5_100).contains(&over), "got {over}");
    }

    #[test]
    fn ack_curve_rejects_percentiles_out_of_order() {
        assert!(AckDelayCurve::new(2030, 756, 2290, 5000).is_err());
        assert!(AckDelayCurve::new(756, 2030, 2290, 100).is_err());
    }

    fn lifetime_share_under(seconds: f64, samples: u32) -> f64 {
        let curve = prod_lifetime_curve();
        let under = (0..samples)
            .filter(|step| curve.at(*step as f64 / samples as f64).as_secs_f64() <= seconds)
            .count();
        under as f64 / samples as f64
    }

    #[test]
    fn stream_lifetime_reproduces_the_prod_bucket_shares() {
        assert!((lifetime_share_under(0.005, 100_000) - 3.964 / 8.386).abs() < 0.002);
        assert!((lifetime_share_under(1.0, 100_000) - 6.544 / 8.386).abs() < 0.002);
        assert!((lifetime_share_under(10.0, 100_000) - 6.856 / 8.386).abs() < 0.002);
        assert!((lifetime_share_under(300.0, 100_000) - 7.831 / 8.386).abs() < 0.002);
    }

    #[test]
    fn stream_lifetime_is_monotonic_and_bounded() {
        let curve = prod_lifetime_curve();
        let mut previous = Duration::ZERO;
        for step in 0..=1000 {
            let sample = curve.at(step as f64 / 1000.0);
            assert!(sample >= previous, "not monotonic at {step}");
            assert!(sample.as_secs_f64() <= 400.0);
            previous = sample;
        }
    }

    #[test]
    fn lifetime_curve_normalizes_raw_grafana_counts() {
        let curve = LifetimeCurve::from_buckets(&[(0.005, 5.22), (400.0, 10.9)]).expect("valid");
        assert!((curve.at(5.22 / 10.9).as_secs_f64() - 0.005).abs() < 1e-6);
        assert!((curve.at(1.0).as_secs_f64() - 400.0).abs() < 1e-6);
    }

    #[test]
    fn lifetime_curve_rejects_malformed_buckets() {
        assert!(LifetimeCurve::from_buckets(&[]).is_err());
        assert!(LifetimeCurve::from_buckets(&[(1.0, 0.5), (0.5, 1.0)]).is_err());
        assert!(LifetimeCurve::from_buckets(&[(0.005, 0.9), (1.0, 0.4)]).is_err());
        assert!(LifetimeCurve::from_buckets(&[(0.005, 0.0), (1.0, 0.0)]).is_err());
    }

    fn cohort_size(population: u64, never_ack_pct: u64) -> u64 {
        (0..population)
            .filter(|index| in_never_ack_cohort(*index, never_ack_pct))
            .count() as u64
    }

    #[test]
    fn never_ack_cohort_holds_its_ratio_on_small_populations() {
        assert_eq!(cohort_size(20, 10), 2);
        assert_eq!(cohort_size(20, 25), 5);
        assert_eq!(cohort_size(20, 50), 10);
        assert_eq!(cohort_size(20, 0), 0);
        assert_eq!(cohort_size(20, 100), 20);
    }

    #[test]
    fn never_ack_cohort_holds_its_ratio_at_pod_scale() {
        assert_eq!(cohort_size(18_600, 10), 1_860);
        assert_eq!(cohort_size(18_600, 4), 744);
    }

    #[test]
    fn never_ack_cohort_is_stable_under_replica_splits() {
        let whole = (0..1000)
            .filter(|index| in_never_ack_cohort(*index, 10))
            .collect::<Vec<_>>();
        let split = (0..500)
            .chain(500..1000)
            .filter(|index| in_never_ack_cohort(*index, 10))
            .collect::<Vec<_>>();
        assert_eq!(whole, split);
    }
}
