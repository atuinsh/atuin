use std::num::NonZeroUsize;

use atuin_client::history::HistoryId;
use atuin_common::db::sqlite::Sqlite;
use atuin_common::db::sqlite::fts::TextHighlighter;
use atuin_common::futures::stream::ChunkedStream;

use super::{HighlightedCapture, IndexError, RankedMatch};

mod v2;

pub use v2::Schema as SchemaV2;

pub type Current = SchemaV2;

pub(super) const CHUNK: NonZeroUsize = NonZeroUsize::new(512).unwrap();

#[allow(async_fn_in_trait, reason = "only used within our code; no Send bound needed")]
pub trait Schema {
    const VERSION: i64;

    async fn setup(db: &Sqlite) -> Result<(), IndexError>;

    async fn insert(
        db: &Sqlite,
        highlighter: TextHighlighter,
        id: HistoryId,
        head: &str,
        tail: Option<&str>,
    ) -> Result<(), IndexError>;

    async fn remove(db: &Sqlite, ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError>;

    async fn search(
        db: &Sqlite,
        terms: impl IntoIterator<Item = &str>,
        limit: usize,
    ) -> ChunkedStream<Result<RankedMatch, IndexError>>;

    async fn highlight(
        db: &Sqlite,
        highlighter: TextHighlighter,
        terms: impl IntoIterator<Item = &str>,
        head: &str,
        tail: Option<&str>,
    ) -> Result<HighlightedCapture, IndexError>;

    async fn indexed_ids(db: &Sqlite) -> ChunkedStream<Result<HistoryId, IndexError>>;
}

pub(super) fn store(err: sqlx::Error) -> IndexError {
    IndexError::Storage(Box::new(err))
}

pub(super) fn id_from_bytes(raw: &[u8]) -> Result<HistoryId, IndexError> {
    let bytes: [u8; 16] = raw.try_into().map_err(|err| IndexError::Storage(Box::new(err)))?;
    Ok(HistoryId::from_bytes(bytes))
}
