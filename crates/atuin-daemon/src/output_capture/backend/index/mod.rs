//! The full-text search index over captured output.
//!
//! Note this is intended to be a **shallow** index and should not actually store the data. See
//! [`super::storage::Storage`] for the storage layer.

mod nop;
mod sqlite;

use atuin_client::history::HistoryId;
use atuin_common::string::highlighted::HighlightedString;
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
#[derive(Debug)]
pub struct OutputMatch {
    pub history_id: HistoryId,
    pub output: HighlightedString,
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
