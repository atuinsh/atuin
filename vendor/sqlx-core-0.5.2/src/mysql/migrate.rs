use crate::connection::ConnectOptions;
use crate::error::Error;
use crate::executor::Executor;
use crate::migrate::MigrateError;
use crate::migrate::{AppliedMigration, Migration};
use crate::migrate::{Migrate, MigrateDatabase};
use crate::mysql::{MySql, MySqlConnectOptions, MySqlConnection};
use crate::query::query;
use crate::query_as::query_as;
use crate::query_scalar::query_scalar;
use crc::crc32;
use futures_core::future::BoxFuture;
use std::str::FromStr;
use std::time::Duration;
use std::time::Instant;

fn parse_for_maintenance(uri: &str) -> Result<(MySqlConnectOptions, String), Error> {
    let mut options = MySqlConnectOptions::from_str(uri)?;

    let database = if let Some(database) = &options.database {
        database.to_owned()
    } else {
        return Err(Error::Configuration(
            "DATABASE_URL does not specify a database".into(),
        ));
    };

    // switch us to <no> database for create/drop commands
    options.database = None;

    Ok((options, database))
}

impl MigrateDatabase for MySql {
    fn create_database(uri: &str) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let (options, database) = parse_for_maintenance(uri)?;
            let mut conn = options.connect().await?;

            let _ = conn
                .execute(&*format!("CREATE DATABASE `{}`", database))
                .await?;

            Ok(())
        })
    }

    fn database_exists(uri: &str) -> BoxFuture<'_, Result<bool, Error>> {
        Box::pin(async move {
            let (options, database) = parse_for_maintenance(uri)?;
            let mut conn = options.connect().await?;

            let exists: bool = query_scalar(
                "select exists(SELECT 1 from INFORMATION_SCHEMA.SCHEMATA WHERE SCHEMA_NAME = ?)",
            )
            .bind(database)
            .fetch_one(&mut conn)
            .await?;

            Ok(exists)
        })
    }

    fn drop_database(uri: &str) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let (options, database) = parse_for_maintenance(uri)?;
            let mut conn = options.connect().await?;

            let _ = conn
                .execute(&*format!("DROP DATABASE IF EXISTS `{}`", database,))
                .await?;

            Ok(())
        })
    }
}

impl Migrate for MySqlConnection {
    fn ensure_migrations_table(&mut self) -> BoxFuture<'_, Result<(), MigrateError>> {
        Box::pin(async move {
            // language=MySQL
            self.execute(
                r#"
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
);
                "#,
            )
            .await?;

            Ok(())
        })
    }

    fn dirty_version(&mut self) -> BoxFuture<'_, Result<Option<i64>, MigrateError>> {
        Box::pin(async move {
            // language=SQL
            let row: Option<(i64,)> = query_as(
                "SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version LIMIT 1",
            )
            .fetch_optional(self)
            .await?;

            Ok(row.map(|r| r.0))
        })
    }

    fn list_applied_migrations(
        &mut self,
    ) -> BoxFuture<'_, Result<Vec<AppliedMigration>, MigrateError>> {
        Box::pin(async move {
            // language=SQL
            let rows: Vec<(i64, Vec<u8>)> =
                query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
                    .fetch_all(self)
                    .await?;

            let migrations = rows
                .into_iter()
                .map(|(version, checksum)| AppliedMigration {
                    version,
                    checksum: checksum.into(),
                })
                .collect();

            Ok(migrations)
        })
    }

    fn lock(&mut self) -> BoxFuture<'_, Result<(), MigrateError>> {
        Box::pin(async move {
            let database_name = current_database(self).await?;
            let lock_id = generate_lock_id(&database_name);

            // create an application lock over the database
            // this function will not return until the lock is acquired

            // https://www.postgresql.org/docs/current/explicit-locking.html#ADVISORY-LOCKS
            // https://www.postgresql.org/docs/current/functions-admin.html#FUNCTIONS-ADVISORY-LOCKS-TABLE

            // language=MySQL
            let _ = query("SELECT GET_LOCK(?, -1)")
                .bind(lock_id)
                .execute(self)
                .await?;

            Ok(())
        })
    }

    fn unlock(&mut self) -> BoxFuture<'_, Result<(), MigrateError>> {
        Box::pin(async move {
            let database_name = current_database(self).await?;
            let lock_id = generate_lock_id(&database_name);

            // language=MySQL
            let _ = query("SELECT RELEASE_LOCK(?)")
                .bind(lock_id)
                .execute(self)
                .await?;

            Ok(())
        })
    }

    fn apply<'e: 'm, 'm>(
        &'e mut self,
        migration: &'m Migration,
    ) -> BoxFuture<'m, Result<Duration, MigrateError>> {
        Box::pin(async move {
            let start = Instant::now();

            let res = self.execute(&*migration.sql).await;

            let elapsed = start.elapsed();

            // language=MySQL
            let _ = query(
                r#"
    INSERT INTO _sqlx_migrations ( version, description, success, checksum, execution_time )
    VALUES ( ?, ?, ?, ?, ? )
                "#,
            )
            .bind(migration.version)
            .bind(&*migration.description)
            .bind(res.is_ok())
            .bind(&*migration.checksum)
            .bind(elapsed.as_nanos() as i64)
            .execute(self)
            .await?;

            res?;

            Ok(elapsed)
        })
    }

    fn revert<'e: 'm, 'm>(
        &'e mut self,
        migration: &'m Migration,
    ) -> BoxFuture<'m, Result<Duration, MigrateError>> {
        Box::pin(async move {
            let start = Instant::now();

            self.execute(&*migration.sql).await?;

            let elapsed = start.elapsed();

            // language=SQL
            let _ = query(r#"DELETE FROM _sqlx_migrations WHERE version = ?"#)
                .bind(migration.version)
                .execute(self)
                .await?;

            Ok(elapsed)
        })
    }
}

async fn current_database(conn: &mut MySqlConnection) -> Result<String, MigrateError> {
    // language=MySQL
    Ok(query_scalar("SELECT DATABASE()").fetch_one(conn).await?)
}

// inspired from rails: https://github.com/rails/rails/blob/6e49cc77ab3d16c06e12f93158eaf3e507d4120e/activerecord/lib/active_record/migration.rb#L1308
fn generate_lock_id(database_name: &str) -> String {
    // 0x3d32ad9e chosen by fair dice roll
    format!(
        "{:x}",
        0x3d32ad9e * (crc32::checksum_ieee(database_name.as_bytes()) as i64)
    )
}
