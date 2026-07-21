use atuin_dotfiles::store::{AliasStore, var::VarStore};
use atuin_scripts::store::ScriptStore;
use eyre::{Context, Result};

use atuin_client::{
    database::Database, history::store::HistoryStore, record::sqlite_store::SqliteStore,
    settings::Settings,
};
use atuin_common::record::RecordId;
use atuin_kv::store::KvStore;

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

    let host_id = Settings::host_id().await?;

    let downloaded = downloaded.unwrap_or(&[]);

    let kv_db = atuin_kv::database::Database::new(settings.kv.db_path.clone(), 1.0).await?;

    let history_store = HistoryStore::new(store.clone(), host_id, encryption_key);
    let alias_store = AliasStore::new(store.clone(), host_id, encryption_key);
    let var_store = VarStore::new(store.clone(), host_id, encryption_key);
    let kv_store = KvStore::new(store.clone(), kv_db, host_id, encryption_key);
    let script_store = ScriptStore::new(store.clone(), host_id, encryption_key);

    // A failure in one store should not stop the others from building - build as much as
    // possible, and warn about the rest.
    if let Err(e) = history_store.build_all(db, downloaded).await {
        eprintln!("Warning: failed to build history: {e}");
    }

    if let Err(e) = alias_store.build().await {
        eprintln!("Warning: failed to build aliases: {e}");
    }

    if let Err(e) = var_store.build().await {
        eprintln!("Warning: failed to build vars: {e}");
    }

    if let Err(e) = kv_store.build().await {
        eprintln!("Warning: failed to build kv: {e}");
    }

    let script_db =
        atuin_scripts::database::Database::new(settings.scripts.db_path.clone(), 1.0).await?;

    if let Err(e) = script_store.build(script_db).await {
        eprintln!("Warning: failed to build scripts: {e}");
    }

    Ok(())
}
