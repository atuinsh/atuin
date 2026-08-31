use std::time::Duration;

use async_trait::async_trait;
use atuin_common::db;
use atuin_common::db::MysqlDbUrl;
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
        // Connect eagerly (like the Postgres/SQLite backends) so a misconfigured
        // or unreachable `db_uri` fails fast here with a connection error,
        // instead of a lazy pool deferring the failure to `migrate!`'s
        // `pool.acquire()` and hanging the server at startup. A bounded
        // `acquire_timeout` caps how long sqlx retries an unreachable server so
        // the failure surfaces promptly rather than after the 30s default.
        let pool = MySqlPoolOptions::new()
            .max_connections(100)
            .acquire_timeout(Duration::from_secs(5))
            .connect(url.as_str())
            .await?;

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

        Ok(res.last_insert_id() as i64)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use atuin_common::db::MysqlDbUrl;
    use url::Url;

    use super::MySql;
    use crate::db::Database;

    /// A misconfigured/unreachable MySQL `db_uri` must fail fast with a
    /// connection error instead of hanging the server at startup.
    ///
    /// Regression test for the lazy-vs-eager `connect` divergence: Postgres and
    /// SQLite connect eagerly and error cleanly on a bad URL, but MySQL used
    /// `connect_lazy`, which deferred the failure to `migrate!`'s
    /// `pool.acquire()` — blocking/retrying forever with no log output.
    #[tokio::test]
    async fn connect_to_unreachable_mysql_fails_fast() {
        // Syntactically valid, but nothing is listening on this refused port.
        let url = MysqlDbUrl(Url::parse("mysql://root:pass@127.0.0.1:59999/atuin").unwrap());

        let result = tokio::time::timeout(Duration::from_secs(15), MySql::connect(url)).await;

        match result {
            // Resolved with a connection error — the correct, fail-fast behaviour.
            Ok(Err(_)) => {}
            Ok(Ok(_)) => panic!("unexpectedly connected to an unreachable MySQL server"),
            Err(_elapsed) => {
                panic!("MySql::connect hung: it did not resolve within 15s on a bad db_uri")
            }
        }
    }
}
