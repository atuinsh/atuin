use std::path::PathBuf;

use super::{Harness, InstallHookError};
use crate::utils::{env_nonempty, home_dir};

#[derive(Debug, Clone, Copy, Default)]
pub struct Opencode;

impl Opencode {
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

impl Harness for Opencode {
    fn name(&self) -> &'static str {
        "opencode"
    }

    /// Install the hooks on the current active user's machine
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        const PLUGIN_SOURCE: &str = include_str!("../../../res/harnesstools/opencode/atuin.ts");
        const PLUGIN_NAME: &str = "atuin.ts";

        // Opencode is XDG-compliant: plugins live under $XDG_CONFIG_HOME/opencode,
        // falling back to ~/.config/opencode.
        let plugins_dir = env_nonempty("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".config"))
            .join("opencode")
            .join("plugins");

        tokio::fs::create_dir_all(&plugins_dir).await?;

        let extension_path = plugins_dir.join(PLUGIN_NAME);

        let already_installed = tokio::fs::read_to_string(&extension_path)
            .await
            .is_ok_and(|existing| existing == PLUGIN_SOURCE);

        if already_installed {
            return Err(InstallHookError::AlreadyInstalled);
        }

        tokio::fs::write(&extension_path, PLUGIN_SOURCE).await?;

        Ok(extension_path)
    }
}
