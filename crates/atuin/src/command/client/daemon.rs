use eyre::Result;

use atuin_client::{database::Sqlite, record::sqlite_store::SqliteStore, settings::Settings};
use atuin_daemon::server::listen;

pub async fn run(settings: Settings, store: SqliteStore, history_db: Sqlite) -> Result<()> {
    listen(settings, store, history_db).await?;

    Ok(())
}
