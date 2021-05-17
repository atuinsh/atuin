//! **SQLite** database driver.

// SQLite is a C library. All interactions require FFI which is unsafe.
// All unsafe blocks should have comments pointing to SQLite docs and ensuring that we maintain
// invariants.
#![allow(unsafe_code)]

mod arguments;
mod column;
mod connection;
mod database;
mod error;
mod options;
mod query_result;
mod row;
mod statement;
mod transaction;
mod type_info;
pub mod types;
mod value;

#[cfg(feature = "migrate")]
mod migrate;

pub use arguments::{SqliteArgumentValue, SqliteArguments};
pub use column::SqliteColumn;
pub use connection::SqliteConnection;
pub use database::Sqlite;
pub use error::SqliteError;
pub use options::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
pub use query_result::SqliteQueryResult;
pub use row::SqliteRow;
pub use statement::SqliteStatement;
pub use transaction::SqliteTransactionManager;
pub use type_info::SqliteTypeInfo;
pub use value::{SqliteValue, SqliteValueRef};

/// An alias for [`Pool`][crate::pool::Pool], specialized for SQLite.
pub type SqlitePool = crate::pool::Pool<Sqlite>;

/// An alias for [`PoolOptions`][crate::pool::PoolOptions], specialized for SQLite.
pub type SqlitePoolOptions = crate::pool::PoolOptions<Sqlite>;

// NOTE: required due to the lack of lazy normalization
impl_into_arguments_for_arguments!(SqliteArguments<'q>);
impl_executor_for_pool_connection!(Sqlite, SqliteConnection, SqliteRow);
impl_executor_for_transaction!(Sqlite, SqliteRow);
impl_column_index_for_row!(SqliteRow);
impl_column_index_for_statement!(SqliteStatement);
impl_acquire!(Sqlite, SqliteConnection);
impl_into_maybe_pool!(Sqlite, SqliteConnection);

// required because some databases have a different handling of NULL
impl_encode_for_option!(Sqlite);
