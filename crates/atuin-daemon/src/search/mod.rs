//! Search module for the daemon gRPC search service.
//!
//! This module provides fuzzy search over command history using frizbee.

use std::borrow::Cow;

mod index;
#[allow(clippy::manual_range_contains, reason = "this is a vendored file")]
mod normalize;

// Include the generated proto code
tonic::include_proto!("search");

/// Longest query the fuzzy matcher will see. Frizbee's `u16` scores overflow (and panic) somewhere
/// past ~2700 needle chars; no real query is anywhere near either limit, so longer input is
/// truncated.
const MAX_QUERY_LEN: usize = 512;

/// Truncate a query to the longest length frizbee can score without panicking in
/// [`frizbee::Matcher::from_query`]. Anything that hands a query to frizbee (including
/// client-side highlighting) must apply this.
pub fn truncate_query(query: &str) -> &str {
    // O(1) happy path -- query cannot exceed `MAX_QUERY_LEN` chars if it doesn't even have that
    // many bytes.
    if query.len() <= MAX_QUERY_LEN {
        return query;
    }
    match query.char_indices().nth(MAX_QUERY_LEN) {
        Some((end, _)) => &query[..end],
        None => query,
    }
}

/// Normalize Latin diacritics to their ASCII equivalents (`é` → `e`) so unaccented queries match
/// accented history entries. Maps char to char so char positions are preserved.
pub fn normalize_diacritics(s: &str) -> Cow<'_, str> {
    use normalize::normalize;
    if s.is_ascii() || !s.chars().any(|c| normalize(c) != c) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(s.chars().map(normalize).collect())
}

// Re-export the index and related types
pub use index::{IndexFilterMode, SearchIndex};
