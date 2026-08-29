//! Shared [`Database`] implementation for the `$1`-dialect backends.
//!
//! SQLite and Postgres speak the same `$1` placeholder dialect and run
//! byte-for-byte identical SQL. Rather than keep two copies in sync, each
//! implements the private [`SqlxBackend`] trait — just a pool accessor — and
//! picks up a full [`Database`] impl for free from the blanket impl below, where
//! the shared query bodies live.
//!
//! The blanket impl is generic over the backend, so it carries the (verbose but
//! one-off) sqlx trait bounds needed to run a query against an arbitrary
//! `sqlx::Database`. Those bounds stay contained here: handlers talk to
//! `Arc<dyn Database>`, never to a `T: Database`, so they never see them.
//!
//! MySQL is deliberately *not* built on this: it needs `?`-style placeholders
//! and diverges in `add_user` (`last_insert_id()` rather than `RETURNING`) and
//! `add_records` (`on duplicate key update`), so it implements [`Database`] by
//! hand and does not implement [`SqlxBackend`].

use async_trait::async_trait;
use atuin_common::db;
use atuin_domain::record::{EncryptedData, Record, RecordIdx, RecordSeriesKey, RecordStatus};
use sqlx::{Encode, Executor, FromRow, IntoArguments, Type};
use tracing::instrument;
use uuid::Uuid;

use super::models::{DbRecord, NewSession, NewUser, RecordSeriesPoint, Session, User};
use super::{Database, DbResult};

/// A concrete sqlx-backed database whose queries use `$1`-style placeholders.
///
/// Implementing this is all a `$1`-dialect backend needs: the blanket
/// `impl<T: SqlxBackend> Database for T` below supplies every query method.
pub trait SqlxBackend {
    /// The sqlx backend this database talks to.
    type Db: sqlx::Database;

    /// The connection pool the shared queries run against.
    fn pool(&self) -> &sqlx::Pool<Self::Db>;
}

#[async_trait]
impl<T> Database for T
where
    T: SqlxBackend + Send + Sync + 'static,
    for<'c> &'c mut <T::Db as sqlx::Database>::Connection: Executor<'c, Database = T::Db>,
    <T::Db as sqlx::Database>::Arguments: IntoArguments<T::Db>,
    for<'r> Session: FromRow<'r, <T::Db as sqlx::Database>::Row>,
    for<'r> User: FromRow<'r, <T::Db as sqlx::Database>::Row>,
    for<'r> DbRecord: FromRow<'r, <T::Db as sqlx::Database>::Row>,
    for<'r> RecordSeriesPoint: FromRow<'r, <T::Db as sqlx::Database>::Row>,
    for<'r> (i64,): FromRow<'r, <T::Db as sqlx::Database>::Row>,
    for<'q> &'q str: Encode<'q, T::Db> + Type<T::Db>,
    i64: Type<T::Db> + for<'q> Encode<'q, T::Db>,
    Uuid: Type<T::Db> + for<'q> Encode<'q, T::Db>,
{
    #[instrument(skip_all)]
    async fn get_session(&self, token: &str) -> DbResult<Session> {
        db::query_as("select id, user_id, token from sessions where token = $1")
            .bind(token)
            .fetch_one(self.pool())
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn get_session_user(&self, token: &str) -> DbResult<User> {
        db::query_as(
            "select users.id, users.username, users.email, users.password from users
            inner join sessions
            on users.id = sessions.user_id
            and sessions.token = $1",
        )
        .bind(token)
        .fetch_one(self.pool())
        .await
        .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn add_session(&self, session: &NewSession) -> DbResult<()> {
        let token: &str = &session.token;

        db::query(
            "insert into sessions
                (user_id, token)
            values($1, $2)",
        )
        .bind(session.user_id)
        .bind(token)
        .execute(self.pool())
        .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn get_user(&self, username: &str) -> DbResult<User> {
        db::query_as("select id, username, email, password from users where username = $1")
            .bind(username)
            .fetch_one(self.pool())
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn get_user_session(&self, u: &User) -> DbResult<Session> {
        db::query_as("select id, user_id, token from sessions where user_id = $1")
            .bind(u.id)
            .fetch_one(self.pool())
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn add_user(&self, user: &NewUser) -> DbResult<i64> {
        let email: &str = &user.email;
        let username: &str = &user.username;
        let password: &str = &user.password;

        let res: (i64,) = db::query_as(
            "insert into users
                (username, email, password)
            values($1, $2, $3)
            returning id",
        )
        .bind(username)
        .bind(email)
        .bind(password)
        .fetch_one(self.pool())
        .await?;

        Ok(res.0)
    }

    #[instrument(skip_all)]
    async fn update_user_password(&self, user: &User) -> DbResult<()> {
        db::query(
            "update users
            set password = $1
            where id = $2",
        )
        .bind(user.password.as_str())
        .bind(user.id)
        .execute(self.pool())
        .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn delete_user(&self, u: &User) -> DbResult<()> {
        db::query("delete from sessions where user_id = $1")
            .bind(u.id)
            .execute(self.pool())
            .await?;

        db::query("delete from history where user_id = $1")
            .bind(u.id)
            .execute(self.pool())
            .await?;

        db::query("delete from store where user_id = $1")
            .bind(u.id)
            .execute(self.pool())
            .await?;

        db::query("delete from users where id = $1")
            .bind(u.id)
            .execute(self.pool())
            .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn delete_store(&self, user: &User) -> DbResult<()> {
        db::query("delete from store where user_id = $1")
            .bind(user.id)
            .execute(self.pool())
            .await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_records(&self, user: &User, records: &[Record<EncryptedData>]) -> DbResult<()> {
        let mut tx = self.pool().begin().await?;

        for i in records {
            let id = atuin_common::utils::uuid_v7();

            db::query(
                "insert into store
                    (id, client_id, host, idx, timestamp, version, tag, data, cek, user_id)
                values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                on conflict do nothing
                ",
            )
            .bind(id)
            .bind(i.id)
            .bind(i.host.id)
            .bind(i.idx as i64)
            .bind(i.timestamp as i64) // throwing away some data, but i64 is still big in terms of time
            .bind(i.version.as_str())
            .bind(i.tag.as_str())
            .bind(i.data.raw.as_str())
            .bind(i.data.cek.as_str())
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    #[instrument(skip_all)]
    async fn next_records(
        &self,
        user: &User,
        series: &RecordSeriesKey,
        start: Option<RecordIdx>,
        count: u64,
    ) -> DbResult<Vec<Record<EncryptedData>>> {
        tracing::debug!("{:?} - {:?} - {:?}", series.host_id, series.tag, start);
        let start = start.unwrap_or(0);

        db::query_as::<_, DbRecord>(
            "select client_id, host, idx, timestamp, version, tag, data, cek from store
                    where user_id = $1
                    and tag = $2
                    and host = $3
                    and idx >= $4
                    order by idx asc
                    limit $5",
        )
        .bind(user.id)
        .bind(series.tag.as_str())
        .bind(series.host_id)
        .bind(start as i64)
        .bind(count as i64)
        .fetch_all(self.pool())
        .await
        .map(|records| records.into_iter().map(Into::into).collect())
        .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn status(&self, user: &User) -> DbResult<RecordStatus> {
        const STATUS_SQL: &str =
            "select host, tag, max(idx) as idx from store where user_id = $1 group by host, tag";

        let points = db::query_as::<_, RecordSeriesPoint>(STATUS_SQL)
            .bind(user.id)
            .fetch_all(self.pool())
            .await?;
        Ok(RecordStatus::from_points(points.into_iter().map(Into::into)))
    }
}
