use std::num::NonZeroUsize;

use atuin_client::history::HistoryId;
use atuin_common::db::sqlite::Sqlite;
use atuin_common::db::sqlite::fts::TextHighlighter;
use atuin_common::futures::stream::ChunkedStream;

use super::{IndexError, OutputMatch};

mod v1;

pub use v1::Schema as SchemaV1;

pub type Current = SchemaV1;

pub(super) const CHUNK: NonZeroUsize = NonZeroUsize::new(512).unwrap();

#[allow(async_fn_in_trait, reason = "only used within our code; no Send bound needed")]
pub trait Schema {
    const VERSION: i64;

    async fn setup(db: &Sqlite) -> Result<(), IndexError>;

    async fn insert(
        db: &Sqlite,
        highlighter: TextHighlighter,
        id: HistoryId,
        text: &str,
    ) -> Result<(), IndexError>;

    async fn remove(db: &Sqlite, ids: impl Iterator<Item = HistoryId>) -> Result<(), IndexError>;

    async fn search(
        db: &Sqlite,
        highlighter: TextHighlighter,
        query: &str,
        limit: usize,
        body: impl AsyncFn(HistoryId) -> Option<String>,
    ) -> ChunkedStream<Result<OutputMatch, IndexError>>;

    async fn indexed_ids(db: &Sqlite) -> ChunkedStream<Result<HistoryId, IndexError>>;
}

pub(super) fn store(err: sqlx::Error) -> IndexError {
    IndexError::Storage(Box::new(err))
}

pub(super) fn id_from_bytes(raw: &[u8]) -> Result<HistoryId, IndexError> {
    let bytes: [u8; 16] = raw.try_into().map_err(|err| IndexError::Storage(Box::new(err)))?;
    Ok(HistoryId::from_bytes(bytes))
}
