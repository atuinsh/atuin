//! Integrations and utilities for interacting with claude code.
use std::path::PathBuf;

use super::rehydrate::{RehydrateError, RehydrateSession, Rehydrated};
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

/// The settings file that Claude Code reads its hooks from.
fn settings_path() -> PathBuf {
    config_dir().joined("settings.json")
}

/// Registers Atuin's hook for the prompts that the user submits in Claude Code, returning the
/// settings file that holds it.
pub async fn install_prompt_hook() -> Result<PathBuf, InstallHookError> {
    let config_path = settings_path();
    json_hooks::install_prompt_hook(&config_path, Ccode.name()).await?;
    Ok(config_path)
}

/// Takes Atuin's hook for the prompts that the user submits out of Claude Code's settings,
/// keeping Atuin's other hooks. Returns `false` when the settings didn't hold that hook.
pub async fn uninstall_prompt_hook() -> Result<bool, InstallHookError> {
    json_hooks::uninstall_prompt_hook(&settings_path(), Ccode.name()).await
}

/// Whether Atuin's Claude Code hook is installed for every Bash event.
pub async fn is_installed() -> bool {
    let config_path = settings_path();
    tokio::task::spawn_blocking(move || json_hooks::has_installed(&config_path, Ccode.name()))
        .await
        .unwrap_or(false)
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

    fn display_name(&self) -> &'static str {
        "Claude Code"
    }

    fn program(&self) -> &'static str {
        "claude"
    }

    fn home_vars(&self) -> &'static [&'static str] {
        &["CLAUDE_CONFIG_DIR"]
    }

    fn alias_names(&self) -> &'static [&'static str] {
        &["claude"]
    }

    /// Install the hooks on the current active user's machine
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        let config_path = settings_path();
        json_hooks::install(&config_path, "^Bash$", self.name()).await?;
        Ok(config_path)
    }

    async fn has_hooks(&self) -> Result<bool, InstallHookError> {
        json_hooks::has(&config_dir().joined("settings.json"), self.name()).await
    }

    async fn uninstall_hooks(&self) -> Result<bool, InstallHookError> {
        json_hooks::uninstall(&config_dir().joined("settings.json"), self.name()).await
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
        Ok(resume::plan(target, CwdRequirement::Preferred, self.program(), args))
    }

    async fn locate(&self, id: &str) -> Option<PathBuf> {
        let (root, id) = (session::default_root(), id.to_owned());
        resume::blocking(move || session::locate(&root, &id)).await
    }

    async fn rehydrate(&self, session: &RehydrateSession) -> Result<Rehydrated, RehydrateError> {
        rehydrate::rehydrate(session).await.map(Rehydrated::at)
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

    #[rstest]
    #[tokio::test]
    async fn the_prompt_hook_joins_the_installed_hooks(home: (tempfile::TempDir, MockEnv)) {
        let (_dir, _env) = home;
        assert!(!is_installed().await);
        Ccode::new().install_hooks().await.unwrap();
        assert!(is_installed().await);

        let path = install_prompt_hook().await.unwrap();

        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(settings["hooks"]["UserPromptSubmit"].as_array().map(Vec::len), Some(1));
    }

    /// Taking the prompt hook out keeps the Bash hooks and the user's own prompt hook, and a
    /// second run finds nothing to take out.
    #[rstest]
    #[tokio::test]
    async fn uninstalling_the_prompt_hook_keeps_the_other_hooks(
        home: (tempfile::TempDir, MockEnv),
    ) {
        let (_dir, _env) = home;
        Ccode::new().install_hooks().await.unwrap();
        let path = install_prompt_hook().await.unwrap();
        let mut settings: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let theirs = serde_json::json!({"hooks": [{"type": "command", "command": "notify"}]});
        settings["hooks"]["UserPromptSubmit"].as_array_mut().unwrap().push(theirs.clone());
        std::fs::write(&path, settings.to_string()).unwrap();

        let first = uninstall_prompt_hook().await.unwrap();
        let second = uninstall_prompt_hook().await.unwrap();

        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(settings["hooks"]["UserPromptSubmit"], serde_json::json!([theirs]));
        assert!(is_installed().await);
        assert_eq!((first, second), (true, false));
    }
}
