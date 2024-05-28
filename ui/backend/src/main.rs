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
use atuin_history::stats;
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
async fn list(offset: Option<u64>) -> Result<Vec<UIHistory>, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;

    let db_path = PathBuf::from(settings.db_path.as_str());
    let db = HistoryDB::new(db_path, settings.local_timeout).await?;

    let history = db
        .list(Some(offset.unwrap_or(0)), Some(100))
        .await?
        .into_iter()
        .map(|h| h.into())
        .collect();

    Ok(history)
}

#[tauri::command]
async fn search(query: String, offset: Option<u64>) -> Result<Vec<UIHistory>, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;

    let db_path = PathBuf::from(settings.db_path.as_str());
    let db = HistoryDB::new(db_path, settings.local_timeout).await?;

    let history = db.search(offset, query.as_str()).await?;

    Ok(history)
}

#[tauri::command]
async fn global_stats() -> Result<GlobalStats, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;
    let db_path = PathBuf::from(settings.db_path.as_str());
    let db = HistoryDB::new(db_path, settings.local_timeout).await?;

    let mut stats = db.global_stats().await?;

    let history = db.list(None, None).await?;
    let history_stats = stats::compute(&settings, &history, 10, 1);

    stats.stats = history_stats;

    Ok(stats)
}

#[tauri::command]
async fn config() -> Result<Settings, String> {
    Settings::new().map_err(|e| e.to_string())
}

#[tauri::command]
async fn session() -> Result<String, String> {
    Settings::new().map_err(|e|e.to_string())?.session_token().map_err(|e|e.to_string())
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
        settings.session_token().map_err(|e|e.to_string())?.as_str(),
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
            config,
            session,
            dotfiles::aliases::import_aliases,
            dotfiles::aliases::delete_alias,
            dotfiles::aliases::set_alias,
            dotfiles::vars::vars,
            dotfiles::vars::delete_var,
            dotfiles::vars::set_var,
        ])
        .plugin(tauri_plugin_sql::Builder::default().build())
        .plugin(tauri_plugin_http::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
