//! Search module for the daemon gRPC search service.
//!
//! This module provides fuzzy search over command history using frizbee.

mod index;
#[allow(clippy::manual_range_contains, reason = "this is a vendored file")]
mod normalize;

// Include the generated proto code
tonic::include_proto!("search");

// Re-export the index and related types
pub use index::{IndexFilterMode, SearchIndex, normalize_diacritics, truncate_query};
