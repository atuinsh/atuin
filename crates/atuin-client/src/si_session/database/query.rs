//! A small query builder over a synchronous rusqlite connection, in the shape of the sqlx calls
//! the rest of the sidecar makes (`query(..).bind(..).execute(..)`, `fetch_one`,
//! `fetch_optional`, `fetch_all`), so the write path reads like the reads around it. Statements
//! go through the connection's prepared statement cache.

use std::marker::PhantomData;

use rusqlite::types::FromSql;
use rusqlite::{Connection, Row, ToSql};

/// What [`Query::execute`] did.
pub(super) struct Done {
    rows: usize,
    rowid: i64,
}

impl Done {
    pub(super) fn rows_affected(&self) -> u64 {
        u64::try_from(self.rows).unwrap_or(u64::MAX)
    }

    pub(super) const fn last_insert_rowid(&self) -> i64 {
        self.rowid
    }
}

/// A row decoded whole: a tuple by position, or a struct by column name.
pub(super) trait FromRow: Sized {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self>;
}

macro_rules! tuple_from_row {
    ($($t:ident $i:tt),+) => {
        impl<$($t: FromSql),+> FromRow for ($($t,)+) {
            fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
                Ok(($(row.get($i)?,)+))
            }
        }
    };
}
tuple_from_row!(A 0, B 1);
tuple_from_row!(A 0, B 1, C 2);
tuple_from_row!(A 0, B 1, C 2, D 3);
tuple_from_row!(A 0, B 1, C 2, D 3, E 4, F 5);

/// How a fetched row becomes what its query returns: the whole row ([`FromRow`]), or its first
/// column ([`Scalar`]).
pub(super) trait Fetch<O> {
    fn fetch(row: &Row<'_>) -> rusqlite::Result<O>;
}

impl<O: FromRow> Fetch<O> for O {
    fn fetch(row: &Row<'_>) -> rusqlite::Result<O> {
        O::from_row(row)
    }
}

/// The first column of a row, for [`query_scalar`].
pub(super) struct Scalar;

impl<O: FromSql> Fetch<O> for Scalar {
    fn fetch(row: &Row<'_>) -> rusqlite::Result<O> {
        row.get(0)
    }
}

/// A statement and its parameters, fetching rows as `O` by `F`.
pub(super) struct Query<'q, O, F> {
    sql: &'q str,
    params: Vec<Box<dyn ToSql + 'q>>,
    out: PhantomData<fn() -> (O, F)>,
}

/// A statement run for its effect.
pub(super) fn query(sql: &str) -> Query<'_, (), ()> {
    Query::new(sql)
}

/// A statement fetching whole rows.
pub(super) fn query_as<O: FromRow>(sql: &str) -> Query<'_, O, O> {
    Query::new(sql)
}

/// A statement fetching the first column of each row.
pub(super) fn query_scalar<O: FromSql>(sql: &str) -> Query<'_, O, Scalar> {
    Query::new(sql)
}

impl<'q, O, F> Query<'q, O, F> {
    fn new(sql: &'q str) -> Self {
        Self {
            sql,
            params: Vec::new(),
            out: PhantomData,
        }
    }

    /// Bind the next positional parameter.
    pub(super) fn bind(mut self, value: impl ToSql + 'q) -> Self {
        self.params.push(Box::new(value));
        self
    }

    fn params(&self) -> Vec<&dyn ToSql> {
        self.params.iter().map(|p| &**p as &dyn ToSql).collect()
    }

    pub(super) fn execute(self, conn: &Connection) -> rusqlite::Result<Done> {
        let rows = conn.prepare_cached(self.sql)?.execute(&*self.params())?;
        Ok(Done {
            rows,
            rowid: conn.last_insert_rowid(),
        })
    }
}

impl<O, F: Fetch<O>> Query<'_, O, F> {
    pub(super) fn fetch_all(self, conn: &Connection) -> rusqlite::Result<Vec<O>> {
        let mut stmt = conn.prepare_cached(self.sql)?;
        let rows = stmt.query_map(&*self.params(), |row| F::fetch(row))?;
        rows.collect()
    }

    pub(super) fn fetch_optional(self, conn: &Connection) -> rusqlite::Result<Option<O>> {
        let mut stmt = conn.prepare_cached(self.sql)?;
        let mut rows = stmt.query(&*self.params())?;
        rows.next()?.map(|row| F::fetch(row)).transpose()
    }

    pub(super) fn fetch_one(self, conn: &Connection) -> rusqlite::Result<O> {
        self.fetch_optional(conn)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
    }
}
