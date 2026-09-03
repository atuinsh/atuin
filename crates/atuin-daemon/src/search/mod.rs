//! Search module for the daemon gRPC search service.
//!
//! This module provides fuzzy search over command history using frizbee.

mod index;

// Include the generated proto code
mod proto {
    #![allow(clippy::must_use_candidate, reason = "prost-generated proto code")]

    tonic::include_proto!("search");
}
pub use proto::*;

/// Longest query the fuzzy matcher will see. Frizbee's `u16` scores overflow (and panic) somewhere
/// past ~2700 needle chars; no real query is anywhere near either limit, so longer input is
/// truncated.
const MAX_QUERY_LEN: usize = 512;

/// Truncate a query to the longest length frizbee can score without panicking in
/// [`frizbee::Matcher::from_query`]. Anything that hands a query to frizbee (including
/// client-side highlighting) must apply this.
#[must_use]
pub fn truncate_query(query: &str) -> &str {
    use atuin_common::string::TruncateCharsExt;
    query.truncate_chars(MAX_QUERY_LEN)
}

// Re-export the index and related types
pub use index::{IndexFilterMode, SearchIndex};
