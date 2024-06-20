use std::path::PathBuf;

use atuin_client::{encryption, record::sqlite_store::SqliteStore, settings::Settings};
use atuin_common::shell::Shell;
use atuin_dotfiles::{
    shell::{existing_aliases, Alias},
    store::AliasStore,
};

async fn alias_store() -> eyre::Result<AliasStore> {
    let settings = Settings::new()?;

    let record_store_path = PathBuf::from(settings.record_store_path.as_str());
    let sqlite_store = SqliteStore::new(record_store_path, settings.local_timeout).await?;

    let encryption_key: [u8; 32] = encryption::load_key(&settings)?.into();

    let host_id = Settings::host_id().expect("failed to get host_id");

    Ok(AliasStore::new(sqlite_store, host_id, encryption_key))
}

#[tauri::command]
pub async fn aliases() -> Result<Vec<Alias>, String> {
    let alias_store = alias_store().await.map_err(|e| e.to_string())?;

    let aliases = alias_store
        .aliases()
        .await
        .map_err(|e| format!("failed to load aliases: {}", e))?;

    Ok(aliases)
}

#[tauri::command]
pub async fn delete_alias(name: String) -> Result<(), String> {
    let alias_store = alias_store().await.map_err(|e| e.to_string())?;

    alias_store
        .delete(name.as_str())
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn set_alias(name: String, value: String) -> Result<(), String> {
    let alias_store = alias_store().await.map_err(|e| e.to_string())?;

    alias_store
        .set(name.as_str(), value.as_str())
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn import_aliases() -> Result<Vec<Alias>, String> {
    let store = alias_store().await.map_err(|e| e.to_string())?;
    let shell = Shell::default_shell().map_err(|e| e.to_string())?;
    let shell_name = shell.to_string();

    if !shell.is_posixish() {
        return Err(format!(
            "Default shell {shell_name} not supported for import"
        ));
    }

    let existing_aliases = existing_aliases(Some(shell)).map_err(|e| e.to_string())?;
    let store_aliases = store.aliases().await.map_err(|e| e.to_string())?;

    let mut res = Vec::new();

    for alias in existing_aliases {
        // O(n), but n is small, and imports infrequent
        // can always make a map
        if store_aliases.contains(&alias) {
            continue;
        }

        res.push(alias.clone());
        store
            .set(&alias.name, &alias.value)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(res)
}
