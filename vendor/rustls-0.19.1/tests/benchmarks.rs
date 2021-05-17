use criterion::criterion_group;
use criterion::criterion_main;
/// Microbenchmarks go here.  Larger benchmarks of (eg.) protocol
/// performance go in examples/internal/bench.rs.
use criterion::Criterion;

#[allow(dead_code)]
mod common;
use crate::common::*;

use rustls::{ServerSession, Session};

use std::io;
use std::sync::Arc;

fn bench_ewouldblock(c: &mut Criterion) {
    let server_config = make_server_config(KeyType::RSA);
    let mut server = ServerSession::new(&Arc::new(server_config));
    let mut read_ewouldblock = FailsReads::new(io::ErrorKind::WouldBlock);
    c.bench_function("read_tls with EWOULDBLOCK", move |b| {
        b.iter(|| server.read_tls(&mut read_ewouldblock))
    });
}

criterion_group!(benches, bench_ewouldblock);
criterion_main!(benches);
