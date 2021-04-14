FROM rust as builder

RUN rustup default nightly


RUN cargo new --bin atuin
WORKDIR /atuin
COPY ./Cargo.toml ./Cargo.toml
COPY ./Cargo.lock ./Cargo.lock

RUN cargo build --release

RUN rm src/*.rs

ADD . ./

RUN rm ./target/release/deps/atuin*
RUN cargo build --release

FROM debian:buster-slim

RUN apt-get update \
    && apt-get install -y ca-certificates tzdata libpq-dev \
    && rm -rf /var/lib/apt/lists/*

EXPOSE 8888

ENV TZ=Etc/UTC
ENV RUST_LOG=info
ENV ATUIN_CONFIG=/config/config.toml

COPY --from=builder /atuin/target/release/atuin ./atuin

ENTRYPOINT ["./atuin"]
