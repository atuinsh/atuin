use async_trait::async_trait;
use atuin_common::db;
use atuin_common::db::PostgresDbUrl;
use sqlx::postgres::PgPoolOptions;

use super::{Database, DbError, DbResult, OrdinalBindingDialect};

const MIN_PG_VERSION: u32 = 14;

#[derive(Clone)]
pub struct Postgres {
    pool: sqlx::Pool<sqlx::postgres::Postgres>,
}

#[async_trait]
impl Database for Postgres {
    type Db = sqlx::postgres::Postgres;
    type Url = PostgresDbUrl;
    type Dialect = OrdinalBindingDialect;

    fn pool(&self) -> &sqlx::Pool<Self::Db> {
        &self.pool
    }

    async fn connect(url: PostgresDbUrl) -> DbResult<Self> {
        let pool = PgPoolOptions::new().max_connections(100).connect(url.as_str()).await?;

        // Call server_version_num to get the DB server's major version number
        // The call returns None for servers older than 8.x.
        let pg_major_version: u32 = pool
            .acquire()
            .await?
            .server_version_num()
            .ok_or(DbError::Other(eyre::Report::msg("could not get PostgreSQL version")))?
            / 10000;

        if pg_major_version < MIN_PG_VERSION {
            return Err(DbError::Other(eyre::Report::msg(format!(
                "unsupported PostgreSQL version {pg_major_version}, minimum required is \
                 {MIN_PG_VERSION}"
            ))));
        }

        db::migrate!(&pool, "src/db/postgres/migrations")
            .await
            .map_err(|error| DbError::Other(error.into()))?;

        Ok(Self { pool })
    }
}
