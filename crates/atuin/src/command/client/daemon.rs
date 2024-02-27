use eyre::{Result, WrapErr};

use atuin_client::{
    database::Sqlite, encryption, history::store::HistoryStore, record::sqlite_store::SqliteStore,
    settings::Settings,
};
use atuin_daemon::server::listen;

pub async fn run(settings: Settings, store: SqliteStore, history_db: Sqlite) -> Result<()> {
    let encryption_key: [u8; 32] = encryption::load_key(&settings)
        .context("could not load encryption key")?
        .into();

    let host_id = Settings::host_id().expect("failed to get host_id");
    let history_store = HistoryStore::new(store.clone(), host_id, encryption_key);

    listen(
        history_store,
        history_db,
        settings.daemon.socket_path.into(),
    )
    .await?;

    Ok(())
}
