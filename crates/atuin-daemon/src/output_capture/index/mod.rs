//! The full-text search index over captured output.
//!
//! This is a *derived* index: the fjall backend is the source of truth, and every entry here can
//! be rebuilt from it (see [`OutputCapture::reconcile`](crate::output_capture::OutputCapture)). It
//! mirrors the [`Backend`](super::backend::Backend) shape -- a trait plus an `enum_dispatch` enum --
//! so a disabled store or a failed-to-open index both degrade to a [`NopIndex`] whose `search`
//! returns nothing.

mod nop;
mod sqlite;

use atuin_client::history::HistoryId;
use enum_dispatch::enum_dispatch;
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
    /// A bounded excerpt of the output around the match.
    pub snippet: String,
    /// Relevance, higher is better (a normalized BM25 score).
    pub score: f64,
}

#[enum_dispatch]
#[allow(async_fn_in_trait, reason = "only used within our code; no Send bound needed")]
pub trait Index {
    /// Index (replacing any prior entry for `id`) the visible text of one capture.
    async fn insert(&self, id: HistoryId, text: &str) -> Result<(), IndexError>;

    /// Drop every id in `ids` from the index. Absent ids are ignored.
    async fn remove(&self, ids: &[HistoryId]) -> Result<(), IndexError>;

    /// Relevance-ranked matches with a bounded snippet, most relevant first.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<OutputMatch>, IndexError>;

    /// Every id currently held in the index, for reconciling against the backend.
    async fn indexed_ids(&self) -> Result<Vec<HistoryId>, IndexError>;
}

#[enum_dispatch(Index)]
#[derive(Debug, Clone)]
pub enum AnyIndex {
    Sqlite(SqliteIndex),
    Nop(NopIndex),
}
