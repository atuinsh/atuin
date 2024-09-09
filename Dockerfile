FROM lukemathwalker/cargo-chef:latest-rust-1.81.0-slim-bookworm AS chef
WORKDIR app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

# Ensure working C compile setup (not installed by default in arm64 images)
RUN apt update && apt install build-essential -y

COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release --bin atuin

FROM debian:bookworm-20240904-slim AS runtime

RUN useradd -c 'atuin user' atuin && mkdir /config && chown atuin:atuin /config
# Install ca-certificates for webhooks to work
RUN apt update && apt install ca-certificates -y && rm -rf /var/lib/apt/lists/*
WORKDIR app

USER atuin

ENV TZ=Etc/UTC
ENV RUST_LOG=atuin::api=info
ENV ATUIN_CONFIG_DIR=/config

COPY --from=builder /app/target/release/atuin /usr/local/bin
ENTRYPOINT ["/usr/local/bin/atuin"]
