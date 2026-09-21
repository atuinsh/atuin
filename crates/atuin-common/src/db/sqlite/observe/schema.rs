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

pub trait Tailable: TableSchema {
    type Cursor: Cursor;
    const CURSOR_COLUMN: &'static str = "rowid";
    fn cursor(&self) -> Self::Cursor;
}

pub trait Diffable: TableSchema + PartialEq {
    type Key: Ord + Clone + Send + Sync + 'static;
    fn key(&self) -> Self::Key;
}
