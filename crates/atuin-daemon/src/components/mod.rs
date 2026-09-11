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
//! - [`sync::SyncComponent`]: Cloud sync

pub mod search;
pub mod sync;

pub use search::SearchComponent;
pub use sync::SyncComponent;
