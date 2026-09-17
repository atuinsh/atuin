use atuin_client::history::HistoryId;
use atuin_common::db::sqlite::fts::TextHighlighter;
use atuin_common::futures::stream::ChunkedStream;
use atuin_common::string::highlighted::HighlightedString;
use futures::{Stream, StreamExt};

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

    async fn highlight(
        &self,
        _query: &str,
        bodies: impl Stream<Item = (HistoryId, impl AsRef<str> + Send + Sync + 'static)>
        + Send
        + 'static,
    ) -> ChunkedStream<Result<(HistoryId, HighlightedString), IndexError>> {
        let highlighter = TextHighlighter::default();
        ChunkedStream::new(bodies.map(move |(id, body)| {
            let body = highlighter.as_highlighted(highlighter.sanitize(body.as_ref()).into_owned());
            vec![Ok((id, body))]
        }))
    }

    async fn indexed_ids(&self) -> ChunkedStream<Result<HistoryId, IndexError>> {
        ChunkedStream::empty()
    }
}
