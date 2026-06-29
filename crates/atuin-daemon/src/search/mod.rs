//! Search module for the daemon gRPC search service.
//!
//! This module provides fuzzy search over command history using Nucleo.

mod index;

// Include the generated proto code
tonic::include_proto!("search");

// Re-export the service and index
pub use index::{IndexFilterMode, QueryContext, SearchIndex};
