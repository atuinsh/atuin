use sqlx::sqlite::SqliteRow;
use sqlx::{Decode, Encode, FromRow, Sqlite, Type};

pub trait TableSchema: for<'r> FromRow<'r, SqliteRow> + Clone + Send + Unpin + 'static {
    const TABLE: &'static str;
    const COLUMNS: &'static [&'static str];
}

pub trait Cursor:
    Ord
    + Clone
    + Send
    + Sync
    + Unpin
    + 'static
    + Type<Sqlite>
    + for<'q> Encode<'q, Sqlite>
    + for<'r> Decode<'r, Sqlite>
{
}

impl<C> Cursor for C where
    C: Ord
        + Clone
        + Send
        + Sync
        + Unpin
        + 'static
        + Type<Sqlite>
        + for<'q> Encode<'q, Sqlite>
        + for<'r> Decode<'r, Sqlite>
{
}

/// A table whose newly-inserted rows can be tailed in commit order.
///
/// [`CURSOR_COLUMN`](Self::CURSOR_COLUMN) must be **unique and strictly increasing** — every insert
/// takes a larger value than any existing row (`rowid`, the default, and `AUTOINCREMENT` integer
/// keys satisfy this). The tail resumes each page with `WHERE cursor > last_delivered`, so a
/// repeated value is dropped at a page boundary and a non-increasing one is skipped entirely; a
/// timestamp or other non-unique column is not a valid cursor.
pub trait Tailable: TableSchema {
    type Cursor: Cursor;
    const CURSOR_COLUMN: &'static str = "rowid";
    fn cursor(&self) -> Self::Cursor;
}

pub trait Diffable: TableSchema + PartialEq {
    type Key: Ord + Clone + Send + Sync + 'static;
    fn key(&self) -> Self::Key;
}
