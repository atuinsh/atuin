use std::str::FromStr;

use async_trait::async_trait;
use atuin_common::record::{EncryptedData, HostId, Record, RecordIdx, RecordStatus};
use atuin_server_database::{
    Database, DbError, DbResult, DbSettings,
    models::{History, NewHistory, NewSession, NewUser, Session, User},
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use tracing::{instrument, trace};
use wrappers::{DbHistory, DbRecord, DbSession, DbUser};

mod wrappers;

#[derive(Clone)]
pub struct Sqlite {
    pool: sqlx::Pool<sqlx::sqlite::Sqlite>,
}

fn fix_error(error: sqlx::Error) -> DbError {
    match error {
        sqlx::Error::RowNotFound => DbError::NotFound,
        error => DbError::Other(error.into()),
    }
}

#[async_trait]
impl Database for Sqlite {
    async fn new(settings: &DbSettings) -> DbResult<Self> {
        let opts = SqliteConnectOptions::from_str(&settings.db_uri)
            .map_err(fix_error)?
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .connect_with(opts)
            .await
            .map_err(fix_error)?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|error| DbError::Other(error.into()))?;

        Ok(Self { pool })
    }

    #[instrument(skip_all)]
    async fn get_session(&self, token: &str) -> DbResult<Session> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn get_session_user(&self, token: &str) -> DbResult<User> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn add_session(&self, session: &NewSession) -> DbResult<()> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn get_user(&self, username: &str) -> DbResult<User> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn get_user_session(&self, u: &User) -> DbResult<Session> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn add_user(&self, user: &NewUser) -> DbResult<i64> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn user_verified(&self, id: i64) -> DbResult<bool> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn verify_user(&self, id: i64) -> DbResult<()> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn user_verification_token(&self, id: i64) -> DbResult<String> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn update_user_password(&self, u: &User) -> DbResult<()> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn total_history(&self) -> DbResult<i64> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn count_history(&self, user: &User) -> DbResult<i64> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn count_history_cached(&self, user: &User) -> DbResult<i64> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn delete_user(&self, u: &User) -> DbResult<()> {
        todo!()
    }

    async fn delete_history(&self, user: &User, id: String) -> DbResult<()> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn deleted_history(&self, user: &User) -> DbResult<Vec<String>> {
        todo!()
    }

    async fn delete_store(&self, user: &User) -> DbResult<()> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn add_records(&self, user: &User, records: &[Record<EncryptedData>]) -> DbResult<()> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn next_records(
        &self,
        user: &User,
        host: HostId,
        tag: String,
        start: Option<RecordIdx>,
        count: u64,
    ) -> DbResult<Vec<Record<EncryptedData>>> {
        todo!()
    }

    async fn status(&self, user: &User) -> DbResult<RecordStatus> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn count_history_range(
        &self,
        user: &User,
        range: std::ops::Range<time::OffsetDateTime>,
    ) -> DbResult<i64> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn list_history(
        &self,
        user: &User,
        created_after: time::OffsetDateTime,
        since: time::OffsetDateTime,
        host: &str,
        page_size: i64,
    ) -> DbResult<Vec<History>> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn add_history(&self, history: &[NewHistory]) -> DbResult<()> {
        todo!()
    }

    #[instrument(skip_all)]
    async fn oldest_history(&self, user: &User) -> DbResult<History> {
        todo!()
    }
}
