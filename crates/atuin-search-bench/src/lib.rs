//! Shared pieces for the search benchmark (see `benches/search.rs`).
#![cfg_attr(test, allow(clippy::disallowed_methods, reason = "tests may use std::fs for fixtures"))]

pub mod corpus;
