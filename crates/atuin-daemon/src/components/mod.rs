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
//! - [`semantic::SemanticComponent`]: In-memory semantic command captures
//! - [`sync::SyncComponent`]: Cloud sync

pub mod search;
pub mod semantic;
pub mod sync;

pub use search::SearchComponent;
pub use semantic::SemanticComponent;
pub use sync::SyncComponent;
