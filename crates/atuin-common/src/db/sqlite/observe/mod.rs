mod config;
mod driver;
mod error;
mod event;
mod schema;
mod table;

use std::path::Path;
use std::time::Duration;

use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use tokio::sync::mpsc;
use tokio_util::task::AbortOnDropHandle;

use driver::{AppendStrategy, MutateStrategy, Strategy, run};

pub use config::{ObserveConfig, Replay};
pub use error::ObserveError;
pub use event::{Appended, Change, ChangeKind};
pub use schema::{Cursor, Diffable, Tailable, TableSchema};
pub use table::SqliteTableObserver;

#[derive(Debug, Clone)]
pub struct SqliteObserver {
    opts: SqliteConnectOptions,
}

impl SqliteObserver {
    #[must_use]
    pub fn new(path: impl AsRef<Path>) -> Self {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .immutable(false)
            .busy_timeout(Duration::from_secs(5));
        Self { opts }
    }

    #[must_use]
    pub fn from_options(opts: SqliteConnectOptions) -> Self {
        Self { opts }
    }

    pub async fn append<T: Tailable>(
        &self,
        cfg: ObserveConfig,
    ) -> Result<SqliteTableObserver<Appended<T>>, ObserveError> {
        let conn = SqliteConnection::connect_with(&self.opts)
            .await
            .map_err(ObserveError::Connect)?;
        Ok(spawn_observer(self.opts.clone(), conn, AppendStrategy::<T>::new(), cfg))
    }

    pub async fn mutate<T: Diffable>(
        &self,
        cfg: ObserveConfig,
    ) -> Result<SqliteTableObserver<Change<T>>, ObserveError> {
        let conn = SqliteConnection::connect_with(&self.opts)
            .await
            .map_err(ObserveError::Connect)?;
        Ok(spawn_observer(self.opts.clone(), conn, MutateStrategy::<T>::new(), cfg))
    }
}

fn spawn_observer<S: Strategy>(
    opts: SqliteConnectOptions,
    conn: SqliteConnection,
    strategy: S,
    cfg: ObserveConfig,
) -> SqliteTableObserver<S::Event> {
    let (tx, rx) = mpsc::channel(cfg.channel_capacity.get());
    let task = tokio::spawn(run(opts, conn, strategy, cfg, tx));
    SqliteTableObserver::new(rx, AbortOnDropHandle::new(task))
}
