# CLAUDE.md

Guidance for Claude Code working in this repository.

## What this is

A gRPC push-notification service written in Rust. Clients (driver/rider apps, dashboards) maintain a persistent bi-directional gRPC stream to the server; producers (BAP/BPP) publish notifications to per-client Redis Streams; the server reads from Redis and pushes onto live client streams, with ACKs flowing back over the same stream. See `Readme.md` for the full design rationale and the architecture comparison vs FCM/MQTT/WebSockets.

## Workspace layout

- `crates/notification_service/` — main service binary.
  - `src/main.rs`, `src/server.rs` — entrypoint, tracing, gRPC + HTTP server bootstrap.
  - `src/environment.rs` — config wiring from dhall.
  - `src/action/notification.rs` — gRPC service impl (`StreamPayload`, ID-based RPCs).
  - `src/action/healthcheck.rs` — health endpoint.
  - `src/reader.rs` — notification reader: receives client connect/disconnect/ack events, pushes notifications to streams, runs the retry loop. Two execution modes; see "Reader modes" below.
  - `src/redis/` — Redis stream/pubsub commands, key builders, types (`NotificationData`, `ActiveNotification`).
  - `src/outbound/` — HTTP calls out (e.g. internal auth).
  - `src/middleware/` — request/response tracking middleware.
  - `src/common/types.rs` — `ClientId`, `Shard`, `SessionMap`, `ReaderMap`, `SenderType`, `MonitoredRwLock` wrappers.
  - `src/tools/prometheus.rs` — metrics (`TOTAL_NOTIFICATIONS`, `RETRIED_NOTIFICATIONS`, `EXPIRED_NOTIFICATIONS`, `CONNECTED_CLIENTS`, `NOTIFICATION_LATENCY`, `CHANNEL_DELAY`, …).
  - `protos/notification_service.proto`, `protos/healthcheck.proto`.
- `crates/tests/` — integration / load tests.
- `dhall-configs/dev/notification_service.dhall` — runtime config (Redis, ports, shards, `delivery_mode`, `delivery_guarantee`, `max_delivery_attempts`, `sweep_delay_millis`, `retry_delay_millis`, …).
- `web-client/`, `node-client/`, `android-client/` — reference client SDKs.
- `nix/`, `flake.nix` — Nix dev shell and build.
- `justfile` — `just run`, `just fmt`, `just services`, `just fix-warnings`.

## Reader modes (`delivery_mode`)

`crates/notification_service/src/reader.rs::run_notification_reader` spawns loops according to `DeliveryMode` (`common/types.rs`). `sweep_looper` (`full_sweep` = `backfill_new_entries` + `retry_pending_in_memory`, every `sweep_delay_millis`) and `expire_notifications_looper` run in both modes.

- **`Pubsub` (event-driven)** — additionally spawns `active_notification_looper` (Redis Pub/Sub on `pubsub_channel_key()`, one global channel so every pod sees every publish — `PUBSUB_MESSAGES{outcome}` tracks local/foreign/no_session) and `retry_looper` (`retry_pending_in_memory` every `retry_delay_millis`, no Redis reads). On connect, `client_reciever` spawns a `catchup` stream read. Set `sweep_delay_millis` long in this mode; the sweep is only a safety net. This is the default in dev dhall.
- **`Sweep` (poll-all)** — only the shared loops. `ActiveNotification` starts empty on connect; the next sweep reads every connected client's stream from its `last_read_id` cursor. `retry_delay_millis` is unused.

When changing reader behavior, keep both modes consistent — the mode-dependent sites are `DeliveryMode::needs_connect_catchup` (in `client_reciever`) and `needs_independent_retry_loop` / the `Pubsub` check in `run_notification_reader`.

## Delivery guarantee (`delivery_guarantee`)

This setting is separate from `delivery_mode`. `DeliveryGuarantee = AtMostOnce | AtLeastOnce` and the `max_delivery_attempts: Optional Natural` cap are combined into a `DeliveryPolicy` (`common/types.rs`) with a `push_cap`. `AtMostOnce` always sets the cap to 1; `AtLeastOnce` uses `max_delivery_attempts`, where `None` means unlimited. The policy is passed to every reader loop.

- **`AtMostOnce`** (dev default, matches the old prod behaviour): after a push reaches at least one target, `settle_push_round` moves the entry to the primary session's `awaiting_ack` set, drops it from the other sessions, and queues an `XDEL` through the cleanup queue (`CleanupReason::Delivered`, which is not counted as expired). `claim_read_batch` only passes on entries past `last_read_id`, so a racing stale read cannot re-insert an entry that was already pushed.
- **Acks in both modes**: `ActiveNotification::acknowledge` matches `pending` first (`AtLeastOnce`, which returns a `stream_id_to_delete` for the ack path to `XDEL`), then `awaiting_ack` (`AtMostOnce`, where the delete was already queued). Either match counts `delivered_notifications{category}` and ACK latency. `awaiting_ack` entries that are never acked are counted in `unacked_notifications_total{category, reason}`: `ttl` from `retry_pending_in_memory`, or `stream_closed` via `count_unacked_on_close` when the session is removed, replaced or evicted. Only acks that match neither set go to `unmatched_acks_total`.
- **`AtLeastOnce`**: entries stay until an ACK arrives or the TTL expires; `retry_pending_in_memory` re-pushes them until `push_cap` is reached.

The guarantee-dependent sites are `claim_read_batch` (called from `ingest_backfill` and `active_notification_dispatch`), `try_claim_push` / `settle_push_round` in `dispatch_and_send_notifications` and `retry_pending_in_memory`, and `DeliveryPolicy::new`. Any new push path must claim through `try_claim_push` and finish with `settle_push_round`.

## Single connection per client (`single_connection_eviction`)

A phone whose network drops leaves a ghost stream on its old pod, because the server's TCP peer is the load balancer rather than the phone. The phone then reconnects to another pod. When the flag is on, each pod gets a random `InstanceId` at boot:

- On a `Single` connect, `client_reciever` publishes a `ClientConnectMessage {clientId, instanceId, connectedAt}` on the global `client_connect_channel_key()` channel after the map insert.
- `client_connect_looper` runs in both delivery modes. `evict_superseded_connection` removes a local `Single` entry only if it connected earlier than the claim. It sends `ALREADY_EXISTS` down the old stream and drops the sender so the stream ends.
- `Multi` sessions and the pod's own claims are left alone. The `connectedAt` comparison stops a late claim from evicting a newer local stream.
- Metrics: `client_connect_messages_total{outcome=self|evicted|kept|not_held}` and `client_slot_events_total{event="evicted_by_peer"}`.

## Sharding & locking

Clients are sharded by `hash_uuid(client_id) % max_shards`. Each shard is a `MonitoredRwLock<FxHashMap<ClientId, SessionMap>>`. `SessionMap` is `Single((ClientTx, Arc<MonitoredRwLock<ActiveNotification>>))` or `Multi(FxHashMap<SessionID, …>)`. `MonitoredRwLock` wraps `tokio::sync::RwLock` with `RwLockName` / `RwLockOperation` instrumentation — preserve those labels when adding new lock sites; metrics depend on them.

## Working in this repo

- Dev shell: `nix develop` (provides `cargo`, `grpcurl`, `treefmt`, pre-commit hooks). Service deps via `just services`.
- Build/run: `cargo run` (or `nix run`). For tokio-console profiling see `Readme.md` "Profiling".
- Format: `just fmt` (treefmt) — CI enforces.
- Lint: `cargo clippy --all-targets --all-features -- -D warnings` (see `just fix-warnings`).
- Smoke test gRPC: see the `grpcurl` invocation under `Readme.md` "Debugging".

## Conventions

- AGPL-3.0 header at the top of every Rust source file — preserve it on edits and new files.
- Don't add comments unless the *why* is non-obvious (hidden constraint, subtle invariant, or workaround). Identifier names should carry the *what*.
- Use the existing `#[macros::measure_duration]` and `measure_latency_duration!` macros for new hot paths so they show up in Prometheus.
- Errors are logged with the `[Notification Service Error] - …` prefix; stay consistent so log greps keep working.
- Config changes go in `dhall-configs/dev/notification_service.dhall` and are read via `environment.rs`; do not hard-code values in the Rust source.
