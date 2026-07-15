//! Daemon events.
//!
//! Events are the primary communication mechanism within the daemon.
//! Components emit events to notify others of state changes, and handle
//! events to react to changes elsewhere in the system.
//!
//! External processes (like CLI commands) can also inject events via the
//! Control gRPC service.

use atuin_client::history::{History, HistoryId};

/// Events that flow through the daemon's event bus.
///
/// Events are broadcast to all components. Each component decides which
/// events it cares about in its `handle_event` implementation.
#[derive(Debug, Clone)]
pub enum DaemonEvent {
    // ---- History lifecycle ----
    /// A command has started running.
    HistoryStarted(History),

    /// A command has finished running.
    HistoryEnded(History),

    // ---- Sync ----
    /// History entries were built from records synced from the server.
    ///
    /// The search component uses this to update its index with the new history.
    HistorySynced(Vec<History>),

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

    /// Request an immediate sync (external trigger).
    ForceSync,

    // ---- External commands ----
    /// History was pruned - search index needs a full rebuild.
    ///
    /// Emitted when the user runs `atuin history prune` or similar.
    HistoryPruned,

    /// History was rebuilt - search index needs a full rebuild.
    ///
    /// Emitted when the user runs `atuin store rebuild history` or similar.
    HistoryRebuilt,

    /// Specific history items were deleted.
    ///
    /// The search component should remove these from its index.
    HistoryDeleted {
        /// IDs of the deleted history entries.
        ids: Vec<HistoryId>,
    },

    /// Settings have changed, components should reload if needed.
    SettingsReloaded,

    // ---- Lifecycle ----
    /// Request graceful shutdown of the daemon.
    ShutdownRequested,
}
