//! Cloud sync.
//!
//! A background task that periodically synchronizes local history with the Atuin
//! cloud server and feeds freshly-downloaded entries into the search index.

mod engine;

pub use engine::SyncEngine;
