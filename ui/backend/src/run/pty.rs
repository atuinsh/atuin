use eyre::{Result, WrapErr};
use std::io::BufRead;
use std::path::PathBuf;

use crate::state::AtuinState;
use tauri::{Emitter, Manager, State};

use atuin_client::{database::Sqlite, record::sqlite_store::SqliteStore, settings::Settings};

#[tauri::command]
pub async fn pty_open<'a>(
    app: tauri::AppHandle,
    state: State<'a, AtuinState>,
    cwd: Option<String>,
) -> Result<uuid::Uuid, String> {
    let id = uuid::Uuid::new_v4();

    let cwd = cwd.map(|c| shellexpand::tilde(c.as_str()).to_string());
    let pty = crate::pty::Pty::open(24, 80, cwd).await.unwrap();

    let reader = pty.reader.clone();

    tauri::async_runtime::spawn_blocking(move || loop {
        let mut buf = [0u8; 512];

        match reader.lock().unwrap().read(&mut buf) {
            // EOF
            Ok(0) => {
                println!("reader loop hit eof");
                break;
            }

            Ok(n) => {
                println!("read {n} bytes");

                // TODO: sort inevitable encoding issues
                let out = String::from_utf8_lossy(&buf).to_string();
                let out = out.trim_matches(char::from(0));
                let channel = format!("pty-{id}");

                app.emit(channel.as_str(), out).unwrap();
            }

            Err(e) => {
                println!("failed to read: {e}");
                break;
            }
        }
    });

    state.pty_sessions.write().await.insert(id, pty);

    Ok(id)
}

#[tauri::command]
pub(crate) async fn pty_write(
    pid: uuid::Uuid,
    data: String,
    state: tauri::State<'_, AtuinState>,
) -> Result<(), String> {
    let sessions = state.pty_sessions.read().await;
    let pty = sessions.get(&pid).ok_or("Pty not found")?;

    let bytes = data.as_bytes().to_vec();
    pty.send_bytes(bytes.into())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn pty_resize(
    pid: uuid::Uuid,
    rows: u16,
    cols: u16,
    state: tauri::State<'_, AtuinState>,
) -> Result<(), String> {
    let sessions = state.pty_sessions.read().await;
    let pty = sessions.get(&pid).ok_or("Pty not found")?;

    pty.resize(rows, cols).await.map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
pub(crate) async fn pty_kill(
    pid: uuid::Uuid,
    state: tauri::State<'_, AtuinState>,
) -> Result<(), String> {
    let pty = state.pty_sessions.write().await.remove(&pid);

    match pty {
        Some(pty) => {
            pty.kill_child().await.map_err(|e| e.to_string())?;
            println!("RIP {pid:?}");
        }
        None => {}
    }

    Ok(())
}
