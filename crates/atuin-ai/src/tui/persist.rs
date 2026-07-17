//! Persistence as an actor: one task owns the `SessionManager` and applies
//! jobs in channel order.
//!
//! `SessionManager`'s methods take `&mut self`, and writes must not be
//! reordered (each job snapshots the full event list — an older snapshot
//! landing after a newer one would roll the session back). A single owning
//! task gives both properties without locks; the old driver got them by
//! blocking its thread.

use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::session::SessionManager;
use crate::tui::state::ConversationEvent;
use crate::usage::UsageSnapshot;

#[derive(Debug)]
pub(crate) enum PersistJob {
    /// Snapshot of the conversation and its trackers.
    Session {
        events: Vec<ConversationEvent>,
        server_session_id: Option<String>,
        file_tracker: Option<String>,
        edit_permissions: Option<String>,
    },
    /// Cache a fresh credit-usage snapshot for the next TUI open.
    Usage {
        key: String,
        snapshot: UsageSnapshot,
    },
    /// `/new`: archive the current session and start fresh.
    Archive,
}

/// Spawn the worker on the ambient tokio runtime. Dropping the returned
/// sender (with the app) ends the worker after it drains pending jobs.
pub(crate) fn spawn_persist_worker(mut session_mgr: SessionManager) -> UnboundedSender<PersistJob> {
    let (tx, mut rx) = unbounded_channel();
    tokio::spawn(async move {
        while let Some(job) = rx.recv().await {
            match job {
                PersistJob::Session {
                    events,
                    server_session_id,
                    file_tracker,
                    edit_permissions,
                } => {
                    if let Err(e) = session_mgr.persist_events(&events).await {
                        tracing::warn!("failed to persist session events: {e}");
                    }
                    if let Some(sid) = server_session_id
                        && let Err(e) = session_mgr.persist_server_session_id(&sid).await
                    {
                        tracing::warn!("failed to persist server session ID: {e}");
                    }
                    if let Some(json) = file_tracker
                        && let Err(e) = session_mgr
                            .set_metadata(crate::file_tracker::METADATA_KEY, &json)
                            .await
                    {
                        tracing::warn!("failed to persist file tracker: {e}");
                    }
                    if let Some(json) = edit_permissions
                        && let Err(e) = session_mgr
                            .set_metadata(crate::edit_permissions::METADATA_KEY, &json)
                            .await
                    {
                        tracing::warn!("failed to persist edit permissions: {e}");
                    }
                }
                PersistJob::Usage { key, snapshot } => {
                    if let Err(e) = session_mgr.set_cached_usage(&key, &snapshot).await {
                        tracing::warn!("failed to persist usage cache: {e}");
                    }
                }
                PersistJob::Archive => {
                    if let Err(e) = session_mgr.archive_and_reset().await {
                        tracing::warn!("failed to archive session: {e}");
                    }
                }
            }
        }
    });
    tx
}
