//! Observe changes to a SQLite database written by another process.
//!
//! [`SqliteObserver`] opens a dedicated, read-only connection to an existing database file and
//! streams typed row changes as some other process commits to it (it detects cross-process commits
//! via `PRAGMA data_version`). Two regimes are available:
//!
//! - [`SqliteObserver::append`] tails newly-inserted rows via a monotonic cursor column.
//! - [`SqliteObserver::mutate`] reports inserts, updates and deletes by diffing successive
//!   whole-table snapshots. It rescans the table on every change, so prefer it for bounded tables.
//!
//! A row type implements [`TableSchema`] (its table name and explicit column list) plus
//! [`Tailable`] (for `append`) or [`Diffable`] (for `mutate`). Each observer owns its own
//! connection; the returned [`SqliteTableObserver`] is a self-driving `Stream` of `Result<_,
//! `[`ObserveError`]`>` that polls the source only while it is consumed (so backpressure is
//! intrinsic), and dropping it releases the connection and stops observing.
//!
//! # Examples
//!
//! ## Tailing new rows
//!
//! ```no_run
//! use atuin_common::db::sqlite::observe::{
//!     Appended, ObserveConfig, Replay, SqliteObserver, TableSchema, Tailable,
//! };
//! use futures::StreamExt;
//!
//! #[derive(Clone, sqlx::FromRow)]
//! struct Message {
//!     id: i64,
//!     body: String,
//! }
//!
//! impl TableSchema for Message {
//!     const TABLE: &'static str = "messages";
//!     const COLUMNS: &'static [&'static str] = &["id", "body"];
//! }
//!
//! impl Tailable for Message {
//!     type Cursor = i64;
//!     const CURSOR_COLUMN: &'static str = "id";
//!     fn cursor(&self) -> i64 {
//!         self.id
//!     }
//! }
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let observer = SqliteObserver::new("/path/to/other-process.db");
//!
//! // `Replay::All` first emits every existing row, then tails new ones;
//! // `Replay::FromNow` (the default) emits only rows committed after start.
//! let mut messages =
//!     observer.append::<Message>(ObserveConfig::builder().replay(Replay::All).build()).await?;
//!
//! while let Some(change) = messages.next().await {
//!     let Appended(message) = change?;
//!     println!("{}: {}", message.id, message.body);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Detecting updates and deletes
//!
//! `mutate` additionally requires [`Diffable`] (a stable key to match rows across snapshots) and
//! yields [`Change`] values:
//!
//! ```no_run
//! # use atuin_common::db::sqlite::observe::{Change, Diffable, ObserveConfig, SqliteObserver, TableSchema};
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
//!         Change::Inserted(m) => println!("+ {}", m.id),
//!         Change::Updated { old, new } => println!("~ {} {:?} -> {:?}", new.id, old.body, new.body),
//!         Change::Deleted(m) => println!("- {}", m.id),
//!     }
//! }
//! # Ok(())
//! # }
//! ```

mod config;
mod driver;
mod error;
mod event;
mod schema;
mod table;

use std::path::Path;
use std::time::Duration;

pub use config::{ObserveConfig, Replay};
use driver::{AppendStrategy, MutateStrategy, Strategy, run};
pub use error::ObserveError;
pub use event::{Appended, Change, ChangeKind};
pub use schema::{Cursor, Diffable, TableSchema, Tailable};
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
pub use table::SqliteTableObserver;

#[derive(Debug, Clone)]
pub struct SqliteObserver {
    opts: SqliteConnectOptions,
}

impl SqliteObserver {
    #[must_use]
    pub fn new(path: impl AsRef<Path>) -> Self {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .immutable(false)
            .busy_timeout(Duration::from_secs(5));
        Self { opts }
    }

    #[must_use]
    pub fn from_options(opts: SqliteConnectOptions) -> Self {
        Self { opts }
    }

    pub async fn append<T: Tailable>(
        &self,
        cfg: ObserveConfig,
    ) -> Result<SqliteTableObserver<Appended<T>>, ObserveError> {
        let mut conn =
            SqliteConnection::connect_with(&self.opts).await.map_err(ObserveError::Connect)?;
        let mut strategy = AppendStrategy::<T>::new();
        strategy.seed(&mut conn, cfg.replay).await.map_err(ObserveError::Seed)?;
        Ok(SqliteTableObserver::new(run(self.opts.clone(), conn, strategy, cfg)))
    }

    pub async fn mutate<T: Diffable>(
        &self,
        cfg: ObserveConfig,
    ) -> Result<SqliteTableObserver<Change<T>>, ObserveError> {
        let mut conn =
            SqliteConnection::connect_with(&self.opts).await.map_err(ObserveError::Connect)?;
        let mut strategy = MutateStrategy::<T>::new();
        strategy.seed(&mut conn, cfg.replay).await.map_err(ObserveError::Seed)?;
        Ok(SqliteTableObserver::new(run(self.opts.clone(), conn, strategy, cfg)))
    }
}
