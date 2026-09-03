//! Database accessors for `atuin-server`.
//!
//! This module is designed around a couple interesting traits.
//!
//! Firstly, there is a [`DynDatabase`] trait, which is an object-safe trait defining a generic
//! database that `atuin-server` uses. It is a shim around `Database`, which is implemented by
//! [`Postgres`], [`MySql`] and [`Sqlite`].
//!
//! The `Database` trait is the main one conforming backends need to implement. Fortunately, since
//! SQL is mostly standardized, it provides sensible default implementations on all the methods.
//!
//! Unfortunately, _binding_ for SQL is not standardized. There are two dialects:
//!
//!   - Ordinal bound -- `SELECT * FROM table WHERE column = $1`
//!   - Positional bound -- `SELECT * FROM table WHERE column = ?`
//!
//! [`Sqlite`] and [`Postgres`] are both ordinal-bound, while [`MySql`] is positional.
//!
//! To avoid duplicating code, these two dialects of SQL are generalized within the [`Dialect`]
//! trait, which merely collects all the SQL statements as strings. The two implementations
//! [`OrdinalBindingDialect`] and [`PositionalBindingDialect`] implement the different behaviors.
//!
//! With that all said, here's a minimal new backend, should you wish to implement one:
//!
//! ```ignore
//! //! Note that this example will not run since sqlx does not support mssql. We're pretending it
//! //! does.
//!
//! // You probably want to define this in atuin-common. Search for MysqlDbUrl.
//! #[derive(Clone, PartialEq, Eq)]
//! pub struct MsSqlDbUrl<T: Borrow<str> = String>(pub T);
//!
//! // Here's the new type.
//! #[derive(Clone)]
//! pub struct MsSql;
//!
//! struct MsSqlDialect;
//!
//! impl Dialect for MsSqlDialect {
//!   const GET_SESSION: &'static str = "SELECT id, user_id, token FROM sessions WHERE token = @P1";
//!   // repeated for all the other necessary sql statements.
//! }
//!
//! #[async_trait]
//! impl Database for MsSql {
//!     // v-- The sqlx database to use
//!     type Db = sqlx::mssql::MsSql;
//!     // v-- A custom type for the database url. Might not wrap a Url, so we create new types in
//!     //     atuin-common.
//!     type Url = MsSqlDbUrl;
//!
//!     // mssql has its own dialect for bindings, so we created the new type.
//!     type Dialect = MsSqlDialect;
//! }
//!
//! ```
pub mod models;
mod postgres;
pub use postgres::Postgres;
mod sqlite;
use async_trait::async_trait;
use atuin_common::db::OwnedDbUrl;
use atuin_domain::record::{EncryptedData, Record, RecordIdx, RecordSeriesKey, RecordStatus};
use easy_cast::Conv;
use serde::{Deserialize, Serialize};
pub use sqlite::Sqlite;
mod mysql;
use atuin_common::db;
pub use mysql::MySql;
use sqlx::{Encode, Executor, FromRow, IntoArguments, Type};
use tracing::instrument;
use uuid::Uuid;

use self::models::{NewSession, NewUser, Session, User};
use crate::db::models::{DbRecord, RecordSeriesPoint};

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From)]
#[display("{self:?}")]
pub enum DbError {
    #[from(skip)]
    NotFound,
    #[from(time::error::ComponentRange, time::error::Error)]
    Other(eyre::Report),
}

impl From<sqlx::Error> for DbError {
    fn from(error: sqlx::Error) -> Self {
        match error {
            sqlx::Error::RowNotFound => Self::NotFound,
            error => Self::Other(error.into()),
        }
    }
}

pub type DbResult<T> = Result<T, DbError>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DbSettings {
    pub db_uri: OwnedDbUrl,
}

/// The SQL a backend runs, written in its placeholder dialect.
///
/// Please read the module docs.
pub trait Dialect {
    const GET_SESSION: &'static str;
    const GET_SESSION_USER: &'static str;
    const ADD_SESSION: &'static str;
    const GET_USER: &'static str;
    const GET_USER_SESSION: &'static str;
    const ADD_USER: &'static str;
    const UPDATE_USER_PASSWORD: &'static str;
    const DELETE_SESSIONS_BY_USER: &'static str;
    const DELETE_HISTORY_BY_USER: &'static str;
    const DELETE_STORE_BY_USER: &'static str;
    const DELETE_USER_BY_ID: &'static str;
    const ADD_RECORDS: &'static str;
    const NEXT_RECORDS: &'static str;
    const STATUS: &'static str;
}

/// The `SELECT * FROM table WHERE column = $1` dialect. See [`Dialect`] and
/// [`PositionalBindingDialect`].
pub struct OrdinalBindingDialect;

impl Dialect for OrdinalBindingDialect {
    const GET_SESSION: &'static str = "SELECT id, user_id, token FROM sessions WHERE token = $1";
    const GET_SESSION_USER: &'static str = "SELECT users.id, users.username, users.email, \
                                            users.password FROM users
        INNER JOIN sessions ON users.id = sessions.user_id AND sessions.token = $1";
    const ADD_SESSION: &'static str = "INSERT INTO sessions (user_id, token) VALUES ($1, $2)";
    const GET_USER: &'static str =
        "SELECT id, username, email, password FROM users WHERE username = $1";
    const GET_USER_SESSION: &'static str =
        "SELECT id, user_id, token FROM sessions WHERE user_id = $1";
    const ADD_USER: &'static str =
        "INSERT INTO users (username, email, password) VALUES ($1, $2, $3) RETURNING id";
    const UPDATE_USER_PASSWORD: &'static str = "UPDATE users SET password = $1 WHERE id = $2";
    const DELETE_SESSIONS_BY_USER: &'static str = "DELETE FROM sessions WHERE user_id = $1";
    const DELETE_HISTORY_BY_USER: &'static str = "DELETE FROM history WHERE user_id = $1";
    const DELETE_STORE_BY_USER: &'static str = "DELETE FROM store WHERE user_id = $1";
    const DELETE_USER_BY_ID: &'static str = "DELETE FROM users WHERE id = $1";
    const ADD_RECORDS: &'static str = "INSERT INTO store (id, client_id, host, idx, timestamp, \
                                       version, tag, data, cek, user_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT DO NOTHING";
    const NEXT_RECORDS: &'static str = "SELECT client_id, host, idx, timestamp, version, tag, \
                                        data, cek FROM store
        WHERE user_id = $1 AND tag = $2 AND host = $3 AND idx >= $4 ORDER BY idx ASC LIMIT $5";
    const STATUS: &'static str =
        "SELECT host, tag, MAX(idx) AS idx FROM store WHERE user_id = $1 GROUP BY host, tag";
}

/// The `SELECT * FROM table WHERE column = ?` dialect. See [`Dialect`] and
/// [`OrdinalBindingDialect`].
pub struct PositionalBindingDialect;

impl Dialect for PositionalBindingDialect {
    const GET_SESSION: &'static str = "SELECT id, user_id, token FROM sessions WHERE token = ?";
    const GET_SESSION_USER: &'static str = "SELECT users.id, users.username, users.email, \
                                            users.password FROM users
        INNER JOIN sessions ON users.id = sessions.user_id AND sessions.token = ?";
    const ADD_SESSION: &'static str = "INSERT INTO sessions (user_id, token) VALUES (?, ?)";
    const GET_USER: &'static str =
        "SELECT id, username, email, password FROM users WHERE username = ?";
    const GET_USER_SESSION: &'static str =
        "SELECT id, user_id, token FROM sessions WHERE user_id = ?";
    const ADD_USER: &'static str = "INSERT INTO users (username, email, password) VALUES (?, ?, ?)";
    const UPDATE_USER_PASSWORD: &'static str = "UPDATE users SET password = ? WHERE id = ?";
    const DELETE_SESSIONS_BY_USER: &'static str = "DELETE FROM sessions WHERE user_id = ?";
    const DELETE_HISTORY_BY_USER: &'static str = "DELETE FROM history WHERE user_id = ?";
    const DELETE_STORE_BY_USER: &'static str = "DELETE FROM store WHERE user_id = ?";
    const DELETE_USER_BY_ID: &'static str = "DELETE FROM users WHERE id = ?";
    const ADD_RECORDS: &'static str = "INSERT INTO store (id, client_id, host, idx, timestamp, \
                                       version, tag, data, cek, user_id)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON DUPLICATE KEY UPDATE id = id";
    const NEXT_RECORDS: &'static str = "SELECT client_id, host, idx, timestamp, version, tag, \
                                        data, cek FROM store
        WHERE user_id = ? AND tag = ? AND host = ? AND idx >= ? ORDER BY idx ASC LIMIT ?";
    const STATUS: &'static str =
        "SELECT host, tag, MAX(idx) AS idx FROM store WHERE user_id = ? GROUP BY host, tag";
}

/// A database backend, used at runtime behind `Arc<dyn Database>`.
#[async_trait]
pub trait DynDatabase: Send + Sync + 'static {
    async fn get_session(&self, token: &str) -> DbResult<Session>;
    async fn get_session_user(&self, token: &str) -> DbResult<User>;
    async fn add_session(&self, session: &NewSession) -> DbResult<()>;

    async fn get_user(&self, username: &str) -> DbResult<User>;
    async fn get_user_session(&self, u: &User) -> DbResult<Session>;
    async fn add_user(&self, user: &NewUser) -> DbResult<i64>;

    async fn update_user_password(&self, u: &User) -> DbResult<()>;

    async fn delete_user(&self, u: &User) -> DbResult<()>;
    async fn delete_store(&self, user: &User) -> DbResult<()>;

    async fn add_records(&self, user: &User, record: &[Record<EncryptedData>]) -> DbResult<()>;
    async fn next_records(
        &self,
        user: &User,
        series: &RecordSeriesKey,
        start: Option<RecordIdx>,
        count: u64,
    ) -> DbResult<Vec<Record<EncryptedData>>>;

    // Return the tail record ID for each store, so (HostID, Tag, TailRecordID)
    async fn status(&self, user: &User) -> DbResult<RecordStatus>;
}

/// Non object-safe backend for a database that atuin supports.
///
/// If you are adding a new backend, this is what you should implement.
#[async_trait]
pub(crate) trait Database: Sized + Send + Sync + 'static
where
    for<'c> &'c mut <Self::Db as sqlx::Database>::Connection: Executor<'c, Database = Self::Db>,
    <Self::Db as sqlx::Database>::Arguments: IntoArguments<Self::Db>,
    for<'r> Session: FromRow<'r, <Self::Db as sqlx::Database>::Row>,
    for<'r> User: FromRow<'r, <Self::Db as sqlx::Database>::Row>,
    for<'r> DbRecord: FromRow<'r, <Self::Db as sqlx::Database>::Row>,
    for<'r> RecordSeriesPoint: FromRow<'r, <Self::Db as sqlx::Database>::Row>,
    for<'r> (i64,): FromRow<'r, <Self::Db as sqlx::Database>::Row>,
    for<'q> &'q str: Encode<'q, Self::Db> + Type<Self::Db>,
    i64: Type<Self::Db> + for<'q> Encode<'q, Self::Db>,
    Uuid: Type<Self::Db> + for<'q> Encode<'q, Self::Db>,
{
    /// The sqlx backend this database talks to.
    type Db: sqlx::Database;

    /// The backend-specific connection URL this database is built from.
    type Url;

    /// The placeholder dialect this backend's SQL is written in.
    type Dialect: Dialect;

    /// The connection pool queries run against.
    fn pool(&self) -> &sqlx::Pool<Self::Db>;

    async fn connect(url: Self::Url) -> DbResult<Self>;

    #[instrument(skip_all)]
    async fn get_session(&self, token: &str) -> DbResult<Session> {
        db::query_as(Self::Dialect::GET_SESSION)
            .bind(token)
            .fetch_one(self.pool())
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn get_session_user(&self, token: &str) -> DbResult<User> {
        db::query_as(Self::Dialect::GET_SESSION_USER)
            .bind(token)
            .fetch_one(self.pool())
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn add_session(&self, session: &NewSession) -> DbResult<()> {
        db::query(Self::Dialect::ADD_SESSION)
            .bind(session.user_id)
            .bind(session.token.as_str())
            .execute(self.pool())
            .await?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn get_user(&self, username: &str) -> DbResult<User> {
        db::query_as(Self::Dialect::GET_USER)
            .bind(username)
            .fetch_one(self.pool())
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn get_user_session(&self, u: &User) -> DbResult<Session> {
        db::query_as(Self::Dialect::GET_USER_SESSION)
            .bind(u.id)
            .fetch_one(self.pool())
            .await
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn add_user(&self, user: &NewUser) -> DbResult<i64> {
        let res: (i64,) = db::query_as(Self::Dialect::ADD_USER)
            .bind(user.username.as_str())
            .bind(user.email.as_str())
            .bind(user.password.as_str())
            .fetch_one(self.pool())
            .await?;
        Ok(res.0)
    }

    #[instrument(skip_all)]
    async fn update_user_password(&self, user: &User) -> DbResult<()> {
        db::query(Self::Dialect::UPDATE_USER_PASSWORD)
            .bind(user.password.as_str())
            .bind(user.id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn delete_user(&self, u: &User) -> DbResult<()> {
        db::query(Self::Dialect::DELETE_SESSIONS_BY_USER).bind(u.id).execute(self.pool()).await?;
        db::query(Self::Dialect::DELETE_HISTORY_BY_USER).bind(u.id).execute(self.pool()).await?;
        db::query(Self::Dialect::DELETE_STORE_BY_USER).bind(u.id).execute(self.pool()).await?;
        db::query(Self::Dialect::DELETE_USER_BY_ID).bind(u.id).execute(self.pool()).await?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn delete_store(&self, user: &User) -> DbResult<()> {
        db::query(Self::Dialect::DELETE_STORE_BY_USER).bind(user.id).execute(self.pool()).await?;
        Ok(())
    }

    #[instrument(skip_all)]
    async fn add_records(&self, user: &User, records: &[Record<EncryptedData>]) -> DbResult<()> {
        let mut tx = self.pool().begin().await?;

        for i in records {
            let id = atuin_common::utils::uuid_v7();

            db::query(Self::Dialect::ADD_RECORDS)
                .bind(id)
                .bind(i.id)
                .bind(i.host.id)
                .bind(i64::conv(i.idx))
                // throwing away some data, but i64 is still big in terms of time
                .bind(i64::conv(i.timestamp))
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

        db::query_as::<_, DbRecord>(Self::Dialect::NEXT_RECORDS)
            .bind(user.id)
            .bind(series.tag.as_str())
            .bind(series.host_id)
            .bind(i64::conv(start))
            .bind(i64::conv(count))
            .fetch_all(self.pool())
            .await
            .map(|records| records.into_iter().map(Into::into).collect())
            .map_err(Into::into)
    }

    #[instrument(skip_all)]
    async fn status(&self, user: &User) -> DbResult<RecordStatus> {
        let points = db::query_as::<_, RecordSeriesPoint>(Self::Dialect::STATUS)
            .bind(user.id)
            .fetch_all(self.pool())
            .await?;
        Ok(RecordStatus::from_points(points.into_iter().map(Into::into)))
    }
}

/// The object-safe version of `Database`. Useful for dependency-injection.
#[async_trait]
impl<T> DynDatabase for T
where
    T: Database,
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
    async fn get_session(&self, token: &str) -> DbResult<Session> {
        Database::get_session(self, token).await
    }

    async fn get_session_user(&self, token: &str) -> DbResult<User> {
        Database::get_session_user(self, token).await
    }

    async fn add_session(&self, session: &NewSession) -> DbResult<()> {
        Database::add_session(self, session).await
    }

    async fn get_user(&self, username: &str) -> DbResult<User> {
        Database::get_user(self, username).await
    }

    async fn get_user_session(&self, u: &User) -> DbResult<Session> {
        Database::get_user_session(self, u).await
    }

    async fn add_user(&self, user: &NewUser) -> DbResult<i64> {
        Database::add_user(self, user).await
    }

    async fn update_user_password(&self, u: &User) -> DbResult<()> {
        Database::update_user_password(self, u).await
    }

    async fn delete_user(&self, u: &User) -> DbResult<()> {
        Database::delete_user(self, u).await
    }

    async fn delete_store(&self, user: &User) -> DbResult<()> {
        Database::delete_store(self, user).await
    }

    async fn add_records(&self, user: &User, records: &[Record<EncryptedData>]) -> DbResult<()> {
        Database::add_records(self, user, records).await
    }

    async fn next_records(
        &self,
        user: &User,
        series: &RecordSeriesKey,
        start: Option<RecordIdx>,
        count: u64,
    ) -> DbResult<Vec<Record<EncryptedData>>> {
        Database::next_records(self, user, series, start, count).await
    }

    async fn status(&self, user: &User) -> DbResult<RecordStatus> {
        Database::status(self, user).await
    }
}
