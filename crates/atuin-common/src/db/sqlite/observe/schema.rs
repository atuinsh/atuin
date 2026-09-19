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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    struct Item {
        id: i64,
        name: String,
    }

    impl TableSchema for Item {
        const TABLE: &'static str = "items";
        const COLUMNS: &'static [&'static str] = &["id", "name"];
    }
    impl Tailable for Item {
        type Cursor = i64;
        const CURSOR_COLUMN: &'static str = "id";
        fn cursor(&self) -> i64 {
            self.id
        }
    }
    impl Diffable for Item {
        type Key = i64;
        fn key(&self) -> i64 {
            self.id
        }
    }

    #[rstest]
    fn schema_exposes_cursor_and_key() {
        let item = Item { id: 7, name: "x".into() };
        assert_eq!(<Item as Tailable>::CURSOR_COLUMN, "id");
        assert_eq!(Item::COLUMNS, &["id", "name"]);
        assert_eq!(item.cursor(), 7);
        assert_eq!(item.key(), 7);
    }
}
