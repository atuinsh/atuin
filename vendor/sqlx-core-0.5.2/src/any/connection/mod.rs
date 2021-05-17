use futures_core::future::BoxFuture;

use crate::any::{Any, AnyConnectOptions};
use crate::connection::Connection;
use crate::error::Error;

#[cfg(feature = "postgres")]
use crate::postgres;

#[cfg(feature = "sqlite")]
use crate::sqlite;

#[cfg(feature = "mssql")]
use crate::mssql;

#[cfg(feature = "mysql")]
use crate::mysql;
use crate::transaction::Transaction;

mod establish;
mod executor;

/// A connection to _any_ SQLx database.
///
/// The database driver used is determined by the scheme
/// of the connection url.
///
/// ```text
/// postgres://postgres@localhost/test
/// sqlite://a.sqlite
/// ```
#[derive(Debug)]
pub struct AnyConnection(pub(super) AnyConnectionKind);

#[derive(Debug)]
pub(crate) enum AnyConnectionKind {
    #[cfg(feature = "postgres")]
    Postgres(postgres::PgConnection),

    #[cfg(feature = "mssql")]
    Mssql(mssql::MssqlConnection),

    #[cfg(feature = "mysql")]
    MySql(mysql::MySqlConnection),

    #[cfg(feature = "sqlite")]
    Sqlite(sqlite::SqliteConnection),
}

macro_rules! delegate_to {
    ($self:ident.$method:ident($($arg:ident),*)) => {
        match &$self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn.$method($($arg),*),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn.$method($($arg),*),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn.$method($($arg),*),

            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(conn) => conn.$method($($arg),*),
        }
    };
}

macro_rules! delegate_to_mut {
    ($self:ident.$method:ident($($arg:ident),*)) => {
        match &mut $self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn.$method($($arg),*),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn.$method($($arg),*),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn.$method($($arg),*),

            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(conn) => conn.$method($($arg),*),
        }
    };
}

impl Connection for AnyConnection {
    type Database = Any;

    type Options = AnyConnectOptions;

    fn close(self) -> BoxFuture<'static, Result<(), Error>> {
        match self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn.close(),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn.close(),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn.close(),

            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(conn) => conn.close(),
        }
    }

    fn ping(&mut self) -> BoxFuture<'_, Result<(), Error>> {
        delegate_to_mut!(self.ping())
    }

    fn begin(&mut self) -> BoxFuture<'_, Result<Transaction<'_, Self::Database>, Error>>
    where
        Self: Sized,
    {
        Transaction::begin(self)
    }

    fn cached_statements_size(&self) -> usize {
        match &self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn.cached_statements_size(),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn.cached_statements_size(),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn.cached_statements_size(),

            // no cache
            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(_) => 0,
        }
    }

    fn clear_cached_statements(&mut self) -> BoxFuture<'_, Result<(), Error>> {
        match &mut self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn.clear_cached_statements(),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn.clear_cached_statements(),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn.clear_cached_statements(),

            // no cache
            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(_) => Box::pin(futures_util::future::ok(())),
        }
    }

    #[doc(hidden)]
    fn flush(&mut self) -> BoxFuture<'_, Result<(), Error>> {
        delegate_to_mut!(self.flush())
    }

    #[doc(hidden)]
    fn should_flush(&self) -> bool {
        delegate_to!(self.should_flush())
    }
}
