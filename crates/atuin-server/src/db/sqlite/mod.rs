use std::str::FromStr;

use async_trait::async_trait;
use atuin_common::db;
use atuin_common::db::SqliteDbUrl;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use super::shared::SqlxBackend;
use super::{ConnectableDatabase, DbError, DbResult};

#[derive(Clone)]
pub struct Sqlite {
    pool: sqlx::Pool<sqlx::sqlite::Sqlite>,
}

#[async_trait]
impl ConnectableDatabase for Sqlite {
    type Url = SqliteDbUrl;

    async fn connect(url: SqliteDbUrl) -> DbResult<Self> {
        let opts = SqliteConnectOptions::from_str(url.as_str())?
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new().connect_with(opts).await?;

        db::migrate!(&pool, "src/db/sqlite/migrations")
            .await
            .map_err(|error| DbError::Other(error.into()))?;

        Ok(Self { pool })
    }
}

impl SqlxBackend for Sqlite {
    type Db = sqlx::sqlite::Sqlite;

    fn pool(&self) -> &sqlx::Pool<Self::Db> {
        &self.pool
    }

    // SQLite clears no tables in `delete_user` beyond the common set.
}
