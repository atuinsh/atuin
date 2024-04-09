use std::path::PathBuf;

use atuin_client::{encryption, record::sqlite_store::SqliteStore, settings::Settings};
use atuin_dotfiles::{shell::Alias, store::AliasStore};

#[tauri::command]
pub async fn aliases() -> Result<Vec<Alias>, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;

    let record_store_path = PathBuf::from(settings.record_store_path.as_str());
    let sqlite_store = SqliteStore::new(record_store_path, settings.local_timeout)
        .await
        .map_err(|e| e.to_string())?;

    let encryption_key: [u8; 32] = encryption::load_key(&settings)
        .map_err(|e| format!("could not load encryption key: {}", e.to_string()))?
        .into();

    let host_id = Settings::host_id().expect("failed to get host_id");

    let alias_store = AliasStore::new(sqlite_store, host_id, encryption_key);

    let aliases = alias_store
        .aliases()
        .await
        .map_err(|e| format!("failed to load aliases: {}", e.to_string()))?;

    Ok(aliases)
}
