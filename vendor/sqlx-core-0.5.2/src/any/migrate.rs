use crate::any::connection::AnyConnectionKind;
use crate::any::kind::AnyKind;
use crate::any::{Any, AnyConnection};
use crate::error::Error;
use crate::migrate::{AppliedMigration, Migrate, MigrateDatabase, MigrateError, Migration};
use futures_core::future::BoxFuture;
use std::str::FromStr;
use std::time::Duration;

impl MigrateDatabase for Any {
    fn create_database(uri: &str) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            match AnyKind::from_str(uri)? {
                #[cfg(feature = "postgres")]
                AnyKind::Postgres => crate::postgres::Postgres::create_database(uri).await,

                #[cfg(feature = "sqlite")]
                AnyKind::Sqlite => crate::sqlite::Sqlite::create_database(uri).await,

                #[cfg(feature = "mysql")]
                AnyKind::MySql => crate::mysql::MySql::create_database(uri).await,

                #[cfg(feature = "mssql")]
                AnyKind::Mssql => unimplemented!(),
            }
        })
    }

    fn database_exists(uri: &str) -> BoxFuture<'_, Result<bool, Error>> {
        Box::pin(async move {
            match AnyKind::from_str(uri)? {
                #[cfg(feature = "postgres")]
                AnyKind::Postgres => crate::postgres::Postgres::database_exists(uri).await,

                #[cfg(feature = "sqlite")]
                AnyKind::Sqlite => crate::sqlite::Sqlite::database_exists(uri).await,

                #[cfg(feature = "mysql")]
                AnyKind::MySql => crate::mysql::MySql::database_exists(uri).await,

                #[cfg(feature = "mssql")]
                AnyKind::Mssql => unimplemented!(),
            }
        })
    }

    fn drop_database(uri: &str) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            match AnyKind::from_str(uri)? {
                #[cfg(feature = "postgres")]
                AnyKind::Postgres => crate::postgres::Postgres::drop_database(uri).await,

                #[cfg(feature = "sqlite")]
                AnyKind::Sqlite => crate::sqlite::Sqlite::drop_database(uri).await,

                #[cfg(feature = "mysql")]
                AnyKind::MySql => crate::mysql::MySql::drop_database(uri).await,

                #[cfg(feature = "mssql")]
                AnyKind::Mssql => unimplemented!(),
            }
        })
    }
}

impl Migrate for AnyConnection {
    fn ensure_migrations_table(&mut self) -> BoxFuture<'_, Result<(), MigrateError>> {
        match &mut self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn.ensure_migrations_table(),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn.ensure_migrations_table(),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn.ensure_migrations_table(),

            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(_conn) => unimplemented!(),
        }
    }

    fn dirty_version(&mut self) -> BoxFuture<'_, Result<Option<i64>, MigrateError>> {
        match &mut self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn.dirty_version(),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn.dirty_version(),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn.dirty_version(),

            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(_conn) => unimplemented!(),
        }
    }

    fn list_applied_migrations(
        &mut self,
    ) -> BoxFuture<'_, Result<Vec<AppliedMigration>, MigrateError>> {
        match &mut self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn.list_applied_migrations(),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn.list_applied_migrations(),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn.list_applied_migrations(),

            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(_conn) => unimplemented!(),
        }
    }

    fn lock(&mut self) -> BoxFuture<'_, Result<(), MigrateError>> {
        match &mut self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn.lock(),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn.lock(),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn.lock(),

            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(_conn) => unimplemented!(),
        }
    }

    fn unlock(&mut self) -> BoxFuture<'_, Result<(), MigrateError>> {
        match &mut self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn.unlock(),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn.unlock(),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn.unlock(),

            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(_conn) => unimplemented!(),
        }
    }

    fn apply<'e: 'm, 'm>(
        &'e mut self,
        migration: &'m Migration,
    ) -> BoxFuture<'m, Result<Duration, MigrateError>> {
        match &mut self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn.apply(migration),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn.apply(migration),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn.apply(migration),

            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(_conn) => {
                let _ = migration;
                unimplemented!()
            }
        }
    }

    fn revert<'e: 'm, 'm>(
        &'e mut self,
        migration: &'m Migration,
    ) -> BoxFuture<'m, Result<Duration, MigrateError>> {
        match &mut self.0 {
            #[cfg(feature = "postgres")]
            AnyConnectionKind::Postgres(conn) => conn.revert(migration),

            #[cfg(feature = "sqlite")]
            AnyConnectionKind::Sqlite(conn) => conn.revert(migration),

            #[cfg(feature = "mysql")]
            AnyConnectionKind::MySql(conn) => conn.revert(migration),

            #[cfg(feature = "mssql")]
            AnyConnectionKind::Mssql(_conn) => unimplemented!(),
        }
    }
}
