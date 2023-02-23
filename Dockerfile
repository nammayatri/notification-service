FROM rust:1.65 as builder

RUN apt-get update \
    && apt install -y protobuf-compiler

WORKDIR /app

# Disable incremental compilation.
#
# Incremental compilation is useful as part of an edit-build-test-edit cycle,
# as it lets the compiler avoid recompiling code that hasn't changed. However,
# on CI, we're not making small edits; we're almost always building the entire
# project from scratch. Thus, incremental compilation on CI actually
# introduces *additional* overhead to support making future builds
# faster...but no future builds will ever occur in any given CI environment.
#
# See https://matklad.github.io/2021/09/04/fast-rust-builds.html#ci-workflow
# for details.
ENV CARGO_INCREMENTAL=0
# Allow more retries for network requests in cargo (downloading crates) and
# rustup (installing toolchains). This should help to reduce flaky CI failures
# from transient network timeouts or other issues.
ENV CARGO_NET_RETRY=10
ENV RUSTUP_MAX_RETRIES=10
# Don't emit giant backtraces in the CI logs.
ENV RUST_BACKTRACE="short"

COPY . .
RUN cargo build --release

FROM debian

RUN apt-get update \
    && apt-get install -y ca-certificates tzdata

COPY --from=builder /app/target/release/css_server /usr/local/bin/css_server
COPY --from=builder /app/config /usr/local/bin/config

EXPOSE 50051

WORKDIR /usr/local/bin

RUN chmod +x ./css_server

CMD ["./css_server"]