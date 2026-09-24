use std::fmt::Debug;

use sqlx::sqlite::SqliteRow;
use sqlx::{Decode, Encode, FromRow, Sqlite, Type};

pub trait TableSchema: for<'r> FromRow<'r, SqliteRow> + Clone + Send + Unpin + 'static {
    const TABLE: &'static str;
    const COLUMNS: &'static [&'static str];
}

pub trait Cursor:
    Ord
    + Clone
    + Debug
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
        + Debug
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
/// [`CURSOR_COLUMN`](Self::CURSOR_COLUMN) must be **unique and increasing among live rows** —
/// every insert takes a larger value than any row present at that moment (`rowid`, the default,
/// and `AUTOINCREMENT` integer keys satisfy this). The tail resumes each page with
/// `WHERE cursor > last_delivered`, so a repeated value is dropped at a page boundary and a
/// non-increasing one is skipped entirely; a timestamp or other non-unique column is not a valid
/// cursor.
///
/// When rows can be deleted and their cursor values later reused (a `rowid` table without
/// `AUTOINCREMENT` assigns `max(rowid) + 1`, so deleting the highest rows recycles their rowids for
/// the next inserts), implement [`identity`](Self::identity). Every page then starts at the last
/// delivered row and, when that row is gone or carries a different identity, the tail rewinds to
/// the start of the table so rows occupying recycled cursor values are still delivered.
/// Delivery is therefore at-least-once: a rewind re-emits every live row, including rows that
/// predate a [`ReplayBehavior::FromNow`](super::ReplayBehavior::FromNow) start, and consumers must dedup by
/// identity.
pub trait Tailable: TableSchema {
    type Cursor: Cursor;
    const CURSOR_COLUMN: &'static str = "rowid";
    fn cursor(&self) -> Self::Cursor;

    /// A value that differs between any two rows that could ever occupy the same cursor value,
    /// such as a primary key that is never reused. `None` (the default) declares that cursor
    /// values are never reused and disables the rewind check.
    ///
    /// Reusing a cursor value `r <= cursor` requires every row from `r` upwards, the last
    /// delivered row included, to have been deleted first, so re-checking that single row catches
    /// every reuse. The one blind spot is a row re-inserted with the *same* identity at exactly
    /// the last delivered cursor value after lower values were recycled; only a replay from the
    /// start (a fresh observer with [`ReplayBehavior::All`](super::ReplayBehavior::All)) recovers from that.
    fn identity(&self) -> Option<String> {
        None
    }
}

pub trait Diffable: TableSchema + PartialEq {
    type Key: Ord + Clone + Send + Sync + 'static;
    fn key(&self) -> Self::Key;
}
