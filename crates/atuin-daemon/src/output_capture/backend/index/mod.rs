//! The full-text search index over captured output.
//!
//! This is a *derived* index: the [`Storage`](super::storage::Storage) is the source of truth, and
//! every entry here can be rebuilt from it (see [`Backend::reconcile`](super::Backend::reconcile)).
//! The index is **not** responsible for storing the captures themselves.

mod nop;
mod sqlite;

use std::ops::Range;

use atuin_client::history::HistoryId;
pub use nop::NopIndex;
pub use sqlite::SqliteIndex;
use thiserror::Error;

pub type IndexBackendError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, Error)]
pub enum IndexError {
    #[error("search index storage error: {0}")]
    Storage(#[source] IndexBackendError),
}

/// A single relevance-ranked full-text match over captured output.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputMatch {
    /// The command whose output matched.
    pub history_id: HistoryId,
    /// The full escape-stripped output that was indexed, so the caller can show the match in
    /// context without re-fetching it.
    pub output: String,
    /// Byte ranges within `output` covering each match; may be empty or hold several.
    pub matches: Vec<Range<usize>>,
    /// Relevance, higher is better (a normalized BM25 score).
    pub score: f64,
}

#[allow(async_fn_in_trait, reason = "only used within our code; no Send bound needed")]
pub trait Index {
    /// Index (replacing any prior entry for `id`) the visible text of one capture.
    async fn insert(&self, id: HistoryId, text: &str) -> Result<(), IndexError>;

    /// Drop every id in `ids` from the index. Absent ids are ignored.
    async fn remove(&self, ids: &[HistoryId]) -> Result<(), IndexError>;

    /// Relevance-ranked matches, most relevant first: each carries the full output and the byte
    /// ranges of every match within it.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<OutputMatch>, IndexError>;

    /// Every id currently held in the index, for reconciling against the storage.
    async fn indexed_ids(&self) -> Result<Vec<HistoryId>, IndexError>;
}
