pub mod models;

pub mod postgres;

pub use postgres::Postgres;

pub mod sqlite;

use async_trait::async_trait;
use atuin_common::db::OwnedDbUrl;
use atuin_domain::record::{EncryptedData, Record, RecordIdx, RecordSeriesKey, RecordStatus};
use serde::{Deserialize, Serialize};
pub use sqlite::Sqlite;

pub mod mysql;

pub use mysql::MySql;

use self::models::{NewSession, NewUser, Session, User};

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

// Password redaction lives on `OwnedDbUrl`'s `Debug`, so the derive is safe here.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DbSettings {
    pub db_uri: OwnedDbUrl,
}

/// A database backend, used at runtime behind `Arc<dyn Database>`.
#[async_trait]
pub trait Database: Send + Sync + 'static {
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

/// A [`Database`] backend that can be constructed from a connection URL.
#[async_trait]
pub trait ConnectableDatabase: Database + Sized {
    /// The backend-specific connection URL this database is built from.
    type Url;

    async fn connect(url: Self::Url) -> DbResult<Self>;
}
