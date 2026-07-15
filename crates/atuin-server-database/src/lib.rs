#![forbid(unsafe_code)]

pub mod models;

use std::fmt::Debug;

use self::models::{NewSession, NewUser, Session, User};
use async_trait::async_trait;
use atuin_common::record::{EncryptedData, HostId, Record, RecordIdx, RecordStatus};
use serde::{Deserialize, Serialize};

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
            sqlx::Error::RowNotFound => DbError::NotFound,
            error => DbError::Other(error.into()),
        }
    }
}

pub type DbResult<T> = Result<T, DbError>;

#[derive(Debug, PartialEq)]
pub enum DbType {
    Postgres,
    Sqlite,
    Unknown,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct DbSettings {
    pub db_uri: String,
    /// Optional URI for read replicas. If set, read-only queries will use this connection.
    pub read_db_uri: Option<String>,
}

impl DbSettings {
    pub fn db_type(&self) -> DbType {
        if self.db_uri.starts_with("postgres://") || self.db_uri.starts_with("postgresql://") {
            DbType::Postgres
        } else if self.db_uri.starts_with("sqlite:") {
            DbType::Sqlite
        } else {
            DbType::Unknown
        }
    }
}

fn redact_db_uri(uri: &str) -> String {
    url::Url::parse(uri)
        .map(|mut url| {
            let _ = url.set_password(Some("****"));
            url.to_string()
        })
        .unwrap_or_else(|_| uri.to_string())
}

// Do our best to redact passwords so they're not logged in the event of an error.
impl Debug for DbSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.db_type() == DbType::Postgres {
            let redacted_uri = redact_db_uri(&self.db_uri);
            let redacted_read_uri = self.read_db_uri.as_ref().map(|uri| redact_db_uri(uri));
            f.debug_struct("DbSettings")
                .field("db_uri", &redacted_uri)
                .field("read_db_uri", &redacted_read_uri)
                .finish()
        } else {
            f.debug_struct("DbSettings")
                .field("db_uri", &self.db_uri)
                .field("read_db_uri", &self.read_db_uri)
                .finish()
        }
    }
}

#[async_trait]
pub trait Database: Sized + Clone + Send + Sync + 'static {
    async fn new(settings: &DbSettings) -> DbResult<Self>;

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
        host: HostId,
        tag: String,
        start: Option<RecordIdx>,
        count: u64,
    ) -> DbResult<Vec<Record<EncryptedData>>>;

    // Return the tail record ID for each store, so (HostID, Tag, TailRecordID)
    async fn status(&self, user: &User) -> DbResult<RecordStatus>;
}
