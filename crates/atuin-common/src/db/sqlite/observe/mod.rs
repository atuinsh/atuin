mod config;
mod driver;
mod error;
mod event;
mod schema;
mod table;

use std::path::Path;
use std::time::Duration;

pub use config::{ObserveConfig, Replay};
use driver::{AppendStrategy, DeliverError, MutateStrategy, Strategy, run};
pub use error::ObserveError;
pub use event::{Appended, Change, ChangeKind};
pub use schema::{Cursor, Diffable, TableSchema, Tailable};
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
pub use table::SqliteTableObserver;
use tokio::sync::mpsc;
use tokio_util::task::AbortOnDropHandle;

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
        let conn =
            SqliteConnection::connect_with(&self.opts).await.map_err(ObserveError::Connect)?;
        spawn_observer(self.opts.clone(), conn, AppendStrategy::<T>::new(), cfg).await
    }

    pub async fn mutate<T: Diffable>(
        &self,
        cfg: ObserveConfig,
    ) -> Result<SqliteTableObserver<Change<T>>, ObserveError> {
        let conn =
            SqliteConnection::connect_with(&self.opts).await.map_err(ObserveError::Connect)?;
        spawn_observer(self.opts.clone(), conn, MutateStrategy::<T>::new(), cfg).await
    }
}

async fn spawn_observer<S: Strategy>(
    opts: SqliteConnectOptions,
    mut conn: SqliteConnection,
    mut strategy: S,
    cfg: ObserveConfig,
) -> Result<SqliteTableObserver<S::Event>, ObserveError> {
    let (tx, rx) = mpsc::channel(cfg.channel_capacity.get());
    let seeded = if matches!(cfg.replay, Replay::FromNow) {
        match strategy.seed(&mut conn, cfg.replay, &tx).await {
            Ok(()) | Err(DeliverError::ConsumerGone) => true,
            Err(DeliverError::Sqlx(e)) => return Err(ObserveError::Seed(e)),
        }
    } else {
        false
    };
    let task = tokio::spawn(run(opts, conn, strategy, cfg, tx, seeded));
    Ok(SqliteTableObserver::new(rx, AbortOnDropHandle::new(task)))
}
