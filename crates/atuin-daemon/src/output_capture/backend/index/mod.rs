//! The full-text search index over captured output.
//!
//! Note this is intended to be a **shallow** index and should not actually store the data. See
//! [`super::storage::Storage`] for the storage layer.

mod nop;
mod sqlite;

use atuin_client::history::HistoryId;
use atuin_common::futures::stream::ChunkedStream;
pub use nop::NopIndex;
pub use sqlite::SqliteIndex;
use thiserror::Error;

use crate::output_capture::OutputMatch;

pub type IndexStorageError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, Error)]
pub enum IndexError {
    #[error("search index storage error: {0}")]
    Storage(#[source] IndexStorageError),
}

#[allow(async_fn_in_trait, reason = "only used within our code; no Send bound needed")]
pub trait Index {
    /// Index (replacing any prior entry for `id`) the visible text of one capture.
    async fn insert(&self, id: HistoryId, text: &str) -> Result<(), IndexError>;

    /// Drop every id in `ids` from the index. Absent ids are ignored.
    async fn remove(&self, ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError>;

    /// Relevance-ranked matches, most relevant first.
    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> ChunkedStream<Result<OutputMatch, IndexError>>;

    /// Every id currently held in the index.
    async fn indexed_ids(&self) -> ChunkedStream<Result<HistoryId, IndexError>>;
}

#[derive(Debug)]
pub enum AnyIndex {
    Sqlite(SqliteIndex),
    Nop(NopIndex),
}

impl Index for AnyIndex {
    async fn insert(&self, id: HistoryId, text: &str) -> Result<(), IndexError> {
        match self {
            Self::Sqlite(i) => i.insert(id, text).await,
            Self::Nop(i) => i.insert(id, text).await,
        }
    }

    async fn remove(&self, ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError> {
        match self {
            Self::Sqlite(i) => i.remove(ids).await,
            Self::Nop(i) => i.remove(ids).await,
        }
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> ChunkedStream<Result<OutputMatch, IndexError>> {
        match self {
            Self::Sqlite(i) => i.search(query, limit).await,
            Self::Nop(i) => i.search(query, limit).await,
        }
    }

    async fn indexed_ids(&self) -> ChunkedStream<Result<HistoryId, IndexError>> {
        match self {
            Self::Sqlite(i) => i.indexed_ids().await,
            Self::Nop(i) => i.indexed_ids().await,
        }
    }
}
