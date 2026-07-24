//! Tests for the benchmark harness in `benches/_util`.
//!
//! The `benchmarks` bench target sets `harness = false`, so it cannot host `#[test]` functions.
//! Including the modules by path here gives the harness real test coverage — a benchmark built on
//! a broken harness produces confident, wrong numbers.

#[path = "../benches/_util/mod.rs"]
mod _util;

use std::time::Duration;

use _util::context::BenchCtx;
use _util::record::BenchRecord;
use _util::server::BenchServer;

#[tokio::test]
async fn registers_a_user_and_round_trips_records() {
    let server = BenchServer::start(Duration::ZERO).await;
    let client = server.register().await;

    assert!(client.me().await.is_ok(), "session token should work");

    let mut ctx = BenchCtx::new();
    let records = BenchRecord::chain(&mut ctx, 5);
    let host = records[0].host.id;
    let tag = records[0].tag.clone();

    client.post_records(&records).await.unwrap();

    // `RecordStatus.hosts` maps host -> tag -> max(idx), so a 5-record chain reports 4.
    let status = client.record_status().await.unwrap();
    assert_eq!(status.get(host, tag.clone()), Some(4));

    let downloaded = client.next_records(host, tag, 0, 10).await.unwrap();
    assert_eq!(downloaded.len(), 5);
}

#[tokio::test]
async fn delete_store_resets_server_state() {
    let server = BenchServer::start(Duration::ZERO).await;
    let client = server.register().await;

    let mut ctx = BenchCtx::new();
    let records = BenchRecord::chain(&mut ctx, 5);
    client.post_records(&records).await.unwrap();
    assert!(!client.record_status().await.unwrap().hosts.is_empty());

    // The upload benchmark relies on this to give every sample an empty server.
    client.delete_store().await.unwrap();
    assert!(client.record_status().await.unwrap().hosts.is_empty());
}

#[tokio::test]
async fn latency_layer_delays_every_request() {
    let rtt = Duration::from_millis(100);
    let server = BenchServer::start(rtt).await;
    let client = server.register().await;

    let start = std::time::Instant::now();
    client.me().await.unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed >= rtt,
        "expected at least one injected RTT of {rtt:?}, took {elapsed:?}"
    );
}
