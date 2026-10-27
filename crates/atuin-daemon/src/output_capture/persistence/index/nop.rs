use atuin_client::history::HistoryId;
use atuin_common::db::sqlite::fts::TextHighlighter;
use atuin_common::futures::stream::ChunkedStream;

use super::{HighlightedCapture, Index, IndexError, RankedMatch};

/// An [`Index`] that stores nothing and matches nothing.
#[derive(Debug, Clone, Copy)]
pub struct NopIndex;

impl Index for NopIndex {
    async fn insert(
        &self,
        _id: HistoryId,
        _head: &str,
        _tail: Option<&str>,
    ) -> Result<(), IndexError> {
        Ok(())
    }

    async fn remove(&self, _ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError> {
        Ok(())
    }

    async fn search(
        &self,
        _terms: impl IntoIterator<Item = &str>,
        _limit: usize,
    ) -> ChunkedStream<Result<RankedMatch, IndexError>> {
        ChunkedStream::empty()
    }

    async fn highlight(
        &self,
        _terms: impl IntoIterator<Item = &str>,
        head: &str,
        tail: Option<&str>,
    ) -> Result<HighlightedCapture, IndexError> {
        Ok(HighlightedCapture::unmarked(TextHighlighter::default(), head, tail))
    }

    async fn indexed_ids(&self) -> ChunkedStream<Result<HistoryId, IndexError>> {
        ChunkedStream::empty()
    }
}
