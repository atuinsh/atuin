FROM rust:alpine as chef
RUN apk add --no-cache musl-dev file make
RUN cargo install cargo-chef 
WORKDIR app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin atuin

FROM alpine:3.12 as runner
RUN apk add shadow ca-certificates 
RUN groupmod -g 1000 users \
    && useradd -u 1000 -m atuin \
    && usermod -G users atuin \
    && mkdir /config \
    && chown atuin:users /config

USER atuin

ENV TZ=Etc/UTC
ENV RUST_LOG=atuin::api=info
ENV ATUIN_CONFIG_DIR=/config

COPY --from=builder /app/target/release/atuin /usr/local/bin
CMD ["/usr/local/bin/atuin"]

