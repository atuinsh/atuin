//! Writing a harness-native transcript back out from captured messages, so a session recorded on
//! another machine (or whose transcript was deleted) can be resumed here.

use std::path::PathBuf;

use time::OffsetDateTime;

use crate::harnesstools::session::{Content, Role, StopReason, Usage};

/// One captured message, harness-agnostic, in transcript order.
#[derive(Clone, Debug)]
pub struct RehydrateMessage {
    /// The harness's own id for the line/part this row came from (capture's `source_id`). Written
    /// back verbatim so re-capturing the rehydrated transcript dedups against the synced rows.
    pub source_id: String,
    pub parent_source_id: Option<String>,
    pub timestamp: OffsetDateTime,
    pub role: Role,
    pub content: Vec<Content>,
    pub model: Option<String>,
    pub usage: Option<Usage>,
    pub stop_reason: Option<StopReason>,
    pub turn_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub git_branch: Option<String>,
}

/// A session to write back out.
#[derive(Clone, Debug)]
pub struct RehydrateSession {
    pub id: String,
    pub title: Option<String>,
    /// Where the session will be resumed on this machine (may differ from the original cwd).
    pub cwd: PathBuf,
    pub original_cwd: Option<PathBuf>,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    pub started_at: OffsetDateTime,
    pub messages: Vec<RehydrateMessage>,
}

#[derive(Debug, thiserror::Error)]
pub enum RehydrateError {
    #[error("{0} sessions can't be rehydrated")]
    Unsupported(&'static str),
    #[error("a transcript for this session already exists at {}", .0.display())]
    AlreadyExists(PathBuf),
    #[error("the harness's data directory could not be found")]
    NoDataDir,
    #[error("rehydrating failed: {0}")]
    Other(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
