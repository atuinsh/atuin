use atuin_dotfiles::store::{AliasStore, var::VarStore};
use atuin_scripts::store::ScriptStore;
use eyre::{Context, Result};

use atuin_client::{
    database::Database, history::store::HistoryStore, record::sqlite_store::SqliteStore,
    settings::Settings,
};
use atuin_common::record::RecordId;

// This is the only crate that ties together all other crates.
// Therefore, it's the only crate where functions tying together all stores can live

/// Rebuild all stores after a sync
/// Note: for history, this only does an _incremental_ sync. Hence the need to specify downloaded
/// records.
pub async fn build(
    settings: &Settings,
    store: &SqliteStore,
    db: &dyn Database,
    downloaded: Option<&[RecordId]>,
) -> Result<()> {
    let encryption_key: [u8; 32] = atuin_client::encryption::load_key(settings)
        .context("could not load encryption key")?
        .into();

    let host_id = Settings::host_id().expect("failed to get host_id");

    let downloaded = downloaded.unwrap_or(&[]);

    let history_store = HistoryStore::new(store.clone(), host_id, encryption_key);
    let alias_store = AliasStore::new(store.clone(), host_id, encryption_key);
    let var_store = VarStore::new(store.clone(), host_id, encryption_key);
    let script_store = ScriptStore::new(store.clone(), host_id, encryption_key);

    history_store.incremental_build(db, downloaded).await?;

    alias_store.build().await?;
    var_store.build().await?;

    let script_db =
        atuin_scripts::database::Database::new(settings.scripts.database_path.clone(), 1.0).await?;
    script_store.build(script_db).await?;

    Ok(())
}
