default:
    @just --list

# Auto-format project tree
fmt:
    treefmt

# Run the project
run:
    cargo run --bin notification-service

# Watch for changes, recompile and run
watch:
    cargo watch -x 'run --bin notification-service'

# Run both simulators against a running service; Ctrl+C stops both
sim:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "${SIM_PROFILE:-debug}" = "release" ]; then
        cargo build --release --bin sim-clients --bin sim-producer
        dir=target/release
    else
        cargo build --bin sim-clients --bin sim-producer
        dir=target/debug
    fi
    clients=""
    producer=""
    trap 'kill ${clients} ${producer} 2>/dev/null || true' EXIT INT TERM
    ./${dir}/sim-clients &
    clients=$!
    sleep "${SIM_WARMUP_SECONDS:-10}"
    ./${dir}/sim-producer &
    producer=$!
    while kill -0 ${clients} 2>/dev/null && kill -0 ${producer} 2>/dev/null; do sleep 1; done

# Hold gRPC streams as a simulated driver fleet
sim-clients:
    cargo run --bin sim-clients

# Write notifications as a simulated ride backend
sim-producer:
    cargo run --bin sim-producer

# Run the simulator tests and lints, the same set CI gates on
sim-check:
    cargo test -p sim-common -p sim-clients -p sim-producer
    cargo clippy -p sim-common -p sim-clients -p sim-producer --all-targets -- -D warnings

# Run the project service dependencies
services:
    notification-services

# Fix and lint the project
fix-warnings:
    cargo fix --allow-dirty --allow-staged
    cargo clippy --all-targets --all-features -- -D warnings
