// This module is shared between the `benchmarks` bench target and `tests/bench_harness.rs`.
// Each target uses a different subset of it, so unused items are expected.
#![allow(dead_code)]

pub mod context;
pub mod record;
