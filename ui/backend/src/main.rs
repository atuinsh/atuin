// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use atuin_client::settings::Settings;

mod db;
mod dotfiles;
mod store;

use db::{GlobalStats, HistoryDB, UIHistory};
use dotfiles::aliases::aliases;

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

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list,
            search,
            global_stats,
            aliases,
            dotfiles::aliases::import_aliases,
            dotfiles::aliases::delete_alias,
            dotfiles::aliases::set_alias,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
