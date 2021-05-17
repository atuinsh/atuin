use std::marker::PhantomData;

use either::Either;
use futures_core::stream::BoxStream;
use futures_util::{StreamExt, TryStreamExt};

use crate::arguments::IntoArguments;
use crate::database::{Database, HasArguments, HasStatement};
use crate::encode::Encode;
use crate::error::Error;
use crate::executor::{Execute, Executor};
use crate::from_row::FromRow;
use crate::query::{query, query_statement, query_statement_with, query_with, Query};
use crate::types::Type;

/// Raw SQL query with bind parameters, mapped to a concrete type using [`FromRow`].
/// Returned from [`query_as`].
#[must_use = "query must be executed to affect database"]
pub struct QueryAs<'q, DB: Database, O, A> {
    pub(crate) inner: Query<'q, DB, A>,
    pub(crate) output: PhantomData<O>,
}

impl<'q, DB, O: Send, A: Send> Execute<'q, DB> for QueryAs<'q, DB, O, A>
where
    DB: Database,
    A: 'q + IntoArguments<'q, DB>,
{
    #[inline]
    fn sql(&self) -> &'q str {
        self.inner.sql()
    }

    #[inline]
    fn statement(&self) -> Option<&<DB as HasStatement<'q>>::Statement> {
        self.inner.statement()
    }

    #[inline]
    fn take_arguments(&mut self) -> Option<<DB as HasArguments<'q>>::Arguments> {
        self.inner.take_arguments()
    }

    #[inline]
    fn persistent(&self) -> bool {
        self.inner.persistent()
    }
}

impl<'q, DB: Database, O> QueryAs<'q, DB, O, <DB as HasArguments<'q>>::Arguments> {
    /// Bind a value for use with this SQL query.
    ///
    /// See [`Query::bind`](Query::bind).
    pub fn bind<T: 'q + Send + Encode<'q, DB> + Type<DB>>(mut self, value: T) -> Self {
        self.inner = self.inner.bind(value);
        self
    }
}

// FIXME: This is very close, nearly 1:1 with `Map`
// noinspection DuplicatedCode
impl<'q, DB, O, A> QueryAs<'q, DB, O, A>
where
    DB: Database,
    A: 'q + IntoArguments<'q, DB>,
    O: Send + Unpin + for<'r> FromRow<'r, DB::Row>,
{
    /// Execute the query and return the generated results as a stream.
    pub fn fetch<'e, 'c: 'e, E>(self, executor: E) -> BoxStream<'e, Result<O, Error>>
    where
        'q: 'e,
        E: 'e + Executor<'c, Database = DB>,
        DB: 'e,
        O: 'e,
        A: 'e,
    {
        self.fetch_many(executor)
            .try_filter_map(|step| async move { Ok(step.right()) })
            .boxed()
    }

    /// Execute multiple queries and return the generated results as a stream
    /// from each query, in a stream.
    pub fn fetch_many<'e, 'c: 'e, E>(
        self,
        executor: E,
    ) -> BoxStream<'e, Result<Either<DB::QueryResult, O>, Error>>
    where
        'q: 'e,
        E: 'e + Executor<'c, Database = DB>,
        DB: 'e,
        O: 'e,
        A: 'e,
    {
        Box::pin(try_stream! {
            let mut s = executor.fetch_many(self.inner);

            while let Some(v) = s.try_next().await? {
                r#yield!(match v {
                    Either::Left(v) => Either::Left(v),
                    Either::Right(row) => Either::Right(O::from_row(&row)?),
                });
            }

            Ok(())
        })
    }

    /// Execute the query and return all the generated results, collected into a [`Vec`].
    #[inline]
    pub async fn fetch_all<'e, 'c: 'e, E>(self, executor: E) -> Result<Vec<O>, Error>
    where
        'q: 'e,
        E: 'e + Executor<'c, Database = DB>,
        DB: 'e,
        O: 'e,
        A: 'e,
    {
        self.fetch(executor).try_collect().await
    }

    /// Execute the query and returns exactly one row.
    pub async fn fetch_one<'e, 'c: 'e, E>(self, executor: E) -> Result<O, Error>
    where
        'q: 'e,
        E: 'e + Executor<'c, Database = DB>,
        DB: 'e,
        O: 'e,
        A: 'e,
    {
        self.fetch_optional(executor)
            .await
            .and_then(|row| row.ok_or(Error::RowNotFound))
    }

    /// Execute the query and returns at most one row.
    pub async fn fetch_optional<'e, 'c: 'e, E>(self, executor: E) -> Result<Option<O>, Error>
    where
        'q: 'e,
        E: 'e + Executor<'c, Database = DB>,
        DB: 'e,
        O: 'e,
        A: 'e,
    {
        let row = executor.fetch_optional(self.inner).await?;
        if let Some(row) = row {
            O::from_row(&row).map(Some)
        } else {
            Ok(None)
        }
    }
}

/// Make a SQL query that is mapped to a concrete type
/// using [`FromRow`].
#[inline]
pub fn query_as<'q, DB, O>(sql: &'q str) -> QueryAs<'q, DB, O, <DB as HasArguments<'q>>::Arguments>
where
    DB: Database,
    O: for<'r> FromRow<'r, DB::Row>,
{
    QueryAs {
        inner: query(sql),
        output: PhantomData,
    }
}

/// Make a SQL query, with the given arguments, that is mapped to a concrete type
/// using [`FromRow`].
#[inline]
pub fn query_as_with<'q, DB, O, A>(sql: &'q str, arguments: A) -> QueryAs<'q, DB, O, A>
where
    DB: Database,
    A: IntoArguments<'q, DB>,
    O: for<'r> FromRow<'r, DB::Row>,
{
    QueryAs {
        inner: query_with(sql, arguments),
        output: PhantomData,
    }
}

// Make a SQL query from a statement, that is mapped to a concrete type.
pub(crate) fn query_statement_as<'q, DB, O>(
    statement: &'q <DB as HasStatement<'q>>::Statement,
) -> QueryAs<'q, DB, O, <DB as HasArguments<'_>>::Arguments>
where
    DB: Database,
    O: for<'r> FromRow<'r, DB::Row>,
{
    QueryAs {
        inner: query_statement(statement),
        output: PhantomData,
    }
}

// Make a SQL query from a statement, with the given arguments, that is mapped to a concrete type.
pub(crate) fn query_statement_as_with<'q, DB, O, A>(
    statement: &'q <DB as HasStatement<'q>>::Statement,
    arguments: A,
) -> QueryAs<'q, DB, O, A>
where
    DB: Database,
    A: IntoArguments<'q, DB>,
    O: for<'r> FromRow<'r, DB::Row>,
{
    QueryAs {
        inner: query_statement_with(statement, arguments),
        output: PhantomData,
    }
}
