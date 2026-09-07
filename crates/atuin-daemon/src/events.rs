//! Daemon events.
//!
//! Events are the primary communication mechanism within the daemon.
//! Components emit events to notify others of state changes, and handle
//! events to react to changes elsewhere in the system.

use std::sync::Arc;

use atuin_client::history::HistoryId;

/// Events that flow through the daemon's event bus.
///
/// Events are broadcast to all components. Each component decides which
/// events it cares about in its `handle_event` implementation.
#[derive(Debug, Clone)]
pub enum DaemonEvent {
    // ---- Sync ----
    /// History entries were built from records synced from the server.
    ///
    /// Must carry an `Arc<[HistoryId]>`.
    /// - These messages get sent across a spmc queue, so we avoid unnecessary clones.
    HistorySynced(Arc<[HistoryId]>),

    /// Sync completed successfully.
    SyncCompleted {
        /// Number of records uploaded.
        uploaded: usize,
        /// Number of records downloaded.
        downloaded: usize,
    },

    /// Sync failed.
    SyncFailed {
        /// Error message describing what went wrong.
        error: String,
    },

    // ---- Settings ----
    /// Settings have changed, components should reload if needed.
    SettingsReloaded,

    // ---- Lifecycle ----
    /// Request graceful shutdown of the daemon.
    ShutdownRequested,
}
