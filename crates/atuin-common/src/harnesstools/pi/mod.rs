use std::path::PathBuf;

use super::{Harness, InstallHookError};
use crate::utils::{env_nonempty, home_dir};

#[derive(Debug, Clone, Copy, Default)]
pub struct Pi;

impl Pi {
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

impl Harness for Pi {
    fn name(&self) -> &'static str {
        "pi"
    }

    /// Install the hooks on the current active user's machine
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        const PLUGIN_SOURCE: &str = include_str!("../../../res/harnesstools/pi/atuin.ts");
        const PLUGIN_NAME: &str = "atuin.ts";

        // Pi does this thing where they respect the PI_CODING_AGENT_DIR environment variable.
        let extensions_dir = env_nonempty("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".pi").join("agent"))
            .join("extensions");

        tokio::fs::create_dir_all(&extensions_dir).await?;

        let extension_path = extensions_dir.join(PLUGIN_NAME);

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
