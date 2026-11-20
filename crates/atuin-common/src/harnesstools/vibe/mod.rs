//! Integrations and utilities for interacting with Mistral Vibe (`vibe`).
//!
//! Every surface Vibe has (its terminal UI, `vibe -p`, `vibe-acp` in the editors, Vibe Desktop's
//! local sessions) runs through `vibe-app-server`, which keeps every session in one store under
//! `$VIBE_HOME/logs/session` (see [`session`]). Vibe Code Web's sessions live on Mistral's
//! servers and are not read.

use std::path::PathBuf;

use super::rehydrate::{RehydrateError, RehydrateSession, Rehydrated};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::{Harness, InstallHookError};
use crate::dirs::home_dir;
use crate::env::var_nonempty;
use crate::path::PathBufExt as _;

pub mod rehydrate;
pub mod session;

/// Vibe's home directory: `$VIBE_HOME`, else `~/.vibe`.
pub(crate) fn home() -> PathBuf {
    var_nonempty("VIBE_HOME").map_or_else(|| home_dir().joined(".vibe"), PathBuf::from)
}

/// Where Vibe keeps its sessions: `<home>/logs/session`.
pub(crate) fn session_root() -> PathBuf {
    home().joined("logs").joined("session")
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Vibe;

impl Harness for Vibe {
    fn name(&self) -> &'static str {
        "vibe"
    }

    fn display_name(&self) -> &'static str {
        "Mistral Vibe"
    }

    fn program(&self) -> &'static str {
        "vibe"
    }

    fn home_vars(&self) -> &'static [&'static str] {
        &["VIBE_HOME"]
    }

    fn alias_names(&self) -> &'static [&'static str] {
        &["mistral-vibe"]
    }

    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        Err(InstallHookError::Unsupported)
    }

    async fn has_hooks(&self) -> Result<bool, InstallHookError> {
        Ok(false)
    }

    async fn uninstall_hooks(&self) -> Result<bool, InstallHookError> {
        Ok(false)
    }

    /// Yes: the writer makes every row it's given part of one conversation, so a turn undone
    /// would come back beside the one that replaced it.
    fn writes_one_branch(&self) -> bool {
        true
    }

    /// Yes: Vibe's import keeps no parent, so the reader names a fork's from what atuin wrote.
    fn restores_fork_links(&self) -> bool {
        true
    }

    /// `vibe --resume <id>`. Vibe finds a session from any directory, and its tools work in the
    /// directory it is started from.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        let args = ["--resume".to_owned(), target.id.clone()];
        Ok(resume::plan(target, CwdRequirement::Preferred, self.program(), args))
    }

    async fn locate(&self, id: &str) -> Option<PathBuf> {
        let (root, id) = (session_root(), id.to_owned());
        resume::blocking(move || session::locate(&root, &id)).await
    }

    async fn rehydrate(&self, session: &RehydrateSession) -> Result<Rehydrated, RehydrateError> {
        rehydrate::rehydrate(&session_root(), session).await.map(Rehydrated::at)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::harnesstools::AnyHarness;

    #[rstest]
    #[case("vibe")]
    #[case("mistral-vibe")]
    fn lookup_resolves_name_and_alias(#[case] name: &str) {
        assert_eq!(AnyHarness::from_name(name).unwrap().name(), "vibe");
    }

    #[rstest]
    fn resume_plan_prefers_session_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let target = ResumeTarget::new("c4e8fddb-566e-39f8-f042-1c480ac90b31").with_cwd(dir.path());
        let plan = Vibe.resume_plan(&target).unwrap().prepare().unwrap();
        assert_eq!(plan.program, "vibe");
        assert_eq!(plan.args, ["--resume", "c4e8fddb-566e-39f8-f042-1c480ac90b31"]);
        assert_eq!(plan.cwd.as_deref(), Some(dir.path()));
    }

    #[rstest]
    #[tokio::test]
    async fn hooks_are_not_offered() {
        assert!(matches!(Vibe.install_hooks().await, Err(InstallHookError::Unsupported)));
    }
}
