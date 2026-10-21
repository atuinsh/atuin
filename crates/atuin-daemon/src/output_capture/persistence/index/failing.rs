use atuin_client::history::HistoryId;
use atuin_common::futures::stream::ChunkedStream;

use super::{HighlightedCapture, Index, IndexError, RankedMatch};

/// An [`Index`] whose every operation fails, standing in for a broken search index.
///
/// This is for tests.
#[derive(Debug, Clone, Copy)]
pub struct FailingIndex;

fn unavailable() -> IndexError {
    IndexError::Storage("the output search index is unavailable".into())
}

impl Index for FailingIndex {
    async fn insert(
        &self,
        _id: HistoryId,
        _head: &str,
        _tail: Option<&str>,
    ) -> Result<(), IndexError> {
        Err(unavailable())
    }

    async fn remove(&self, _ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError> {
        Err(unavailable())
    }

    async fn search(
        &self,
        _terms: impl IntoIterator<Item = &str>,
        _limit: usize,
    ) -> ChunkedStream<Result<RankedMatch, IndexError>> {
        ChunkedStream::from_error(unavailable())
    }

    async fn highlight(
        &self,
        _terms: impl IntoIterator<Item = &str>,
        _head: &str,
        _tail: Option<&str>,
    ) -> Result<HighlightedCapture, IndexError> {
        Err(unavailable())
    }

    async fn indexed_ids(&self) -> ChunkedStream<Result<HistoryId, IndexError>> {
        ChunkedStream::from_error(unavailable())
    }
}
