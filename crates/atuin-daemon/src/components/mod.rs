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
//! - [`history::HistoryComponent`]: Command history lifecycle management
//! - [`search::SearchComponent`]: Fuzzy search over history
//! - [`sync::SyncComponent`]: Cloud sync

pub mod history;
pub mod search;
pub mod sync;

pub use history::HistoryComponent;
pub use search::SearchComponent;
pub use sync::SyncComponent;
