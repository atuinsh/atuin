FROM rust:1.40

WORKDIR /usr/num_cpus

COPY . .

RUN cargo build

CMD [ "cargo", "test", "--lib" ]
