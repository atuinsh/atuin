// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::State;

use std::path::PathBuf;

use tauri::{AppHandle, Manager};
use time::format_description::well_known::Rfc3339;

mod db;
mod dotfiles;
mod install;
mod pty;
mod run;
mod state;
mod store;

use atuin_client::settings::Settings;
use atuin_client::{
    encryption, history::HISTORY_TAG, record::sqlite_store::SqliteStore, record::store::Store,
};
use atuin_history::stats;
use db::{GlobalStats, HistoryDB, UIHistory};
use dotfiles::aliases::aliases;

#[derive(Debug, serde::Serialize)]
struct HomeInfo {
    pub record_count: u64,
    pub history_count: u64,
    pub username: Option<String>,
    pub last_sync: Option<String>,
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
    Settings::new()
        .map_err(|e| e.to_string())?
        .session_token()
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn login(username: String, password: String, key: String) -> Result<String, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;

    let record_store_path = PathBuf::from(settings.record_store_path.as_str());
    let store = SqliteStore::new(record_store_path, settings.local_timeout)
        .await
        .map_err(|e| e.to_string())?;

    if settings.logged_in() {
        return Err(String::from("Already logged in"));
    }

    let session = atuin_client::login::login(&settings, &store, username, password, key)
        .await
        .map_err(|e| e.to_string())?;

    Ok(session)
}

#[tauri::command]
async fn logout() -> Result<(), String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;

    atuin_client::logout::logout(&settings)
        .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
async fn register(username: String, email: String, password: String) -> Result<String, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;

    let session = atuin_client::register::register(&settings, username, email, password)
        .await
        .map_err(|e| e.to_string())?;

    Ok(session)
}

#[tauri::command]
async fn home_info() -> Result<HomeInfo, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;
    let record_store_path = PathBuf::from(settings.record_store_path.as_str());
    let sqlite_store = SqliteStore::new(record_store_path, settings.local_timeout)
        .await
        .map_err(|e| e.to_string())?;

    let last_sync = Settings::last_sync()
        .map_err(|e| e.to_string())?
        .format(&Rfc3339)
        .map_err(|e| e.to_string())?;

    let record_count = sqlite_store.len_all().await.map_err(|e| e.to_string())?;
    let history_count = sqlite_store
        .len_tag(HISTORY_TAG)
        .await
        .map_err(|e| e.to_string())?;

    let info = if !settings.logged_in() {
        HomeInfo {
            username: None,
            last_sync: None,
            record_count,
            history_count,
        }
    } else {
        let client = atuin_client::api_client::Client::new(
            &settings.sync_address,
            settings
                .session_token()
                .map_err(|e| e.to_string())?
                .as_str(),
            settings.network_connect_timeout,
            settings.network_timeout,
        )
        .map_err(|e| e.to_string())?;

        let me = client.me().await.map_err(|e| e.to_string())?;

        HomeInfo {
            username: Some(me.username),
            last_sync: Some(last_sync.to_string()),
            record_count,
            history_count,
        }
    };

    Ok(info)
}

// Match the format that the frontend library we use expects
// All the processing in Rust, not JSunwrap.
// Faaaassssssst af ⚡️🦀
#[derive(Debug, serde::Serialize)]
pub struct HistoryCalendarDay {
    pub date: String,
    pub count: u64,
    pub level: u8,
}

#[tauri::command]
async fn history_calendar() -> Result<Vec<HistoryCalendarDay>, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;
    let db_path = PathBuf::from(settings.db_path.as_str());
    let db = HistoryDB::new(db_path, settings.local_timeout).await?;

    let calendar = db.calendar().await?;

    // probs don't want to iterate _this_ many times, but it's only the last year. so 365
    // iterations at max. should be quick.

    let max = calendar
        .iter()
        .max_by_key(|d| d.1)
        .expect("Can't find max count");

    let ret = calendar
        .iter()
        .map(|d| {
            // calculate the "level". we have 5, so figure out which 5th it fits into
            let percent: f64 = d.1 as f64 / max.1 as f64;
            let level = if d.1 == 0 {
                0.0
            } else {
                (percent / 0.2).round() + 1.0
            };

            HistoryCalendarDay {
                date: d.0.clone(),
                count: d.1,
                level: std::cmp::min(4, level as u8),
            }
        })
        .collect();

    Ok(ret)
}

#[tauri::command]
async fn prefix_search(query: &str) -> Result<Vec<String>, String> {
    let settings = Settings::new().map_err(|e| e.to_string())?;

    let db_path = PathBuf::from(settings.db_path.as_str());
    let db = HistoryDB::new(db_path, settings.local_timeout).await?;

    let history = db.prefix_search(query).await?;
    let commands = history.into_iter().map(|h| h.command).collect();

    Ok(commands)
}

fn show_window(app: &AppHandle) {
    let windows = app.webview_windows();

    windows
        .values()
        .next()
        .expect("Sorry, no window found")
        .set_focus()
        .expect("Can't Bring Window to Focus");
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            list,
            search,
            prefix_search,
            global_stats,
            aliases,
            home_info,
            config,
            session,
            login,
            logout,
            register,
            history_calendar,
            run::pty::pty_open,
            run::pty::pty_write,
            run::pty::pty_resize,
            run::pty::pty_kill,
            install::install_cli,
            install::is_cli_installed,
            install::setup_cli,
            dotfiles::aliases::import_aliases,
            dotfiles::aliases::delete_alias,
            dotfiles::aliases::set_alias,
            dotfiles::vars::vars,
            dotfiles::vars::delete_var,
            dotfiles::vars::set_var,
        ])
        .plugin(tauri_plugin_sql::Builder::default().build())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            let _ = show_window(app);
        }))
        .manage(state::AtuinState::default())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
