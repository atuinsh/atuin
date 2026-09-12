//! The nightly category: the 100k-write and 1M-row tiers of the `tests/scale.rs` bodies. They
//! pass, but take minutes each in a debug build, so they are `#[ignore]`d and this binary is what
//! `.github/workflows/nightly.yml` runs:
//!
//!     cargo nextest run -p atuin-daemon --test nightly --run-ignored all
//!
//! Only tests that are expected to pass belong here. Tests that document an unfixed defect stay
//! `#[ignore]`d next to their siblings (`scale.rs`, `concurrency.rs`) so they never turn this job
//! red; move a tier here once its defect is fixed.
#![cfg(unix)]

mod common;

use common::scale::{
    deleting_a_sample_leaves_everything_else_body, deleting_everything_in_one_request_body,
    index_matches_db_after_seeding_body, rebuild_from_a_large_store_body,
};

#[ignore = "nightly tier; see the module docs for how to run it"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn index_matches_db_after_seeding_one_m() {
    index_matches_db_after_seeding_body(1_000_000).await;
}

#[ignore = "nightly tier; see the module docs for how to run it"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_a_sample_leaves_everything_else_one_m() {
    deleting_a_sample_leaves_everything_else_body(1_000_000).await;
}

#[ignore = "nightly tier; see the module docs for how to run it"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_everything_in_one_request_hundred_k() {
    deleting_everything_in_one_request_body(100_000).await;
}

#[ignore = "nightly tier; see the module docs for how to run it"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_everything_in_one_request_one_m() {
    deleting_everything_in_one_request_body(1_000_000).await;
}

#[ignore = "nightly tier; see the module docs for how to run it"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rebuild_from_a_large_store_hundred_k() {
    rebuild_from_a_large_store_body(100_000).await;
}

#[ignore = "nightly tier; see the module docs for how to run it"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rebuild_from_a_large_store_one_m() {
    rebuild_from_a_large_store_body(1_000_000).await;
}
