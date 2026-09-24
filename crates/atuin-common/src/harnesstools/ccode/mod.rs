//! Integrations and utilities for interacting with claude code.
use std::path::PathBuf;

use super::{Harness, InstallHookError, json_hooks};
use crate::utils::home_dir;

pub mod session;

#[derive(Debug, Clone, Copy, Default)]
pub struct Ccode;

impl Ccode {
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

impl Harness for Ccode {
    fn name(&self) -> &'static str {
        "claude-code"
    }

    fn alias_names(&self) -> &'static [&'static str] {
        &["claude"]
    }

    /// Install the hooks on the current active user's machine
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        // Claude Code reads hooks from its settings.json.
        let config_path = home_dir().join(".claude").join("settings.json");
        json_hooks::install(&config_path, "^Bash$", self.name()).await?;
        Ok(config_path)
    }
}
