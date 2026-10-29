//! Integrations and utilities for interacting with Grok Build (`grok`).

use std::path::PathBuf;

use super::rehydrate::{RehydrateError, RehydrateSession};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::{Harness, InstallHookError, json_hooks};
use crate::dirs::home_dir;
use crate::env::var_nonempty;
use crate::path::PathBufExt as _;

/// Grok Build's home directory: `$GROK_HOME`, else `~/.grok`.
pub(crate) fn home() -> PathBuf {
    var_nonempty("GROK_HOME").map_or_else(|| home_dir().joined(".grok"), PathBuf::from)
}

/// Whether `atuin hook install grok` has registered hooks in `$GROK_HOME/hooks/atuin.json`.
#[must_use]
pub fn has_installed_hooks() -> bool {
    json_hooks::has_installed(&home().joined("hooks").joined("atuin.json"), "grok")
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Grok;

impl Grok {
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

impl Harness for Grok {
    fn name(&self) -> &'static str {
        "grok"
    }

    fn alias_names(&self) -> &'static [&'static str] {
        &["grok-build"]
    }

    /// Install Atuin's command hooks in `$GROK_HOME/hooks/atuin.json`.
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        let config_path = home().joined("hooks").joined("atuin.json");
        json_hooks::install(&config_path, "^Bash$", self.name()).await?;
        Ok(config_path)
    }

    /// `grok --resume <id>`. Grok Build finds the session across directories, and works in the
    /// directory it is started from.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        let args = ["--resume".to_owned(), target.id.clone()];
        Ok(resume::plan(target, CwdRequirement::Preferred, "grok", args))
    }

    async fn locate(&self, _id: &str) -> Option<PathBuf> {
        None
    }

    async fn rehydrate(&self, _session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
        Err(RehydrateError::Unsupported("Grok Build"))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::Value;

    use super::*;
    use crate::env::mock::MockEnv;
    use crate::harnesstools::AnyHarness;

    #[rstest]
    #[case("grok")]
    #[case("grok-build")]
    fn lookup_resolves_name_and_alias(#[case] name: &str) {
        let harness = AnyHarness::from_name(name).unwrap();
        assert_eq!(harness.name(), "grok");
    }

    #[rstest]
    #[tokio::test]
    async fn install_hooks_writes_atuin_json_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let env = MockEnv::install();
        env.set("GROK_HOME", dir.path());
        assert!(!has_installed_hooks());

        let path = Grok.install_hooks().await.unwrap();
        assert_eq!(path, dir.path().join("hooks").joined("atuin.json"));
        assert!(has_installed_hooks());

        let parsed: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for stage in ["PreToolUse", "PostToolUse", "PostToolUseFailure"] {
            assert_eq!(parsed["hooks"][stage][0]["matcher"], "^Bash$");
        }

        let second = Grok.install_hooks().await.unwrap_err();
        assert!(matches!(second, InstallHookError::AlreadyInstalled));
    }

    #[rstest]
    fn resume_plan_prefers_session_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let target = ResumeTarget::new("01a18221-a7b8-75e0-b6ef-b2a5cc95d2c9").with_cwd(dir.path());
        let plan = Grok.resume_plan(&target).unwrap().prepare().unwrap();
        assert_eq!(plan.program, "grok");
        assert_eq!(plan.args, ["--resume", "01a18221-a7b8-75e0-b6ef-b2a5cc95d2c9"]);
        assert_eq!(plan.cwd.as_deref(), Some(dir.path()));
    }
}
