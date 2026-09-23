//! Shared install logic for harnesses that register atuin through a Claude-Code-style JSON hook
//! config.
//!
//! TODO(markovejnovic): This is a little bit of legacy slop and should be revisited.

use std::path::Path;

use serde_json::{Map, Value, json};

use super::InstallHookError;

/// Hook lifecycle events atuin registers itself for.
const HOOK_EVENT_TYPES: &[&str] = &["PreToolUse", "PostToolUse", "PostToolUseFailure"];

/// Merge atuin's command hooks into a Claude-Code-style JSON hook config at `config_path`, leaving
/// the user's existing keys, values, and ordering intact.
///
/// The hooks invoke the running executable by absolute path, since harnesses do not necessarily
/// run them with the user's shell `PATH`.
pub async fn install(
    config_path: &Path,
    matcher: &str,
    harness: &str,
) -> Result<(), InstallHookError> {
    let hook_command = hook_command(&std::env::current_exe()?, harness)?;

    if let Some(parent) = config_path.parent() {
        crate::fs::create_dir_all(parent).await?;
    }

    let mut root: Value = match crate::fs::read_to_string(config_path).await {
        Ok(content) => serde_json::from_str(&content)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(err) => return Err(err.into()),
    };

    let hooks = root
        .as_object_mut()
        .ok_or(InstallHookError::Malformed("root is not an object"))?
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or(InstallHookError::Malformed("`hooks` is not an object"))?;

    if !add_hook_entries(hooks, matcher, harness, &hook_command)? {
        return Err(InstallHookError::AlreadyInstalled);
    }

    crate::fs::write(config_path, serde_json::to_string_pretty(&root)?).await?;

    Ok(())
}

/// Replace every event's atuin hooks with one `hook_command` entry, returning whether `hooks`
/// changed.
///
/// Atuin hooks are removed wherever they sit, so reinstalling after an upgrade or a moved binary
/// never leaves a stale or duplicate hook behind.
fn add_hook_entries(
    hooks: &mut Map<String, Value>,
    matcher: &str,
    harness: &str,
    hook_command: &str,
) -> Result<bool, InstallHookError> {
    let before = hooks.clone();
    for event_type in HOOK_EVENT_TYPES {
        let entries = hooks
            .entry(*event_type)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or(InstallHookError::Malformed("a hook event is not an array"))?;

        for entry in entries.iter_mut() {
            if let Some(entry_hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                entry_hooks.retain(|hook| {
                    !hook
                        .get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|command| invokes_atuin_hook(command, harness))
                });
            }
        }
        entries.retain(|entry| {
            entry.get("hooks").and_then(Value::as_array).is_none_or(|hooks| !hooks.is_empty())
        });

        entries.push(json!({
            "matcher": matcher,
            "hooks": [{"type": "command", "command": hook_command}],
        }));
    }

    Ok(*hooks != before)
}

/// Build the shell command that runs `atuin hook <harness>` through `executable`.
fn hook_command(executable: &Path, harness: &str) -> Result<String, InstallHookError> {
    let executable_str = executable
        .to_str()
        .ok_or_else(|| InstallHookError::NonUtf8Executable(executable.to_owned()))?;

    // shlex quotes for POSIX shells, but a Windows harness may run the hook through cmd.exe, which
    // only understands double quotes.
    #[cfg(windows)]
    let command = format!(r#""{executable_str}" hook {harness}"#);

    #[cfg(not(windows))]
    let command = shlex::try_join([executable_str, "hook", harness]).map_err(|source| {
        InstallHookError::UnquotableExecutable {
            path: executable.to_owned(),
            source,
        }
    })?;

    Ok(command)
}

/// Whether `command` runs `atuin hook <harness>`, through any path to the atuin executable.
fn invokes_atuin_hook(command: &str, harness: &str) -> bool {
    let Some(parts) = shlex::split(command) else {
        return false;
    };

    parts.len() == 3
        && Path::new(&parts[0]).file_name().and_then(|name| name.to_str()).is_some_and(|name| {
            name.eq_ignore_ascii_case("atuin") || name.eq_ignore_ascii_case("atuin.exe")
        })
        && parts[1] == "hook"
        && parts[2] == harness
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn add_hook_entries_replaces_legacy_commands_without_duplicates() {
        let command = "/opt/atuin/bin/atuin hook claude-code";
        let mut hooks = json!({
            "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [
                    {"type": "command", "command": "\"$HOME/.atuin/bin/atuin\" hook claude-code"},
                    {"type": "command", "command": "printf keep-me"},
                ],
            }, {
                "matcher": "^Bash$",
                "hooks": [{"type": "command", "command": "atuin hook claude-code"}],
            }, {
                "matcher": "^Bash$",
                "hooks": [{"type": "command", "command": "atuin hook codex"}],
            }],
        });
        let hooks_map = hooks.as_object_mut().unwrap();

        assert!(add_hook_entries(hooks_map, "^Bash$", "claude-code", command).unwrap());
        assert!(!add_hook_entries(hooks_map, "^Bash$", "claude-code", command).unwrap());

        let installed = json!({
            "matcher": "^Bash$",
            "hooks": [{"type": "command", "command": command}],
        });
        assert_eq!(
            hooks,
            json!({
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{"type": "command", "command": "printf keep-me"}],
                }, {
                    "matcher": "^Bash$",
                    "hooks": [{"type": "command", "command": "atuin hook codex"}],
                }, installed],
                "PostToolUse": [installed],
                "PostToolUseFailure": [installed],
            })
        );
    }

    #[cfg(not(windows))]
    #[rstest]
    #[case::plain("/opt/atuin/bin/atuin", "/opt/atuin/bin/atuin hook codex")]
    #[case::apostrophe("/opt/Atuin's bin/atuin", r#""/opt/Atuin's bin/atuin" hook codex"#)]
    #[case::metacharacters(r#"/opt/"q"/$HOME/atuin"#, r#"'/opt/"q"/$HOME/atuin' hook codex"#)]
    fn hook_command_quotes_posix_executable_paths(
        #[case] executable: &str,
        #[case] expected: &str,
    ) {
        let command = hook_command(Path::new(executable), "codex").unwrap();

        assert_eq!(command, expected);
        assert!(invokes_atuin_hook(&command, "codex"));
    }

    #[cfg(windows)]
    #[rstest]
    fn hook_command_quotes_windows_executable_paths() {
        let command =
            hook_command(Path::new(r"C:\Program Files\Atuin\atuin.exe"), "codex").unwrap();

        assert_eq!(command, r#""C:\Program Files\Atuin\atuin.exe" hook codex"#);
        assert!(invokes_atuin_hook(&command, "codex"));
    }

    #[rstest]
    #[case::bare("atuin hook codex", true)]
    #[case::absolute("'/opt/atuin/bin/atuin' hook codex", true)]
    #[case::windows_suffix("'/opt/atuin/bin/atuin.exe' hook codex", true)]
    #[case::wrapper("'/opt/atuin/bin/atuin.sh' hook codex", false)]
    #[case::backup("'/opt/atuin/bin/atuin.backup' hook codex", false)]
    #[case::different_harness("atuin hook claude-code", false)]
    fn recognizes_only_atuin_executables(#[case] command: &str, #[case] expected: bool) {
        assert_eq!(invokes_atuin_hook(command, "codex"), expected);
    }
}
