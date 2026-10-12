use std::path::PathBuf;

use super::rehydrate::{RehydrateError, RehydrateSession};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::{Harness, InstallHookError, json_hooks};
use crate::utils::{env_nonempty, home_dir};

mod hook_trust;
pub mod rehydrate;
pub mod session;
pub(crate) mod state_db;

/// Codex's home: `$CODEX_HOME`, else `~/.codex`.
pub(crate) fn home() -> PathBuf {
    env_nonempty("CODEX_HOME").map_or_else(|| home_dir().join(".codex"), PathBuf::from)
}

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

    /// Install the hooks on the current active user's machine, trusted (see `hook_trust`).
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        // Codex reads Claude-Code-style hooks from $CODEX_HOME/hooks.json.
        let home = home();
        let config_path = home.join("hooks.json");
        let config = hook_trust::config(&home).await?;
        let installed = match json_hooks::install(&config_path, "^Bash$", self.name()).await {
            Ok(()) => true,
            Err(InstallHookError::AlreadyInstalled) => false,
            Err(err) => return Err(err),
        };
        // Hooks installed before atuin trusted them are trusted on installing them again.
        let trusted = hook_trust::trust(&home, self.name(), config).await?;
        if !installed && !trusted {
            return Err(InstallHookError::AlreadyInstalled);
        }
        Ok(config_path)
    }

    /// `codex resume <id>`. Codex finds the id from any directory, but asks whether to work in
    /// the session's directory or the current one when they differ.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        let id = session::resume_id(&target.id);
        Ok(resume::plan(target, CwdRequirement::Preferred, "codex", [
            "resume".to_owned(),
            id.to_owned(),
        ]))
    }

    async fn locate(&self, id: &str) -> Option<PathBuf> {
        let (root, id) = (session::default_root(), id.to_owned());
        resume::blocking(move || session::locate(&root, &id)).await
    }

    async fn rehydrate(&self, session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
        rehydrate::rehydrate(session).await
    }
}
