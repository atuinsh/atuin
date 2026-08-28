use async_trait::async_trait;
use atuin_common::db;
use atuin_common::db::MysqlDbUrl;
use sea_orm::{DatabaseConnection, SqlxMySqlConnector};
use sqlx::mysql::MySqlPoolOptions;

use super::{Database, DbError, DbResult};

#[derive(Clone)]
pub struct MySql {
    /// sea-orm connection over the sqlx pool.
    conn: DatabaseConnection,
}

#[async_trait]
impl Database for MySql {
    type Url = MysqlDbUrl;

    async fn connect(url: MysqlDbUrl) -> DbResult<Self> {
        let pool = MySqlPoolOptions::new().max_connections(100).connect_lazy(url.as_str())?;

        db::migrate!(&pool, "src/db/mysql/migrations")
            .await
            .map_err(|error| DbError::Other(error.into()))?;

        Ok(Self {
            conn: SqlxMySqlConnector::from_sqlx_mysql_pool(pool),
        })
    }

    fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }
}
