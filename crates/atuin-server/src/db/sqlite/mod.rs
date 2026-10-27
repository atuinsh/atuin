use std::str::FromStr;

use async_trait::async_trait;
use atuin_common::db;
use atuin_common::db::SqliteDbUrl;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use super::{Database, DbError, DbResult, OrdinalBindingDialect};

#[derive(Clone)]
pub struct Sqlite {
    pool: sqlx::Pool<sqlx::sqlite::Sqlite>,
}

#[async_trait]
impl Database for Sqlite {
    type Db = sqlx::sqlite::Sqlite;
    type Url = SqliteDbUrl;
    type Dialect = OrdinalBindingDialect;

    fn pool(&self) -> &sqlx::Pool<Self::Db> {
        &self.pool
    }

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
