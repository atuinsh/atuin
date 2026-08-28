use std::str::FromStr;

use async_trait::async_trait;
use atuin_common::db;
use atuin_common::db::SqliteDbUrl;
use sea_orm::{DatabaseConnection, SqlxSqliteConnector};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use super::{Database, DbError, DbResult};

#[derive(Clone)]
pub struct Sqlite {
    /// sea-orm connection over the sqlx pool.
    conn: DatabaseConnection,
}

#[async_trait]
impl Database for Sqlite {
    type Url = SqliteDbUrl;

    async fn connect(url: SqliteDbUrl) -> DbResult<Self> {
        let opts = SqliteConnectOptions::from_str(url.as_str())?
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new().connect_with(opts).await?;

        db::migrate!(&pool, "src/db/sqlite/migrations")
            .await
            .map_err(|error| DbError::Other(error.into()))?;

        Ok(Self {
            conn: SqlxSqliteConnector::from_sqlx_sqlite_pool(pool),
        })
    }

    fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }
}
