//! The full-text search index over captured output.
//!
//! Note this is intended to be a **shallow** index and should not actually store the data. See
//! [`super::blob::BlobStore`] for the storage layer.

mod nop;
mod sqlite;

use atuin_client::history::HistoryId;
use atuin_common::futures::stream::ChunkedStream;
use enum_dispatch::enum_dispatch;
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

#[enum_dispatch]
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

#[enum_dispatch(Index)]
#[derive(Debug)]
pub enum AnyIndex {
    Sqlite(SqliteIndex),
    Nop(NopIndex),
}
