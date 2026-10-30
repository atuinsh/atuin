//! Kiro CLI (`kiro-cli`): reading its sessions, installing its hooks and resuming.
//!
//! Read-only: atuin reads Kiro's sessions but never writes them, so a session is restored,
//! caught up, switched, forked or continued into Kiro by nothing here
//! ([`Harness::writes_transcripts`]); a Kiro copy on this machine resumes as it is. What the
//! reader knows of the format was verified against Kiro CLI 2.36.0 (see the session module).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::rehydrate::{RehydrateError, RehydrateSession};
use super::resume::{self, CwdRequirement, ResumeError, ResumePlan, ResumeTarget};
use super::{Harness, InstallHookError, json_hooks};
use crate::dirs::home_dir;
use crate::env::var_nonempty;
use crate::path::PathBufExt as _;

pub mod session;

/// Kiro's home directory: `$KIRO_HOME` where Kiro itself honours it, else `~/.kiro`.
pub(crate) fn kiro_home() -> PathBuf {
    home_from(var_nonempty("KIRO_HOME").as_deref(), &home_dir())
}

/// Kiro ignores a `KIRO_HOME` that is relative, a filesystem root, or the home directory `home`
/// or one of its ancestors, and falls back to `~/.kiro`.
fn home_from(var: Option<&OsStr>, home: &Path) -> PathBuf {
    let Some(dir) = var.map(Path::new) else {
        return home.join(".kiro");
    };
    if !dir.is_absolute() || dir.parent().is_none() || home.starts_with(dir) {
        return home.join(".kiro");
    }
    dir.to_path_buf()
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Kiro;

impl Harness for Kiro {
    fn name(&self) -> &'static str {
        "kiro"
    }

    fn alias_names(&self) -> &'static [&'static str] {
        &["kiro-cli"]
    }

    fn writes_transcripts(&self) -> bool {
        false
    }

    /// Kiro reads every `*.json` in `$KIRO_HOME/hooks` at session start (a running Kiro needs a
    /// restart). The file is atuin's own, so it is written whole.
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
        let config_path = kiro_home().joined("hooks").joined("atuin.json");
        install_hooks_in(&config_path, &std::env::current_exe()?).await?;
        Ok(config_path)
    }

    /// `kiro-cli chat --resume-id <id>`. Kiro finds the session in whichever bucket holds it,
    /// wherever it is run from, and goes on in that session's own directory.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError> {
        let args = ["chat".to_owned(), "--resume-id".to_owned(), target.id.clone()];
        Ok(resume::plan(target, CwdRequirement::None, "kiro-cli", args))
    }

    async fn locate(&self, id: &str) -> Option<PathBuf> {
        let (root, id) = (session::default_root(), id.to_owned());
        resume::blocking(move || session::locate(&root, &id)).await
    }

    async fn rehydrate(&self, _session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
        Err(RehydrateError::Unsupported("Kiro"))
    }
}

/// Write atuin's hooks to `config_path` in Kiro's hook-file format (`{"version": "v1", "hooks":
/// [...]}`): `PreToolUse` and `PostToolUse` on the shell tool, each running `atuin hook kiro`
/// through `executable`. Kiro has no `PostToolUseFailure`: a failed command still gets
/// `PostToolUse`, with its exit code in the response. The matcher names `execute_bash`, which
/// also matches Kiro's shell on Windows (`execute_pwsh`).
async fn install_hooks_in(config_path: &Path, executable: &Path) -> Result<(), InstallHookError> {
    let command = json_hooks::hook_command(executable, "kiro")?;
    let entry = |name: &str, trigger: &str| {
        json!({
            "name": name,
            "trigger": trigger,
            "matcher": "execute_bash",
            "action": {"type": "command", "command": command},
        })
    };
    let hooks = json!({
        "version": "v1",
        "hooks": [entry("atuin-pre", "PreToolUse"), entry("atuin-post", "PostToolUse")],
    });
    let installed = match tokio::fs::read_to_string(config_path).await {
        Ok(text) => serde_json::from_str::<Value>(&text).ok() == Some(hooks.clone()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(err.into()),
    };
    if installed {
        return Err(InstallHookError::AlreadyInstalled);
    }
    if let Some(parent) = config_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(config_path, serde_json::to_string_pretty(&hooks)?).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn resumes_by_id_from_anywhere() {
        let target =
            ResumeTarget::new("sess_7c362b91-6e72-4c10-be1f-76a31a8d5f48").with_cwd("/work");
        let plan = Kiro.resume_plan(&target).unwrap();
        assert_eq!(plan.program, "kiro-cli");
        assert_eq!(plan.args, ["chat", "--resume-id", "sess_7c362b91-6e72-4c10-be1f-76a31a8d5f48"]);
        assert_eq!(plan.cwd_requirement, CwdRequirement::None);
    }

    #[rstest]
    fn is_found_by_its_names() {
        for name in ["kiro", "kiro-cli"] {
            let harness = super::super::AnyHarness::from_name(name).unwrap();
            assert_eq!(harness.name(), "kiro", "{name}");
        }
    }

    #[rstest]
    #[tokio::test]
    async fn never_writes_a_transcript() {
        assert!(!Kiro.writes_transcripts());
        let session = RehydrateSession {
            id: "sess_x".to_owned(),
            title: None,
            cwd: PathBuf::from("/work"),
            original_cwd: None,
            git_branch: None,
            model: None,
            started_at: time::OffsetDateTime::UNIX_EPOCH,
            messages: Vec::new(),
            fork_of: None,
        };
        assert!(matches!(Kiro.rehydrate(&session).await, Err(RehydrateError::Unsupported("Kiro"))));
    }

    /// Kiro's own rule: `KIRO_HOME` must be absolute, below a root, and not the home directory
    /// or above it.
    #[rstest]
    #[case::unset(None, "/home/u/.kiro")]
    #[case::elsewhere(Some("/srv/kiro"), "/srv/kiro")]
    #[case::under_home(Some("/home/u/kiro-home"), "/home/u/kiro-home")]
    #[case::relative(Some("kiro"), "/home/u/.kiro")]
    #[case::root(Some("/"), "/home/u/.kiro")]
    #[case::home(Some("/home/u"), "/home/u/.kiro")]
    #[case::above_home(Some("/home"), "/home/u/.kiro")]
    fn home_follows_kiros_rule(#[case] var: Option<&str>, #[case] want: &str) {
        let home = Path::new("/home/u");
        assert_eq!(home_from(var.map(OsStr::new), home), Path::new(want));
    }

    #[rstest]
    #[tokio::test]
    async fn install_writes_kiros_hook_file_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks").joined("atuin.json");
        let exe = Path::new("/opt/atuin/bin/atuin");
        install_hooks_in(&path, exe).await.unwrap();
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["version"], "v1");
        let hooks = file["hooks"].as_array().unwrap();
        assert_eq!(
            hooks
                .iter()
                .map(|h| (h["name"].as_str().unwrap(), h["trigger"].as_str().unwrap()))
                .collect::<Vec<_>>(),
            [("atuin-pre", "PreToolUse"), ("atuin-post", "PostToolUse")]
        );
        for hook in hooks {
            assert_eq!(hook["matcher"], "execute_bash");
            assert_eq!(hook["action"]["type"], "command");
            assert_eq!(hook["action"]["command"], "/opt/atuin/bin/atuin hook kiro");
        }
        assert!(matches!(
            install_hooks_in(&path, exe).await,
            Err(InstallHookError::AlreadyInstalled)
        ));
        // A moved binary is a new file.
        install_hooks_in(&path, Path::new("/usr/local/bin/atuin")).await.unwrap();
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["hooks"][0]["action"]["command"], "/usr/local/bin/atuin hook kiro");
    }
}
