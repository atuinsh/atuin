use std::collections::HashMap;
use std::sync::Mutex;
use tauri::async_runtime::RwLock;

use atuin_run::pty::Pty;

#[derive(Default)]
pub(crate) struct AtuinState {
    pub pty_sessions: RwLock<HashMap<uuid::Uuid, Pty>>,
}
