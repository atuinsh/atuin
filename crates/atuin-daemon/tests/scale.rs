//! The daemon at the sizes real histories reach: the tiers fast enough for per-push CI. The
//! 100k-write and 1M-row tiers of the same bodies live in `tests/nightly.rs`.
#![cfg(unix)]

mod common;

use std::time::{Duration, Instant};

use common::TestEnv;
use common::scale::{
    deleting_a_sample_leaves_everything_else_body, deleting_everything_in_one_request_body,
    index_matches_db_after_seeding_body, rebuild_from_a_large_store_body, report,
};
use rstest::*;

#[rstest]
#[case::one_k(1_000)]
#[case::ten_k(10_000)]
#[case::hundred_k(100_000)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn index_matches_db_after_seeding(#[case] rows: usize) {
    index_matches_db_after_seeding_body(rows).await;
}

#[rstest]
#[case::ten_k(10_000)]
#[case::hundred_k(100_000)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_a_sample_leaves_everything_else(#[case] rows: usize) {
    deleting_a_sample_leaves_everything_else_body(rows).await;
}

#[rstest]
#[case::one_k(1_000)]
#[case::ten_k(10_000)]
#[case::two_hundred_k(200_000)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_everything_in_one_request(#[case] rows: usize) {
    deleting_everything_in_one_request_body(rows).await;
}

#[rstest]
#[case::one_k(1_000)]
#[case::ten_k(10_000)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rebuild_from_a_large_store(#[case] rows: usize) {
    rebuild_from_a_large_store_body(rows).await;
}

/// Sequential shell hooks stay fast on a large index: 500 start/end pairs, mean well under a
/// human-noticeable prompt delay even in a debug build.
#[rstest]
#[case::hundred_k(100_000)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shell_hooks_stay_fast_on_a_large_index(#[case] rows: usize) {
    let env = TestEnv::builder().seed_rows(rows).build().await;
    let mut client = env.history_client().await;
    let started = Instant::now();
    for i in 0..500 {
        env.record(&mut client, &format!("hook {i}")).await;
    }
    let mean = started.elapsed() / 500;
    report("500 start/end pairs", rows, started);
    assert!(mean < Duration::from_millis(50), "mean hook round-trip {mean:?}");
    assert_eq!(env.active_rows().await, i64::try_from(rows + 500).unwrap());
}
