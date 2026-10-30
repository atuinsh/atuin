//! Integrations and utilities for interacting with claude code.
use std::path::PathBuf;

use super::rehydrate::{RehydrateError, RehydrateSession};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::{Harness, InstallHookError, json_hooks};
use crate::dirs::home_dir;
use crate::env::var_nonempty;
use crate::path::PathBufExt as _;

pub mod rehydrate;
pub mod session;

/// Claude Code's config directory: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub(crate) fn config_dir() -> PathBuf {
    var_nonempty("CLAUDE_CONFIG_DIR").map_or_else(|| home_dir().joined(".claude"), PathBuf::from)
}

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
        let config_path = config_dir().joined("settings.json");
        json_hooks::install(&config_path, "^Bash$", self.name()).await?;
        Ok(config_path)
    }

    /// `claude --resume <id>`. Claude Code finds the id from any directory, but the session
    /// works in the directory it is started from.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        // A subagent's transcript is captured as its own session (`agent-<id>`), but Claude Code
        // only resumes the session that spawned it.
        if target.id.starts_with("agent-") {
            return Err(ResumeError::NotResumable(
                "a Claude Code subagent cannot be resumed on its own; resume its parent session",
            ));
        }
        let args = ["--resume".to_owned(), target.id.clone()];
        Ok(resume::plan(target, CwdRequirement::Preferred, "claude", args))
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
    use serde_json::Value;

    use super::*;
    use crate::env::MockEnv;

    #[rstest::fixture]
    fn home() -> (tempfile::TempDir, MockEnv) {
        let dir = tempfile::tempdir().unwrap();
        let env = MockEnv::install();
        env.set("HOME", dir.path());
        (dir, env)
    }

    #[rstest]
    fn the_config_dir_is_dot_claude_unless_overridden(home: (tempfile::TempDir, MockEnv)) {
        let (dir, env) = home;
        assert_eq!(config_dir(), dir.path().join(".claude"));
        env.set("CLAUDE_CONFIG_DIR", "");
        assert_eq!(config_dir(), dir.path().join(".claude"));
        env.set("CLAUDE_CONFIG_DIR", dir.path().join("custom"));
        assert_eq!(config_dir(), dir.path().join("custom"));
    }

    /// With `CLAUDE_CONFIG_DIR` set, the hooks land in the settings Claude Code reads from there,
    /// not the real `~/.claude/settings.json`.
    #[rstest]
    #[tokio::test]
    async fn hooks_install_under_claude_config_dir(home: (tempfile::TempDir, MockEnv)) {
        let (dir, env) = home;
        let custom = dir.path().join("custom");
        env.set("CLAUDE_CONFIG_DIR", &custom);

        let path = Ccode::new().install_hooks().await.unwrap();

        assert_eq!(path, custom.join("settings.json"));
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(settings["hooks"]["PreToolUse"].is_array(), "{settings}");
        assert!(!dir.path().join(".claude").exists());
    }
}
