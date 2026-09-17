use atuin_client::history::HistoryId;
use atuin_common::db::sqlite::fts::TextHighlighter;
use atuin_common::futures::stream::ChunkedStream;
use atuin_common::string::highlighted::HighlightedString;

use super::{Index, IndexError, RankedMatch};

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
    ) -> ChunkedStream<Result<RankedMatch, IndexError>> {
        ChunkedStream::empty()
    }

    async fn highlight(&self, _query: &str, body: &str) -> Result<HighlightedString, IndexError> {
        let highlighter = TextHighlighter::default();
        Ok(highlighter.as_highlighted(highlighter.sanitize(body).into_owned()))
    }

    async fn indexed_ids(&self) -> ChunkedStream<Result<HistoryId, IndexError>> {
        ChunkedStream::empty()
    }
}
