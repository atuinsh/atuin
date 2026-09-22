//! Daemon components.
//!
//! Components are the building blocks of the daemon. Each component handles
//! a specific domain and can:
//!
//! - Expose gRPC services
//! - React to events
//! - Spawn background tasks
//!
//! Available components:
//!
//! - [`search::SearchComponent`]: Fuzzy search over history

pub mod search;

pub use search::SearchComponent;
