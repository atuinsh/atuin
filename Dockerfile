FROM lukemathwalker/cargo-chef:latest-rust-slim-bookworm@sha256:e406ad0baa7266cee09ca9f62f30d7ed330bdb25be9f337ff8090e7ae215f7fd AS chef
WORKDIR app

# Build with the toolchain pinned in rust-toolchain.toml, not the image's.
COPY rust-toolchain.toml .
RUN rustup toolchain install --profile minimal --no-self-update \
      "$(sed -n 's/^ *channel *= *"\([^"]*\)".*/\1/p' rust-toolchain.toml)"

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

# Ensure working C compile setup (not installed by default in arm64 images)
RUN apt update && apt install build-essential libssl-dev pkg-config -y

COPY --from=planner /app/recipe.json recipe.json
# The recipe references the [patch.crates-io] axoasset path dependency, but
# cargo-chef does not skeleton patched crates, so the real sources are needed
COPY vendor vendor
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release --bin atuin-server

FROM debian:bookworm-20260713-slim AS runtime
LABEL org.opencontainers.image.source="https://github.com/atuinsh/atuin" \
  org.opencontainers.image.url="https://atuin.sh" \
  org.opencontainers.image.licenses="MIT"

RUN useradd -c 'atuin user' atuin && mkdir /config && chown atuin:atuin /config
# ca-certificates for webhooks to work, curl for the healthcheck
RUN apt update && apt install --no-install-recommends ca-certificates curl libssl3 -y && rm -rf /var/lib/apt/lists/*
WORKDIR app

USER atuin

ENV TZ=Etc/UTC
ENV RUST_LOG=atuin_server=info
ENV ATUIN_CONFIG_DIR=/config

COPY --from=builder /app/target/release/atuin-server /usr/local/bin

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -fsS -o /dev/null "http://localhost:${ATUIN_PORT:-8888}/healthz"

ENTRYPOINT ["/usr/local/bin/atuin-server"]
