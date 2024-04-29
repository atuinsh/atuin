use std::path::PathBuf;

use atuin_client::{encryption, record::sqlite_store::SqliteStore, settings::Settings};
use atuin_common::shell::Shell;
use atuin_dotfiles::{
    shell::{existing_aliases, Alias, Var},
    store::var::VarStore,
};

async fn var_store() -> eyre::Result<VarStore> {
    let settings = Settings::new()?;

    let record_store_path = PathBuf::from(settings.record_store_path.as_str());
    let sqlite_store = SqliteStore::new(record_store_path, settings.local_timeout).await?;

    let encryption_key: [u8; 32] = encryption::load_key(&settings)?.into();

    let host_id = Settings::host_id().expect("failed to get host_id");

    Ok(VarStore::new(sqlite_store, host_id, encryption_key))
}

#[tauri::command]
pub async fn vars() -> Result<Vec<Var>, String> {
    let var_store = var_store().await.map_err(|e| e.to_string())?;

    let vars = var_store
        .vars()
        .await
        .map_err(|e| format!("failed to load aliases: {}", e))?;

    Ok(vars)
}

#[tauri::command]
pub async fn delete_var(name: String) -> Result<(), String> {
    let var_store = var_store().await.map_err(|e| e.to_string())?;

    var_store
        .delete(name.as_str())
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn set_var(name: String, value: String, export: bool) -> Result<(), String> {
    let var_store = var_store().await.map_err(|e| e.to_string())?;

    var_store
        .set(name.as_str(), value.as_str(), export)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}
