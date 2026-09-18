use std::path::PathBuf;

use super::{Harness, InstallHookError, json_hooks};
use crate::utils::home_dir;

pub mod session;

#[derive(Debug, Clone, Copy, Default)]
pub struct Codex;

impl Codex {
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

impl Harness for Codex {
    fn name(&self) -> &'static str {
        "codex"
    }

    /// Install the hooks on the current active user's machine
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        // Codex reads Claude-Code-style hooks from ~/.codex/hooks.json.
        let config_path = home_dir().join(".codex").join("hooks.json");
        json_hooks::install(&config_path, "^Bash$", "atuin hook codex").await?;
        Ok(config_path)
    }
}
