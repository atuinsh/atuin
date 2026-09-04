//! Daemon events.
//!
//! Events are the primary communication mechanism within the daemon.
//! Components emit events to notify others of state changes, and handle
//! events to react to changes elsewhere in the system.

/// Events that flow through the daemon's event bus.
///
/// Events are broadcast to all components. Each component decides which
/// events it cares about in its `handle_event` implementation.
#[derive(Debug, Clone)]
pub enum DaemonEvent {
    // ---- Settings ----
    /// Settings have changed, components should reload if needed.
    SettingsReloaded,

    // ---- Lifecycle ----
    /// Request graceful shutdown of the daemon.
    ShutdownRequested,
}
