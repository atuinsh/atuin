// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use atuin_client::database::OptFilters;
use atuin_client::settings::{FilterMode, SearchMode};
use serde::Serialize;
use std::path::PathBuf;
use time::OffsetDateTime;
use uuid::Uuid;

use tauri::SystemTray;
use tauri::{CustomMenuItem, SystemTrayMenu, SystemTrayMenuItem};

use atuin_client::history::HistoryId;

use atuin_client::{
    database::{Context, Database, Sqlite},
    history::History,
    settings::Settings,
};

mod db;

use db::{GlobalStats, HistoryDB, UIHistory};

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
    // here `"quit".to_string()` defines the menu item id, and the second parameter is the menu item label.
    let quit = CustomMenuItem::new("quit".to_string(), "Quit");
    let tray_menu = SystemTrayMenu::new().add_item(quit);
    let tray = SystemTray::new().with_menu(tray_menu);

    tauri::Builder::default()
        .system_tray(tray)
        .invoke_handler(tauri::generate_handler![list, search, global_stats])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
