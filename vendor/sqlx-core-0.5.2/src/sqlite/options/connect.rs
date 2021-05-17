use crate::connection::ConnectOptions;
use crate::error::Error;
use crate::executor::Executor;
use crate::sqlite::connection::establish::establish;
use crate::sqlite::{SqliteConnectOptions, SqliteConnection};
use futures_core::future::BoxFuture;
use log::LevelFilter;
use std::time::Duration;

impl ConnectOptions for SqliteConnectOptions {
    type Connection = SqliteConnection;

    fn connect(&self) -> BoxFuture<'_, Result<Self::Connection, Error>>
    where
        Self::Connection: Sized,
    {
        Box::pin(async move {
            let mut conn = establish(self).await?;

            // send an initial sql statement comprised of options
            //
            // Note that locking_mode should be set before journal_mode; see
            // https://www.sqlite.org/wal.html#use_of_wal_without_shared_memory .
            let init = format!(
                "PRAGMA locking_mode = {}; PRAGMA journal_mode = {}; PRAGMA foreign_keys = {}; PRAGMA synchronous = {}; PRAGMA auto_vacuum = {}",
                self.locking_mode.as_str(),
                self.journal_mode.as_str(),
                if self.foreign_keys { "ON" } else { "OFF" },
                self.synchronous.as_str(),
                self.auto_vacuum.as_str(),
            );

            conn.execute(&*init).await?;

            Ok(conn)
        })
    }

    fn log_statements(&mut self, level: LevelFilter) -> &mut Self {
        self.log_settings.log_statements(level);
        self
    }

    fn log_slow_statements(&mut self, level: LevelFilter, duration: Duration) -> &mut Self {
        self.log_settings.log_slow_statements(level, duration);
        self
    }
}
