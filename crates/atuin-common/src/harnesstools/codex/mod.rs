use std::path::PathBuf;

use super::rehydrate::{RehydrateError, RehydrateSession};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::{Harness, InstallHookError, json_hooks};
use crate::dirs::home_dir;

pub mod rehydrate;
pub mod session;
pub(crate) mod state_db;

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
        json_hooks::install(&config_path, "^Bash$", self.name()).await?;
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
