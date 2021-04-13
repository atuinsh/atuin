# no point in tagging the rust version, currently using nightly
FROM rust:slim-buster

RUN apt update && apt -y install libssl-dev libpq-dev pkg-config make
RUN rustup default nightly

WORKDIR /atuin
COPY . /atuin

RUN cargo build --release

ENTRYPOINT ["/atuin/target/release/atuin"]
