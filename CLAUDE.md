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
- `dhall-configs/dev/notification_service.dhall` — runtime config (Redis, ports, shards, `retry_delay_millis`, `read_all_connected_client_notifications`, …).
- `web-client/`, `node-client/`, `android-client/` — reference client SDKs.
- `nix/`, `flake.nix` — Nix dev shell and build.
- `justfile` — `just run`, `just fmt`, `just services`, `just fix-warnings`.

## Reader modes (`read_all_connected_client_notifications`)

`crates/notification_service/src/reader.rs::run_notification_reader` branches on this flag:

- **`false` (event-driven)** — spawns `client_reciever_looper`, `retry_notifications_looper`, and `active_notification_looper`. New entries are picked up via Redis Pub/Sub on `pubsub_channel_key()`. `ActiveNotification` is preloaded on connect and tracks per-session unacked stream IDs; the retry sweep only queries clients with `count() > 0`. Resends bump `RETRIED_NOTIFICATIONS`.
- **`true` (poll-all)** — pub/sub loop is **not** spawned. `ActiveNotification` is created empty on connect (skipping the connect-time Redis read). The retry sweep returns every connected `client_id` per shard and reads its stream every `retry_delay_millis`. All deliveries count as `TOTAL_NOTIFICATIONS`. This is the default in dev dhall.

When changing reader behavior, keep both branches consistent — the `if read_all_connected_client_notifications` checks appear in `client_reciever`, `retry_notifications`, and `run_notification_reader`.

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
