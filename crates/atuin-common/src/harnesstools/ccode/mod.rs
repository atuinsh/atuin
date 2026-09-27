//! Integrations and utilities for interacting with claude code.
use std::path::PathBuf;

use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
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

    /// `claude --resume <id>` (`--fork-session` to branch a copy). Since CC 2.1.223 it finds the
    /// id from any directory, but the session works in the directory it is started from.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        // A subagent's transcript is captured as its own session (`agent-<id>`), but Claude Code
        // only resumes the session that spawned it.
        if target.id.starts_with("agent-") {
            return Err(ResumeError::NotResumable(
                "a Claude Code subagent cannot be resumed on its own; resume its parent session",
            ));
        }
        let mut args = vec!["--resume".to_owned(), target.id.clone()];
        if target.fork {
            args.push("--fork-session".to_owned());
        }
        Ok(resume::plan(target, CwdRequirement::Preferred, "claude", args))
    }

    async fn locate(&self, id: &str) -> Option<PathBuf> {
        let (root, id) = (session::default_root(), id.to_owned());
        resume::blocking(move || session::locate(&root, &id)).await
    }
}
