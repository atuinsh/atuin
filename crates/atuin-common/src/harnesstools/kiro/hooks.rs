//! Atuin's hook file for Kiro CLI: `$KIRO_HOME/hooks/atuin.json`.
//!
//! Kiro's V3 engine loads every `*.json` in its user hooks directory when a session starts, and
//! runs a hook's command with the event on stdin in Claude Code's shape (verified against Kiro
//! CLI 2.36.0 by writing this file and watching both entries fire around a shell command). The
//! file is atuin's own, with no user entries to keep, so a reinstall writes it whole.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::super::{InstallHookError, json_hooks};
use crate::path::PathBufExt as _;

/// What Kiro runs the hook as: `atuin hook kiro`.
const HARNESS: &str = "kiro";

/// Kiro's shell tool on macOS and Linux. A matcher on it also matches `execute_pwsh`, the
/// Windows one.
const MATCHER: &str = "execute_bash";

/// Write atuin's hooks to `<home>/hooks/atuin.json`, running `executable` (atuin, by absolute
/// path: Kiro doesn't run hooks with the user's `PATH`), and return that path.
/// [`InstallHookError::AlreadyInstalled`] when the file already says exactly this, so a rerun
/// after an upgrade or a moved binary rewrites it and nothing else does.
pub async fn install(home: &Path, executable: &Path) -> Result<PathBuf, InstallHookError> {
    let path = home.join("hooks").joined("atuin.json");
    let wanted = hook_file(&json_hooks::hook_command(executable, HARNESS)?);

    match tokio::fs::read_to_string(&path).await {
        // Compared as JSON, so a user's reformatting doesn't count as a change.
        Ok(existing) if serde_json::from_str::<Value>(&existing).ok() == Some(wanted.clone()) => {
            return Err(InstallHookError::AlreadyInstalled);
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, serde_json::to_string_pretty(&wanted)? + "\n").await?;

    Ok(path)
}

/// Kiro's "v2 hook file": one entry each for the start and the end of a shell command. There is
/// no `PostToolUseFailure`; a failed command gets `PostToolUse` with its exit code in the
/// response.
fn hook_file(command: &str) -> Value {
    let entry = |name: &str, trigger: &str| {
        json!({
            "name": name,
            "trigger": trigger,
            "matcher": MATCHER,
            "action": {"type": "command", "command": command},
        })
    };
    json!({
        "version": "v1",
        "hooks": [entry("atuin-pre", "PreToolUse"), entry("atuin-post", "PostToolUse")],
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[rstest]
    #[tokio::test]
    async fn writes_both_entries_for_the_shell_tool() {
        let home = tempfile::tempdir().unwrap();

        let path = install(home.path(), Path::new("/opt/atuin/bin/atuin")).await.unwrap();

        assert_eq!(path, home.path().join("hooks").joined("atuin.json"));
        // Quoted as the platform's shell wants it (double quotes on Windows).
        let command = json_hooks::hook_command(Path::new("/opt/atuin/bin/atuin"), HARNESS).unwrap();
        let action = json!({"type": "command", "command": command});
        assert_eq!(
            read_json(&path),
            json!({
                "version": "v1",
                "hooks": [
                    {"name": "atuin-pre", "trigger": "PreToolUse", "matcher": "execute_bash", "action": action},
                    {"name": "atuin-post", "trigger": "PostToolUse", "matcher": "execute_bash", "action": action},
                ],
            })
        );
    }

    #[rstest]
    #[tokio::test]
    async fn a_second_install_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let atuin = Path::new("/opt/atuin/bin/atuin");
        let path = install(home.path(), atuin).await.unwrap();
        let written = std::fs::read_to_string(&path).unwrap();

        assert!(matches!(
            install(home.path(), atuin).await,
            Err(InstallHookError::AlreadyInstalled)
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
    }

    /// The user's formatting of the same hooks is still the same hooks.
    #[rstest]
    #[tokio::test]
    async fn a_reformatted_file_counts_as_installed() {
        let home = tempfile::tempdir().unwrap();
        let atuin = Path::new("/opt/atuin/bin/atuin");
        let path = install(home.path(), atuin).await.unwrap();
        let compact = serde_json::to_string(&read_json(&path)).unwrap();
        std::fs::write(&path, &compact).unwrap();

        assert!(matches!(
            install(home.path(), atuin).await,
            Err(InstallHookError::AlreadyInstalled)
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), compact);
    }

    /// A moved binary, or a file edited by hand, is written over whole: the file is atuin's.
    #[rstest]
    #[case::moved_binary(r#"{"version":"v1","hooks":[{"name":"atuin-pre","trigger":"PreToolUse","matcher":"execute_bash","action":{"type":"command","command":"/old/atuin hook kiro"}}]}"#)]
    #[case::edited_by_hand(r#"{"version":"v1","hooks":[{"name":"mine","trigger":"Stop","action":{"type":"command","command":"say done"}}]}"#)]
    #[case::not_json("not json")]
    #[tokio::test]
    async fn an_outdated_file_is_rewritten(#[case] existing: &str) {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("hooks").joined("atuin.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, existing).unwrap();

        install(home.path(), Path::new("/opt/atuin/bin/atuin")).await.unwrap();

        let hooks = read_json(&path)["hooks"].as_array().unwrap().clone();
        assert_eq!(hooks.len(), 2);
        let command = json_hooks::hook_command(Path::new("/opt/atuin/bin/atuin"), HARNESS).unwrap();
        for hook in hooks {
            assert_eq!(hook["action"]["command"], command);
        }
    }

    #[cfg(not(windows))]
    #[rstest]
    #[tokio::test]
    async fn quotes_the_executable_path() {
        let home = tempfile::tempdir().unwrap();

        let path = install(home.path(), Path::new("/opt/Atuin's bin/atuin")).await.unwrap();

        let hooks = read_json(&path)["hooks"].as_array().unwrap().clone();
        assert_eq!(hooks[0]["action"]["command"], r#""/opt/Atuin's bin/atuin" hook kiro"#);
    }
}
