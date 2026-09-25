use std::path::{Path, PathBuf};

use super::{Harness, InstallHookError};
use crate::utils::{env_nonempty, home_dir};

pub mod session;

/// Pi's config directory: `PI_CODING_AGENT_DIR`, else `~/.pi/agent` (pi-mono
/// `packages/coding-agent/src/config.ts` `getAgentDir`).
fn agent_dir() -> PathBuf {
    env_nonempty("PI_CODING_AGENT_DIR")
        .map_or_else(|| home_dir().join(".pi").join("agent"), |dir| expand_tilde(&dir))
}

/// Expand a leading `~` the way pi does for its path settings and variables (pi-mono
/// `packages/coding-agent/src/utils/paths.ts` `normalizePath`): `~` alone and `~/...`, nothing
/// else.
fn expand_tilde(path: &std::ffi::OsStr) -> PathBuf {
    let path = Path::new(path);
    match path.strip_prefix("~") {
        Ok(rest)
            if path.to_str().is_some_and(|p| {
                p == "~" || p.starts_with("~/") || (cfg!(windows) && p.starts_with("~\\"))
            }) =>
        {
            home_dir().join(rest)
        }
        _ => path.to_path_buf(),
    }
}

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

        let extensions_dir = agent_dir().join("extensions");

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
