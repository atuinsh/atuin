//! The full-text search index over captured output.
//!
//! Note this is intended to be a **shallow** index and should not actually store the data. See
//! [`super::blob::BlobStore`] for the storage layer; search hits are highlighted at query time
//! from the body the caller fetches out of it.

#[cfg(test)]
mod failing;
mod nop;
mod schema;
mod sqlite;

use atuin_client::history::HistoryId;
use atuin_common::db::sqlite::fts::TextHighlighter;
use atuin_common::futures::stream::ChunkedStream;
use atuin_common::string::highlighted::HighlightedString;
use enum_dispatch::enum_dispatch;
#[cfg(test)]
pub use failing::FailingIndex;
pub use nop::NopIndex;
pub use sqlite::SqliteIndex;
use thiserror::Error;

pub type IndexStorageError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, Error)]
pub enum IndexError {
    #[error("search index storage error: {0}")]
    Storage(#[source] IndexStorageError),
}

#[derive(Debug, Clone, Copy)]
pub struct RankedMatch {
    pub history_id: HistoryId,
    pub score: f64,
}

/// A capture's visible text with its matches marked: the kept head and, when the middle was
/// discarded, the kept tail.
#[derive(Clone)]
pub struct HighlightedCapture {
    pub head: HighlightedString,
    pub tail: Option<HighlightedString>,
}

impl HighlightedCapture {
    /// `head` and `tail` with nothing marked, and any markers already in them sanitized away.
    #[must_use]
    pub fn unmarked(highlighter: TextHighlighter, head: &str, tail: Option<&str>) -> Self {
        let unmarked =
            |text: &str| highlighter.as_highlighted(highlighter.sanitize(text).into_owned());
        Self {
            head: unmarked(head),
            tail: tail.map(unmarked),
        }
    }
}

#[enum_dispatch]
#[allow(async_fn_in_trait, reason = "only used within our code; no Send bound needed")]
pub trait Index {
    /// Index (replacing any prior entry for `id`) the visible text of one capture: the kept `head`
    /// and, when the middle was discarded, the kept `tail`. A phrase never matches across the two,
    /// since the words either side of the cut were not next to each other.
    async fn insert(&self, id: HistoryId, head: &str, tail: Option<&str>)
    -> Result<(), IndexError>;

    /// Drop every id in `ids` from the index. Absent ids are ignored.
    async fn remove(&self, ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError>;

    /// Relevance-ranked matches for every one of `terms` (a term of several words as a phrase),
    /// most relevant first.
    async fn search(
        &self,
        terms: impl IntoIterator<Item = &str>,
        limit: usize,
    ) -> ChunkedStream<Result<RankedMatch, IndexError>>;

    /// Mark where `terms` match in a capture's `head` and `tail`, tokenized the way the index was;
    /// unmarked when they no longer match. Markers already present are sanitized away first.
    async fn highlight(
        &self,
        terms: impl IntoIterator<Item = &str>,
        head: &str,
        tail: Option<&str>,
    ) -> Result<HighlightedCapture, IndexError>;

    /// Every id currently held in the index.
    async fn indexed_ids(&self) -> ChunkedStream<Result<HistoryId, IndexError>>;
}

#[enum_dispatch(Index)]
#[derive(Debug, Clone)]
pub enum AnyIndex {
    Sqlite(SqliteIndex),
    Nop(NopIndex),
    #[cfg(test)]
    Failing(FailingIndex),
}
