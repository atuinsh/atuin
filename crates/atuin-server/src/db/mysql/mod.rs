use async_trait::async_trait;
use atuin_common::db;
use atuin_common::db::MysqlDbUrl;
use easy_cast::Conv;
use sqlx::mysql::MySqlPoolOptions;
use tracing::instrument;

use super::models::NewUser;
use super::{Database, DbError, DbResult, Dialect, PositionalBindingDialect};

#[derive(Clone)]
pub struct MySql {
    pool: sqlx::Pool<sqlx::mysql::MySql>,
}

#[async_trait]
impl Database for MySql {
    type Db = sqlx::mysql::MySql;
    type Url = MysqlDbUrl;
    type Dialect = PositionalBindingDialect;

    fn pool(&self) -> &sqlx::Pool<Self::Db> {
        &self.pool
    }

    async fn connect(url: MysqlDbUrl) -> DbResult<Self> {
        let pool = MySqlPoolOptions::new().max_connections(100).connect(url.as_str()).await?;

        db::migrate!(&pool, "src/db/mysql/migrations")
            .await
            .map_err(|error| DbError::Other(error.into()))?;

        Ok(Self { pool })
    }

    // MySQL has no `RETURNING`, so unlike the default it reads the new id from
    // `last_insert_id()` off the query result.
    #[instrument(skip_all)]
    async fn add_user(&self, user: &NewUser) -> DbResult<i64> {
        let res = db::query(<Self::Dialect as Dialect>::ADD_USER)
            .bind(user.username.as_str())
            .bind(user.email.as_str())
            .bind(user.password.as_str())
            .execute(self.pool())
            .await?;

        Ok(i64::conv(res.last_insert_id()))
    }
}
