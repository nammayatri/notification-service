/*  Copyright 2022-23, Juspay India Pvt Ltd
    This program is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License
    as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. This program
    is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
    or FITNESS FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more details. You should have received a copy of
    the GNU Affero General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
*/

use crate::{
    config::{AckMode, Config, ReconnectMode},
    distribution::in_never_ack_cohort,
    metrics::Metrics,
    seen::{Seen, SeenWindow},
};
use anyhow::{anyhow, Result};
use futures::StreamExt;
use notification_service::{notification_client::NotificationClient, NotificationAck};
use parking_lot::Mutex;
use rand::Rng;
use sim_common::population;
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{
    metadata::MetadataValue,
    transport::{Channel, Endpoint},
    Request,
};
use tracing::*;

const ACK_CHANNEL_BUFFER: usize = 256;
const SEEN_WINDOW: usize = 512;
const RECONNECT_JITTER_MILLIS: u64 = 500;
const MAX_BACKOFF: Duration = Duration::from_secs(5);
const OPEN_STREAM_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

fn produced_at_millis(entity_data: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(entity_data)
        .ok()?
        .get("produced_at_ms")?
        .as_u64()
}

fn ack_delay(cfg: &Config) -> Duration {
    match cfg.ack_mode {
        AckMode::Delayed => Duration::from_millis(cfg.ack_delay_ms),
        AckMode::Distribution => cfg.ack_delays.sample(),
        AckMode::Instant | AckMode::Never => Duration::ZERO,
    }
}

async fn hold_stream(
    client_id: &str,
    cfg: &Config,
    channel: &Channel,
    endpoint: &str,
    metrics: &Arc<Metrics>,
    seen: &Arc<Mutex<SeenWindow>>,
    never_ack: bool,
) -> Result<&'static str> {
    let (ack_tx, ack_rx) = mpsc::channel::<NotificationAck>(ACK_CHANNEL_BUFFER);
    let mut request = Request::new(ReceiverStream::new(ack_rx));
    request.metadata_mut().insert(
        "token",
        MetadataValue::try_from(client_id).map_err(|err| anyhow!("invalid token header: {err}"))?,
    );
    request.metadata_mut().insert(
        "token-origin",
        MetadataValue::try_from(cfg.token_origin.as_str())
            .map_err(|err| anyhow!("invalid token-origin header: {err}"))?,
    );

    let mut grpc = NotificationClient::new(channel.clone());
    let response = timeout(OPEN_STREAM_TIMEOUT, grpc.stream_payload(request))
        .await
        .map_err(|_| anyhow!("timed out opening StreamPayload"))??;
    let mut inbound = response.into_inner();

    metrics.connected.with_label_values(&[endpoint]).inc();

    let lifetime = match cfg.reconnect_mode {
        ReconnectMode::Distribution => Some(cfg.reconnect_lifetimes.sample()),
        ReconnectMode::ServerOnly => None,
    };
    let lifetime_elapsed = async move {
        match lifetime {
            Some(duration) => sleep(duration).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(lifetime_elapsed);

    let reason = loop {
        let inbound_next = tokio::select! {
            biased;
            _ = &mut lifetime_elapsed => break "client_closed",
            item = inbound.next() => item,
        };
        match inbound_next {
            None => break "server_closed",
            Some(Err(status)) => {
                debug!("[Stream Ended] {} : {}", client_id, status);
                break "stream_error";
            }
            Some(Ok(payload)) => {
                let received_at = Instant::now();
                metrics.received.with_label_values(&[endpoint]).inc();

                let first_sight = match seen.lock().observe(&payload.id) {
                    Seen::First => true,
                    Seen::Repeat { after_ack } => {
                        let phase = if after_ack { "after_ack" } else { "before_ack" };
                        metrics
                            .duplicates
                            .with_label_values(&[endpoint, phase])
                            .inc();
                        false
                    }
                };

                if first_sight {
                    if let Some(produced_at) = payload
                        .entity
                        .as_ref()
                        .and_then(|entity| produced_at_millis(&entity.data))
                    {
                        let elapsed_millis = now_millis().saturating_sub(produced_at);
                        metrics
                            .delivery_latency
                            .with_label_values(&[endpoint])
                            .observe(elapsed_millis as f64 / 1000.0);
                        metrics
                            .delivery_percentiles
                            .record_micros(elapsed_millis * 1000);
                    }
                }

                if never_ack || cfg.ack_mode == AckMode::Never {
                    if first_sight {
                        metrics.never_acked.with_label_values(&[endpoint]).inc();
                    }
                    continue;
                }

                let ack_delay = ack_delay(cfg);
                let ack_tx = ack_tx.clone();
                let seen = seen.clone();
                let metrics = metrics.clone();
                let endpoint = endpoint.to_string();
                tokio::spawn(async move {
                    if !ack_delay.is_zero() {
                        sleep(ack_delay).await;
                    }
                    if ack_tx
                        .send(NotificationAck {
                            id: payload.id.clone(),
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                    seen.lock().mark_acked(&payload.id);
                    let elapsed = received_at.elapsed();
                    metrics.acks_sent.with_label_values(&[&endpoint]).inc();
                    metrics
                        .ack_latency
                        .with_label_values(&[&endpoint])
                        .observe(elapsed.as_secs_f64());
                    metrics
                        .ack_percentiles
                        .record_micros(elapsed.as_micros() as u64);
                });
            }
        }
    };
    metrics.connected.with_label_values(&[endpoint]).dec();

    Ok(reason)
}

pub async fn run_client(
    index: u64,
    cfg: Arc<Config>,
    channel: Channel,
    endpoint: Arc<String>,
    metrics: Arc<Metrics>,
) {
    let client_id = population::client_id(index);
    let never_ack = in_never_ack_cohort(index, cfg.never_ack_pct);
    let seen = Arc::new(Mutex::new(SeenWindow::new(SEEN_WINDOW)));
    let mut backoff = Duration::from_millis(100);

    loop {
        match hold_stream(
            &client_id, &cfg, &channel, &endpoint, &metrics, &seen, never_ack,
        )
        .await
        {
            Ok(reason) => {
                metrics
                    .reconnects
                    .with_label_values(&[&endpoint, reason])
                    .inc();
                backoff = Duration::from_millis(100);
            }
            Err(err) => {
                metrics
                    .connect_failures
                    .with_label_values(&[&endpoint])
                    .inc();
                warn!(
                    "[Notification Service Error] - sim client {} could not hold a stream : {}",
                    client_id, err
                );
                sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }

        let jitter = { rand::thread_rng().gen_range(0..RECONNECT_JITTER_MILLIS) };
        sleep(Duration::from_millis(jitter)).await;
    }
}

pub async fn preflight_endpoints(endpoints: &[String]) -> Result<()> {
    for endpoint in endpoints {
        Endpoint::from_shared(endpoint.to_string())?
            .connect_timeout(CONNECT_TIMEOUT)
            .connect()
            .await
            .map_err(|err| {
                anyhow!(
                    "clients.endpoints entry {endpoint} is unreachable: {err}\n\
                     Use an address that resolves from where this process runs: \
                     http://127.0.0.1:50051 for a local service, real pod IPs when targeting a \
                     deployment from outside the cluster, or an in-cluster service name when \
                     running as a pod. A placeholder such as `pod-a` will not resolve anywhere."
                )
            })?;
    }
    Ok(())
}

pub fn build_channel(endpoint: &str) -> Result<Channel> {
    Ok(Endpoint::from_shared(endpoint.to_string())?
        .tcp_nodelay(true)
        .connect_timeout(CONNECT_TIMEOUT)
        .http2_keep_alive_interval(Duration::from_secs(30))
        .keep_alive_timeout(Duration::from_secs(10))
        .connect_lazy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::load;

    #[test]
    fn produced_at_is_read_back_out_of_the_producers_entity_data() {
        let data = r#"{"searchRequestId":"abc","produced_at_ms":1734512893001}"#;
        assert_eq!(produced_at_millis(data), Some(1734512893001));
    }

    #[test]
    fn entity_data_without_the_stamp_yields_no_latency_sample() {
        assert_eq!(produced_at_millis(r#"{"searchRequestId":"abc"}"#), None);
        assert_eq!(produced_at_millis("not json at all"), None);
        assert_eq!(produced_at_millis(r#"{"produced_at_ms":"nope"}"#), None);
    }

    #[test]
    fn distribution_mode_ignores_the_constant_ack_delay() {
        let mut cfg = load();
        cfg.ack_delay_ms = 9_999;

        cfg.ack_mode = AckMode::Instant;
        assert_eq!(ack_delay(&cfg), Duration::ZERO);

        cfg.ack_mode = AckMode::Delayed;
        assert_eq!(ack_delay(&cfg), Duration::from_millis(9_999));

        cfg.ack_mode = AckMode::Distribution;
        assert!(ack_delay(&cfg) <= Duration::from_millis(cfg.ack_delays.max_ms));
    }

    #[test]
    fn a_bad_endpoint_string_is_rejected_before_the_fleet_spawns() {
        assert!(build_channel("not a url").is_err());
    }
}
