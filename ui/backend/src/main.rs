// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use time::format_description::well_known::Rfc3339;

use atuin_client::settings::Settings;

mod db;
mod dotfiles;
mod store;

use atuin_client::{
    encryption, history::HISTORY_TAG, record::sqlite_store::SqliteStore, record::store::Store,
};
use db::{GlobalStats, HistoryDB, UIHistory};
use dotfiles::aliases::aliases;

#[derive(Debug, serde::Serialize)]
struct HomeInfo {
    pub username: String,
    pub record_count: u64,
    pub history_count: u64,
    pub last_sync: String,
}

#[tauri::command]
async fn list() -> Result<Vec<UIHistory>, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;

    let db_path = PathBuf::from(settings.db_path.as_str());
    let db = HistoryDB::new(db_path, settings.local_timeout).await?;

    let history = db.list(Some(100), false).await?;

    Ok(history)
}

#[tauri::command]
async fn search(query: String) -> Result<Vec<UIHistory>, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;

    let db_path = PathBuf::from(settings.db_path.as_str());
    let db = HistoryDB::new(db_path, settings.local_timeout).await?;

    let history = db.search(query.as_str()).await?;

    Ok(history)
}

#[tauri::command]
async fn global_stats() -> Result<GlobalStats, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;
    let db_path = PathBuf::from(settings.db_path.as_str());
    let db = HistoryDB::new(db_path, settings.local_timeout).await?;

    let stats = db.global_stats().await?;

    Ok(stats)
}

#[tauri::command]
async fn home_info() -> Result<HomeInfo, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;
    let record_store_path = PathBuf::from(settings.record_store_path.as_str());
    let sqlite_store = SqliteStore::new(record_store_path, settings.local_timeout)
        .await
        .map_err(|e| e.to_string())?;

    let client = atuin_client::api_client::Client::new(
        &settings.sync_address,
        &settings.session_token,
        settings.network_connect_timeout,
        settings.network_timeout,
    )
    .map_err(|e| e.to_string())?;

    let session_path = settings.session_path.as_str();
    let last_sync = Settings::last_sync()
        .map_err(|e| e.to_string())?
        .format(&Rfc3339)
        .map_err(|e| e.to_string())?;
    let record_count = sqlite_store.len_all().await.map_err(|e| e.to_string())?;
    let history_count = sqlite_store
        .len_tag(HISTORY_TAG)
        .await
        .map_err(|e| e.to_string())?;

    let info = if !PathBuf::from(session_path).exists() {
        HomeInfo {
            username: String::from(""),
            last_sync: last_sync.to_string(),
            record_count,
            history_count,
        }
    } else {
        let me = client.me().await.map_err(|e| e.to_string())?;

        HomeInfo {
            username: me.username,
            last_sync: last_sync.to_string(),
            record_count,
            history_count,
        }
    };

    Ok(info)
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list,
            search,
            global_stats,
            aliases,
            home_info,
            dotfiles::aliases::import_aliases,
            dotfiles::aliases::delete_alias,
            dotfiles::aliases::set_alias,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
