use atuin_client::history::HistoryId;
use atuin_common::futures::stream::ChunkedStream;

use super::{Index, IndexError, OutputMatch};

/// An [`Index`] that stores nothing and matches nothing.
#[derive(Debug, Clone, Copy)]
pub struct NopIndex;

impl Index for NopIndex {
    async fn insert(&self, _id: HistoryId, _text: &str) -> Result<(), IndexError> {
        Ok(())
    }

    async fn remove(&self, _ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError> {
        Ok(())
    }

    async fn search(
        &self,
        _query: &str,
        _limit: usize,
    ) -> ChunkedStream<Result<OutputMatch, IndexError>> {
        ChunkedStream::empty()
    }

    async fn indexed_ids(&self) -> ChunkedStream<Result<HistoryId, IndexError>> {
        ChunkedStream::empty()
    }
}
