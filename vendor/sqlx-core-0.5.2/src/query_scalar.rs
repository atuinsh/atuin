use either::Either;
use futures_core::stream::BoxStream;
use futures_util::{StreamExt, TryFutureExt, TryStreamExt};

use crate::arguments::IntoArguments;
use crate::database::{Database, HasArguments, HasStatement};
use crate::encode::Encode;
use crate::error::Error;
use crate::executor::{Execute, Executor};
use crate::from_row::FromRow;
use crate::query_as::{
    query_as, query_as_with, query_statement_as, query_statement_as_with, QueryAs,
};
use crate::types::Type;

/// Raw SQL query with bind parameters, mapped to a concrete type using [`FromRow`] on `(O,)`.
/// Returned from [`query_scalar`].
#[must_use = "query must be executed to affect database"]
pub struct QueryScalar<'q, DB: Database, O, A> {
    inner: QueryAs<'q, DB, (O,), A>,
}

impl<'q, DB: Database, O: Send, A: Send> Execute<'q, DB> for QueryScalar<'q, DB, O, A>
where
    A: 'q + IntoArguments<'q, DB>,
{
    #[inline]
    fn sql(&self) -> &'q str {
        self.inner.sql()
    }

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

impl<'q, DB: Database, O> QueryScalar<'q, DB, O, <DB as HasArguments<'q>>::Arguments> {
    /// Bind a value for use with this SQL query.
    ///
    /// See [`Query::bind`](crate::query::Query::bind).
    pub fn bind<T: 'q + Send + Encode<'q, DB> + Type<DB>>(mut self, value: T) -> Self {
        self.inner = self.inner.bind(value);
        self
    }
}

// FIXME: This is very close, nearly 1:1 with `Map`
// noinspection DuplicatedCode
impl<'q, DB, O, A> QueryScalar<'q, DB, O, A>
where
    DB: Database,
    O: Send + Unpin,
    A: 'q + IntoArguments<'q, DB>,
    (O,): Send + Unpin + for<'r> FromRow<'r, DB::Row>,
{
    /// Execute the query and return the generated results as a stream.
    #[inline]
    pub fn fetch<'e, 'c: 'e, E>(self, executor: E) -> BoxStream<'e, Result<O, Error>>
    where
        'q: 'e,
        E: 'e + Executor<'c, Database = DB>,
        DB: 'e,
        A: 'e,
        O: 'e,
    {
        self.inner.fetch(executor).map_ok(|it| it.0).boxed()
    }

    /// Execute multiple queries and return the generated results as a stream
    /// from each query, in a stream.
    #[inline]
    pub fn fetch_many<'e, 'c: 'e, E>(
        self,
        executor: E,
    ) -> BoxStream<'e, Result<Either<DB::QueryResult, O>, Error>>
    where
        'q: 'e,
        E: 'e + Executor<'c, Database = DB>,
        DB: 'e,
        A: 'e,
        O: 'e,
    {
        self.inner
            .fetch_many(executor)
            .map_ok(|v| v.map_right(|it| it.0))
            .boxed()
    }

    /// Execute the query and return all the generated results, collected into a [`Vec`].
    #[inline]
    pub async fn fetch_all<'e, 'c: 'e, E>(self, executor: E) -> Result<Vec<O>, Error>
    where
        'q: 'e,
        E: 'e + Executor<'c, Database = DB>,
        DB: 'e,
        (O,): 'e,
        A: 'e,
    {
        self.inner
            .fetch(executor)
            .map_ok(|it| it.0)
            .try_collect()
            .await
    }

    /// Execute the query and returns exactly one row.
    #[inline]
    pub async fn fetch_one<'e, 'c: 'e, E>(self, executor: E) -> Result<O, Error>
    where
        'q: 'e,
        E: 'e + Executor<'c, Database = DB>,
        DB: 'e,
        O: 'e,
        A: 'e,
    {
        self.inner.fetch_one(executor).map_ok(|it| it.0).await
    }

    /// Execute the query and returns at most one row.
    #[inline]
    pub async fn fetch_optional<'e, 'c: 'e, E>(self, executor: E) -> Result<Option<O>, Error>
    where
        'q: 'e,
        E: 'e + Executor<'c, Database = DB>,
        DB: 'e,
        O: 'e,
        A: 'e,
    {
        Ok(self.inner.fetch_optional(executor).await?.map(|it| it.0))
    }
}

/// Make a SQL query that is mapped to a single concrete type
/// using [`FromRow`].
#[inline]
pub fn query_scalar<'q, DB, O>(
    sql: &'q str,
) -> QueryScalar<'q, DB, O, <DB as HasArguments<'q>>::Arguments>
where
    DB: Database,
    (O,): for<'r> FromRow<'r, DB::Row>,
{
    QueryScalar {
        inner: query_as(sql),
    }
}

/// Make a SQL query, with the given arguments, that is mapped to a single concrete type
/// using [`FromRow`].
#[inline]
pub fn query_scalar_with<'q, DB, O, A>(sql: &'q str, arguments: A) -> QueryScalar<'q, DB, O, A>
where
    DB: Database,
    A: IntoArguments<'q, DB>,
    (O,): for<'r> FromRow<'r, DB::Row>,
{
    QueryScalar {
        inner: query_as_with(sql, arguments),
    }
}

// Make a SQL query from a statement, that is mapped to a concrete value.
pub(crate) fn query_statement_scalar<'q, DB, O>(
    statement: &'q <DB as HasStatement<'q>>::Statement,
) -> QueryScalar<'q, DB, O, <DB as HasArguments<'_>>::Arguments>
where
    DB: Database,
    (O,): for<'r> FromRow<'r, DB::Row>,
{
    QueryScalar {
        inner: query_statement_as(statement),
    }
}

// Make a SQL query from a statement, with the given arguments, that is mapped to a concrete value.
pub(crate) fn query_statement_scalar_with<'q, DB, O, A>(
    statement: &'q <DB as HasStatement<'q>>::Statement,
    arguments: A,
) -> QueryScalar<'q, DB, O, A>
where
    DB: Database,
    A: IntoArguments<'q, DB>,
    (O,): for<'r> FromRow<'r, DB::Row>,
{
    QueryScalar {
        inner: query_statement_as_with(statement, arguments),
    }
}
