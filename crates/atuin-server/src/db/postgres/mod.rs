use async_trait::async_trait;
use atuin_common::db;
use atuin_common::db::PostgresDbUrl;
use sea_orm::{DatabaseConnection, SqlxPostgresConnector};
use sqlx::postgres::{PgPool, PgPoolOptions};

use super::{Database, DbError, DbResult};

const MIN_PG_VERSION: u32 = 14;

#[derive(Clone)]
pub struct Postgres {
    /// sea-orm connection over the sqlx pool.
    conn: DatabaseConnection,
}

/// Ensure a pool points at a PostgreSQL new enough for the queries we run.
async fn ensure_supported_version(pool: &PgPool) -> DbResult<()> {
    // Call server_version_num to get the DB server's major version number.
    // The call returns None for servers older than 8.x.
    let major = pool
        .acquire()
        .await?
        .server_version_num()
        .ok_or_else(|| DbError::Other(eyre::Report::msg("could not get PostgreSQL version")))?
        / 10000;

    if major < MIN_PG_VERSION {
        return Err(DbError::Other(eyre::Report::msg(format!(
            "unsupported PostgreSQL version {major}, minimum required is {MIN_PG_VERSION}"
        ))));
    }

    Ok(())
}

#[async_trait]
impl Database for Postgres {
    type Url = PostgresDbUrl;

    async fn connect(url: PostgresDbUrl) -> DbResult<Self> {
        let pool = PgPoolOptions::new().max_connections(100).connect(url.as_str()).await?;
        ensure_supported_version(&pool).await?;

        db::migrate!(&pool, "src/db/postgres/migrations")
            .await
            .map_err(|error| DbError::Other(error.into()))?;

        Ok(Self {
            conn: SqlxPostgresConnector::from_sqlx_postgres_pool(pool),
        })
    }

    fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }
}
