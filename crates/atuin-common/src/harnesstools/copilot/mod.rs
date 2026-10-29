//! GitHub Copilot CLI (`copilot`): reading its sessions, writing them back out, installing its
//! hooks and resuming.
//!
//! Copilot's log format is undocumented and the CLI closed: what the reader and writer know of
//! it was verified against Copilot CLI 1.0.104 (see their module docs).

use std::path::PathBuf;

use serde_json::json;

use super::rehydrate::{RehydrateError, RehydrateSession};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::{Harness, InstallHookError, json_hooks};
use crate::dirs::home_dir;
use crate::env::var_nonempty;
use crate::path::PathBufExt as _;

pub mod rehydrate;
pub mod session;

/// Copilot's config directory: `$COPILOT_HOME`, else `~/.copilot`.
pub(crate) fn copilot_home() -> PathBuf {
    var_nonempty("COPILOT_HOME").map(PathBuf::from).unwrap_or_else(|| home_dir().joined(".copilot"))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Copilot;

impl Harness for Copilot {
    fn name(&self) -> &'static str {
        "copilot"
    }

    fn alias_names(&self) -> &'static [&'static str] {
        &["github-copilot", "copilot-cli"]
    }

    /// Copilot reads every `*.json` in its hooks directory. Under PascalCase event names it
    /// takes Claude Code's entries and sends Claude Code's payload (`hook_event_name`,
    /// `tool_name` `Bash`, `tool_input.command`), so atuin's hook reads it as Claude Code's
    /// (verified against Copilot CLI 1.0.104).
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        let config_path = copilot_home().joined("hooks").joined("atuin.json");
        json_hooks::install_in(&config_path, "^Bash$", self.name(), json!({"version": 1})).await?;
        Ok(config_path)
    }

    /// `copilot --resume=<id>`. Copilot finds the session from anywhere and reopens it in the
    /// directory its log records, wherever it is run from.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        // A subagent is captured as a session of its own, but Copilot only resumes the session
        // that spawned it.
        if session::is_subagent_id(&target.id) {
            return Err(ResumeError::NotResumable(
                "a Copilot subagent cannot be resumed on its own; resume its parent session",
            ));
        }
        let args = [format!("--resume={}", target.id)];
        Ok(resume::plan(target, CwdRequirement::None, "copilot", args))
    }

    async fn locate(&self, id: &str) -> Option<PathBuf> {
        let (root, id) = (session::default_root(), id.to_owned());
        resume::blocking(move || session::locate(&root, &id)).await
    }

    async fn rehydrate(&self, session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
        rehydrate::rehydrate(session).await
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn resumes_by_id_from_anywhere() {
        let target = ResumeTarget::new("8623b9dc-b2aa-40dc-a8ce-1f3738d4d4db").with_cwd("/work");
        let plan = Copilot.resume_plan(&target).unwrap();
        assert_eq!(plan.program, "copilot");
        assert_eq!(plan.args, ["--resume=8623b9dc-b2aa-40dc-a8ce-1f3738d4d4db"]);
    }

    #[rstest]
    fn a_subagent_is_not_resumable_on_its_own() {
        let target = ResumeTarget::new("agent-toolu_01Cz14Gbx74QYDbB2WUL3D5T");
        assert!(matches!(Copilot.resume_plan(&target), Err(ResumeError::NotResumable(_))));
    }

    #[rstest]
    fn is_found_by_its_names() {
        for name in ["copilot", "github-copilot", "copilot-cli"] {
            let harness = super::super::AnyHarness::from_name(name).unwrap();
            assert_eq!(harness.name(), "copilot", "{name}");
        }
    }
}
