use crate::connection::ConnectOptions;
use crate::error::Error;
use crate::postgres::{PgConnectOptions, PgConnection};
use futures_core::future::BoxFuture;
use log::LevelFilter;
use std::time::Duration;

impl ConnectOptions for PgConnectOptions {
    type Connection = PgConnection;

    fn connect(&self) -> BoxFuture<'_, Result<Self::Connection, Error>>
    where
        Self::Connection: Sized,
    {
        Box::pin(PgConnection::establish(self))
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
