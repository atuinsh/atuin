//! Observe changes to a SQLite database written by another process.
//!
//! [`SqliteObserver`] opens a dedicated, read-only connection to an existing database file and
//! streams typed row changes as some other process commits to it. Two regimes are available:
//!
//! - [`SqliteObserver::append`] tails newly-inserted rows via a unique, increasing cursor column
//!   (see [`Tailable`]). Delivery is at-least-once: when the writer deletes rows and recycles
//!   their cursor values, a type that provides [`Tailable::identity`] makes the tail rewind and
//!   re-emit the table from the start rather than skip the recycled rows.
//! - [`SqliteObserver::mutate`] reports inserts, updates and deletes by diffing successive
//!   whole-table snapshots. It rescans the table on every change, so beware the performance cost.
//!
//! # Examples
//!
//! ## Tailing new rows
//!
//! A `rowid` table without `AUTOINCREMENT` recycles the rowids of deleted rows, so the row type
//! exposes its never-reused primary key as the [`identity`](Tailable::identity):
//!
//! ```no_run
//! use atuin_common::db::sqlite::observe::{
//!     ObserveConfig, ReplayBehavior, RowAppendedEvent, SqliteObserver, TableSchema, Tailable,
//! };
//! use futures::StreamExt;
//!
//! // CREATE TABLE messages (id TEXT PRIMARY KEY, body TEXT NOT NULL)
//! #[derive(Clone, sqlx::FromRow)]
//! struct Message {
//!     rowid: i64,
//!     id: String,
//!     body: String,
//! }
//!
//! impl TableSchema for Message {
//!     const TABLE: &'static str = "messages";
//!     const COLUMNS: &'static [&'static str] = &["rowid", "id", "body"];
//! }
//!
//! impl Tailable for Message {
//!     type Cursor = i64;
//!
//!     fn cursor(&self) -> i64 {
//!         self.rowid
//!     }
//!
//!     fn identity(&self) -> Option<impl Eq> {
//!         Some(self.id.as_str())
//!     }
//! }
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let observer = SqliteObserver::new("/path/to/other-process.db");
//!
//! // `ReplayBehavior::All` first emits every existing row, then tails new ones;
//! // `ReplayBehavior::FromNow` (the default) emits only rows committed after start.
//! let mut messages = observer
//!     .append::<Message>(ObserveConfig::builder().replay(ReplayBehavior::All).build())
//!     .await?;
//!
//! while let Some(change) = messages.next().await {
//!     let RowAppendedEvent(message) = change?;
//!     println!("{}: {}", message.id, message.body);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Detecting updates and deletes
//!
//! `mutate` additionally requires [`Diffable`] (a stable key to match rows across snapshots) and
//! yields [`RowChangedEvent`] values:
//!
//! ```no_run
//! # use atuin_common::db::sqlite::observe::{Diffable, ObserveConfig, RowChangedEvent, SqliteObserver, TableSchema};
//! # #[derive(Clone, PartialEq, sqlx::FromRow)]
//! # struct Message { id: i64, body: String }
//! # impl TableSchema for Message {
//! #     const TABLE: &'static str = "messages";
//! #     const COLUMNS: &'static [&'static str] = &["id", "body"];
//! # }
//! impl Diffable for Message {
//!     type Key = i64;
//!     fn key(&self) -> i64 {
//!         self.id
//!     }
//! }
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! # use futures::StreamExt;
//! let observer = SqliteObserver::new("/path/to/other-process.db");
//! let mut changes = observer.mutate::<Message>(ObserveConfig::builder().build()).await?;
//!
//! while let Some(change) = changes.next().await {
//!     match change? {
//!         RowChangedEvent::Inserted(m) => println!("+ {}", m.id),
//!         RowChangedEvent::Updated { old, new } => {
//!             println!("~ {} {:?} -> {:?}", new.id, old.body, new.body)
//!         }
//!         RowChangedEvent::Deleted(m) => println!("- {}", m.id),
//!     }
//! }
//! # Ok(())
//! # }
//! ```

mod driver;

use std::fmt::Debug;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use driver::{AppendStrategy, MutateStrategy, Strategy};
use futures::Stream;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteRow};
use sqlx::{Connection, Decode, Encode, FromRow, Sqlite, Type};
use strum_macros::{Display, EnumDiscriminants, EnumIter};
use typed_builder::TypedBuilder;

use super::quote_ident;

pub trait TableSchema: for<'r> FromRow<'r, SqliteRow> + Clone + Send + Unpin + 'static {
    const TABLE: &'static str;
    const COLUMNS: &'static [&'static str];

    /// A `SELECT` of [`COLUMNS`](Self::COLUMNS) from [`TABLE`](Self::TABLE), followed by `clause`.
    #[must_use]
    fn select(clause: &str) -> String {
        let cols = Self::COLUMNS.iter().map(|&c| quote_ident(c)).collect::<Vec<_>>().join(", ");
        format!("SELECT {cols} FROM {} {clause}", quote_ident(Self::TABLE))
    }
}

/// A value [`Tailable`] rows are ordered by, such as an `i64` rowid.
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

/// A table row whose newly-inserted rows can be tailed in commit order.
///
/// Implementing this trait enables you to use the [`SqliteObserver::append`] utility to track
/// appends to the table.
///
/// Important notes:
///   - [`CURSOR_COLUMN`](Self::CURSOR_COLUMN) must be **unique and increasing among live rows**.
///   - Note that `AUTOINCREMENT` rows work only if rows do not get deleted. If a row gets deleted,
///     `AUTOINCREMENT` rows can result in recycling the value, which would cause conflicts. In that
///     case, implement [`Self::identity`].
pub trait Tailable: TableSchema {
    /// The type of [`CURSOR_COLUMN`](Self::CURSOR_COLUMN), such as `i64` for a `rowid`.
    type Cursor: Cursor;

    /// The name of the cursor column.
    const CURSOR_COLUMN: &'static str = "rowid";

    /// This row's value of [`CURSOR_COLUMN`](Self::CURSOR_COLUMN).
    fn cursor(&self) -> Self::Cursor;

    /// A never-reused value, such as a primary key, telling apart rows that reuse a cursor value.
    fn identity(&self) -> Option<impl Eq> {
        None::<()>
    }
}

/// A table row whose inserts, updates and deletes can be detected by diffing table snapshots.
///
/// Implementing this trait enables you to use the [`SqliteObserver::mutate`] utility to track
/// changes to the table. Rows are matched across snapshots by [`key`](Self::key): a key that
/// disappears is a delete, a new key an insert, and a key whose row is no longer [`PartialEq`] to
/// its previous one an update.
///
/// Important notes:
///   - [`key`](Self::key) must be **unique among live rows and stable across updates**. A row whose
///     key changes is reported as a delete plus an insert.
///   - Every commit to the database rescans the whole table, so keep observed tables small.
pub trait Diffable: TableSchema + PartialEq {
    /// The type rows are matched by across snapshots, such as an `i64` primary key.
    type Key: Ord + Clone + Send + Sync + 'static;

    /// This row's [`Key`](Self::Key).
    fn key(&self) -> Self::Key;
}

/// Whether an observation first emits the rows already in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Display)]
pub enum ReplayBehavior {
    #[default]
    FromNow,
    All,
}

/// Options for a stream started by [`SqliteObserver::append`] or [`SqliteObserver::mutate`].
#[derive(Debug, Clone, Copy, TypedBuilder)]
pub struct ObserveConfig {
    #[builder(default)]
    pub replay: ReplayBehavior,
    #[builder(default = Duration::from_millis(250))]
    pub poll_interval: Duration,
}

/// A failure to open or query the observed database.
#[derive(Debug, thiserror::Error)]
pub enum ObserveError {
    #[error("failed to open the observer connection: {0}")]
    Connect(#[source] sqlx::Error),
    #[error("the initial replay/seed scan failed: {0}")]
    Seed(#[source] sqlx::Error),
    #[error("a query against the observed database failed: {0}")]
    Query(#[source] sqlx::Error),
}

/// A row appended to the table tailed by [`SqliteObserver::append`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowAppendedEvent<T>(pub T);

/// An insert, update or delete [`SqliteObserver::mutate`] saw between two table snapshots.
#[derive(Debug, Clone, PartialEq, Eq, EnumDiscriminants)]
#[strum_discriminants(name(RowChangeKind), derive(Display, EnumIter, Hash))]
pub enum RowChangedEvent<T> {
    Inserted(T),
    Updated {
        old: T,
        new: T,
    },
    Deleted(T),
}

/// Observes a SQLite database written by another process, one connection per stream.
///
/// See [`self`].
#[derive(Debug, Clone)]
pub struct SqliteObserver {
    opts: SqliteConnectOptions,
}

impl SqliteObserver {
    #[must_use]
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self::from_options(
            SqliteConnectOptions::new()
                .filename(path)
                .read_only(true)
                .immutable(false)
                .busy_timeout(Duration::from_secs(5)),
        )
    }

    #[must_use]
    pub const fn from_options(opts: SqliteConnectOptions) -> Self {
        Self { opts }
    }

    /// Tails newly-inserted rows of `T`'s table (see [`Tailable`]).
    ///
    /// # Errors
    ///
    /// [`ObserveError::Connect`] when the database cannot be opened and [`ObserveError::Seed`]
    /// when the initial replay scan fails; later failures are yielded as items the stream carries
    /// on after.
    pub async fn append<T: Tailable>(
        &self,
        cfg: ObserveConfig,
    ) -> Result<SqliteTableObserver<RowAppendedEvent<T>>, ObserveError> {
        let mut conn =
            SqliteConnection::connect_with(&self.opts).await.map_err(ObserveError::Connect)?;
        let mut strategy = AppendStrategy::<T>::new();
        strategy.seed(&mut conn, cfg.replay).await.map_err(ObserveError::Seed)?;
        Ok(SqliteTableObserver::new(strategy.run(conn, cfg)))
    }

    /// Reports inserts, updates and deletes to `T`'s table (see [`Diffable`]).
    ///
    /// # Errors
    ///
    /// [`ObserveError::Connect`] when the database cannot be opened and [`ObserveError::Seed`]
    /// when the initial snapshot fails; later failures are yielded as items the stream carries
    /// on after.
    pub async fn mutate<T: Diffable>(
        &self,
        cfg: ObserveConfig,
    ) -> Result<SqliteTableObserver<RowChangedEvent<T>>, ObserveError> {
        let mut conn =
            SqliteConnection::connect_with(&self.opts).await.map_err(ObserveError::Connect)?;
        let mut strategy = MutateStrategy::<T>::new();
        strategy.seed(&mut conn, cfg.replay).await.map_err(ObserveError::Seed)?;
        Ok(SqliteTableObserver::new(strategy.run(conn, cfg)))
    }
}

pub struct SqliteTableObserver<E> {
    inner: Pin<Box<dyn Stream<Item = Result<E, ObserveError>> + Send>>,
}

impl<E> SqliteTableObserver<E> {
    fn new(inner: impl Stream<Item = Result<E, ObserveError>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(inner),
        }
    }
}

impl<E> Stream for SqliteTableObserver<E> {
    type Item = Result<E, ObserveError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[tokio::test]
    async fn handle_forwards_stream_items() {
        let inner = futures::stream::iter(vec![Ok::<u32, ObserveError>(1), Ok(2), Ok(3)]);
        let observer = SqliteTableObserver::new(inner);

        let got: Vec<u32> = observer.map(Result::unwrap).collect().await;
        assert_eq!(got, vec![1, 2, 3]);
    }
}
