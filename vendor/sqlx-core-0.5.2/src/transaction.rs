use std::borrow::Cow;
use std::fmt::{self, Debug, Formatter};
use std::ops::{Deref, DerefMut};

use futures_core::future::BoxFuture;

use crate::database::Database;
use crate::error::Error;
use crate::pool::MaybePoolConnection;

/// Generic management of database transactions.
///
/// This trait should not be used, except when implementing [`Connection`].
#[doc(hidden)]
pub trait TransactionManager {
    type Database: Database;

    /// Begin a new transaction or establish a savepoint within the active transaction.
    fn begin(
        conn: &mut <Self::Database as Database>::Connection,
    ) -> BoxFuture<'_, Result<(), Error>>;

    /// Commit the active transaction or release the most recent savepoint.
    fn commit(
        conn: &mut <Self::Database as Database>::Connection,
    ) -> BoxFuture<'_, Result<(), Error>>;

    /// Abort the active transaction or restore from the most recent savepoint.
    fn rollback(
        conn: &mut <Self::Database as Database>::Connection,
    ) -> BoxFuture<'_, Result<(), Error>>;

    /// Starts to abort the active transaction or restore from the most recent snapshot.
    fn start_rollback(conn: &mut <Self::Database as Database>::Connection);
}

/// An in-progress database transaction or savepoint.
///
/// A transaction starts with a call to [`Pool::begin`] or [`Connection::begin`].
///
/// A transaction should end with a call to [`commit`] or [`rollback`]. If neither are called
/// before the transaction goes out-of-scope, [`rollback`] is called. In other
/// words, [`rollback`] is called on `drop` if the transaction is still in-progress.
///
/// A savepoint is a special mark inside a transaction that allows all commands that are
/// executed after it was established to be rolled back, restoring the transaction state to
/// what it was at the time of the savepoint.
///
/// [`Connection::begin`]: crate::connection::Connection::begin()
/// [`Pool::begin`]: crate::pool::Pool::begin()
/// [`commit`]: Self::commit()
/// [`rollback`]: Self::rollback()
pub struct Transaction<'c, DB>
where
    DB: Database,
{
    connection: MaybePoolConnection<'c, DB>,
    open: bool,
}

impl<'c, DB> Transaction<'c, DB>
where
    DB: Database,
{
    pub(crate) fn begin(
        conn: impl Into<MaybePoolConnection<'c, DB>>,
    ) -> BoxFuture<'c, Result<Self, Error>> {
        let mut conn = conn.into();

        Box::pin(async move {
            DB::TransactionManager::begin(&mut conn).await?;

            Ok(Self {
                connection: conn,
                open: true,
            })
        })
    }

    /// Commits this transaction or savepoint.
    pub async fn commit(mut self) -> Result<(), Error> {
        DB::TransactionManager::commit(&mut self.connection).await?;
        self.open = false;

        Ok(())
    }

    /// Aborts this transaction or savepoint.
    pub async fn rollback(mut self) -> Result<(), Error> {
        DB::TransactionManager::rollback(&mut self.connection).await?;
        self.open = false;

        Ok(())
    }
}

// NOTE: required due to lack of lazy normalization
#[allow(unused_macros)]
macro_rules! impl_executor_for_transaction {
    ($DB:ident, $Row:ident) => {
        impl<'c, 't> crate::executor::Executor<'t>
            for &'t mut crate::transaction::Transaction<'c, $DB>
        {
            type Database = $DB;

            fn fetch_many<'e, 'q: 'e, E: 'q>(
                self,
                query: E,
            ) -> futures_core::stream::BoxStream<
                'e,
                Result<
                    either::Either<<$DB as crate::database::Database>::QueryResult, $Row>,
                    crate::error::Error,
                >,
            >
            where
                't: 'e,
                E: crate::executor::Execute<'q, Self::Database>,
            {
                (&mut **self).fetch_many(query)
            }

            fn fetch_optional<'e, 'q: 'e, E: 'q>(
                self,
                query: E,
            ) -> futures_core::future::BoxFuture<'e, Result<Option<$Row>, crate::error::Error>>
            where
                't: 'e,
                E: crate::executor::Execute<'q, Self::Database>,
            {
                (&mut **self).fetch_optional(query)
            }

            fn prepare_with<'e, 'q: 'e>(
                self,
                sql: &'q str,
                parameters: &'e [<Self::Database as crate::database::Database>::TypeInfo],
            ) -> futures_core::future::BoxFuture<
                'e,
                Result<
                    <Self::Database as crate::database::HasStatement<'q>>::Statement,
                    crate::error::Error,
                >,
            >
            where
                't: 'e,
            {
                (&mut **self).prepare_with(sql, parameters)
            }

            #[doc(hidden)]
            fn describe<'e, 'q: 'e>(
                self,
                query: &'q str,
            ) -> futures_core::future::BoxFuture<
                'e,
                Result<crate::describe::Describe<Self::Database>, crate::error::Error>,
            >
            where
                't: 'e,
            {
                (&mut **self).describe(query)
            }
        }
    };
}

impl<'c, DB> Debug for Transaction<'c, DB>
where
    DB: Database,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // TODO: Show the full type <..<..<..
        f.debug_struct("Transaction").finish()
    }
}

impl<'c, DB> Deref for Transaction<'c, DB>
where
    DB: Database,
{
    type Target = DB::Connection;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

impl<'c, DB> DerefMut for Transaction<'c, DB>
where
    DB: Database,
{
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.connection
    }
}

impl<'c, DB> Drop for Transaction<'c, DB>
where
    DB: Database,
{
    fn drop(&mut self) {
        if self.open {
            // starts a rollback operation

            // what this does depends on the database but generally this means we queue a rollback
            // operation that will happen on the next asynchronous invocation of the underlying
            // connection (including if the connection is returned to a pool)

            DB::TransactionManager::start_rollback(&mut self.connection);
        }
    }
}

#[allow(dead_code)]
pub(crate) fn begin_ansi_transaction_sql(depth: usize) -> Cow<'static, str> {
    if depth == 0 {
        Cow::Borrowed("BEGIN")
    } else {
        Cow::Owned(format!("SAVEPOINT _sqlx_savepoint_{}", depth))
    }
}

#[allow(dead_code)]
pub(crate) fn commit_ansi_transaction_sql(depth: usize) -> Cow<'static, str> {
    if depth == 1 {
        Cow::Borrowed("COMMIT")
    } else {
        Cow::Owned(format!("RELEASE SAVEPOINT _sqlx_savepoint_{}", depth - 1))
    }
}

#[allow(dead_code)]
pub(crate) fn rollback_ansi_transaction_sql(depth: usize) -> Cow<'static, str> {
    if depth == 1 {
        Cow::Borrowed("ROLLBACK")
    } else {
        Cow::Owned(format!(
            "ROLLBACK TO SAVEPOINT _sqlx_savepoint_{}",
            depth - 1
        ))
    }
}
