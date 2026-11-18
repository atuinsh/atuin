//! Shared install logic for harnesses that register atuin through a Claude-Code-style JSON hook
//! config.
//!
//! TODO(markovejnovic): This is a little bit of legacy slop and should be revisited.

use std::path::Path;

use serde_json::{Map, Value, json};

use super::InstallHookError;

/// Hook lifecycle events atuin registers itself for.
const HOOK_EVENT_TYPES: &[&str] = &["PreToolUse", "PostToolUse", "PostToolUseFailure"];

/// How a hook entry tells the harness to run atuin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookStyle {
    /// A `command` string, run through the harness's shell.
    Shell,
    /// `exec` and `args`, run directly with no shell. Avoids quoting the executable path, which
    /// PowerShell rejects at the start of a command.
    Exec,
}

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
    install_in(config_path, matcher, harness, HookStyle::Shell, Value::Object(Map::new())).await
}

/// [`install`], into `skeleton` when there is no config at `config_path` yet: what the harness
/// wants around the hooks (Copilot's `"version": 1`).
pub async fn install_in(
    config_path: &Path,
    matcher: &str,
    harness: &str,
    style: HookStyle,
    skeleton: Value,
) -> Result<(), InstallHookError> {
    let hook = hook_entry(&std::env::current_exe()?, harness, style)?;

    if let Some(parent) = config_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut root: Value = match tokio::fs::read_to_string(config_path).await {
        Ok(content) => serde_json::from_str(&content)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => skeleton,
        Err(err) => return Err(err.into()),
    };

    let hooks = root
        .as_object_mut()
        .ok_or(InstallHookError::Malformed("root is not an object"))?
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or(InstallHookError::Malformed("`hooks` is not an object"))?;

    if !add_hook_entries(hooks, matcher, harness, &hook)? {
        return Err(InstallHookError::AlreadyInstalled);
    }

    tokio::fs::write(config_path, serde_json::to_string_pretty(&root)?).await?;

    Ok(())
}

/// Replace every event's atuin hooks with one `hook` entry, returning whether `hooks`
/// changed.
///
/// Atuin hooks are removed wherever they sit, so reinstalling after an upgrade or a moved binary
/// never leaves a stale or duplicate hook behind. An entry already as it would be added stays
/// where it is: Codex trusts each hook by where it is, so moving it would move the user's after
/// it and leave them untrusted.
fn add_hook_entries(
    hooks: &mut Map<String, Value>,
    matcher: &str,
    harness: &str,
    hook: &Value,
) -> Result<bool, InstallHookError> {
    let before = hooks.clone();
    for event_type in HOOK_EVENT_TYPES {
        let entries = hooks
            .entry(*event_type)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or(InstallHookError::Malformed("a hook event is not an array"))?;

        let installed = json!({
            "matcher": matcher,
            "hooks": [hook],
        });
        let kept = entries.iter().position(|entry| *entry == installed);
        for (at, entry) in entries.iter_mut().enumerate() {
            if Some(at) == kept {
                continue;
            }
            if let Some(entry_hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                entry_hooks.retain(|hook| !hook_invokes_atuin(hook, harness));
            }
        }
        entries.retain(|entry| {
            entry.get("hooks").and_then(Value::as_array).is_none_or(|hooks| !hooks.is_empty())
        });

        if kept.is_none() {
            entries.push(installed);
        }
    }

    Ok(*hooks != before)
}

/// Build the hook entry that runs `atuin hook <harness>` through `executable`.
fn hook_entry(
    executable: &Path,
    harness: &str,
    style: HookStyle,
) -> Result<Value, InstallHookError> {
    match style {
        HookStyle::Shell => {
            Ok(json!({"type": "command", "command": hook_command(executable, harness)?}))
        }
        HookStyle::Exec => {
            let executable = executable
                .to_str()
                .ok_or_else(|| InstallHookError::NonUtf8Executable(executable.to_owned()))?;
            Ok(json!({"type": "command", "exec": executable, "args": ["hook", harness]}))
        }
    }
}

/// Build the shell command that runs `atuin hook <harness>` through `executable`.
pub(super) fn hook_command(executable: &Path, harness: &str) -> Result<String, InstallHookError> {
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

/// Whether `config_path` already registers `atuin hook <harness>` across every hook lifecycle
/// stage, so an inherited hook from another harness can defer to it without losing events when
/// the native hook has not been installed.
#[must_use]
pub fn has_installed(config_path: &Path, harness: &str) -> bool {
    let Ok(content) = std::fs::read_to_string(config_path) else {
        return false;
    };
    let Ok(root) = serde_json::from_str::<Value>(&content) else {
        return false;
    };
    let Some(hooks) = root.get("hooks").and_then(Value::as_object) else {
        return false;
    };
    let self_cmd = std::env::current_exe().ok().and_then(|exe| hook_command(&exe, harness).ok());
    HOOK_EVENT_TYPES.iter().all(|stage| {
        hooks.get(*stage).and_then(Value::as_array).is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry.get("hooks").and_then(Value::as_array).is_some_and(|cmds| {
                    cmds.iter().any(|cmd| {
                        cmd.get("command").and_then(Value::as_str).is_some_and(|command| {
                            invokes_atuin_hook(command, harness)
                                || self_cmd.as_deref() == Some(command)
                        })
                    })
                })
            })
        })
    })
}

/// Whether `command` runs `atuin hook <harness>`, through any path to the atuin executable.
pub fn invokes_atuin_hook(command: &str, harness: &str) -> bool {
    let Some(parts) = shlex::split(command) else {
        return false;
    };

    is_atuin_hook_argv(parts.iter().map(String::as_str), harness)
}

/// Whether the hook entry runs `atuin hook <harness>`, as a `command` string or as `exec` and
/// `args`.
fn hook_invokes_atuin(hook: &Value, harness: &str) -> bool {
    if let Some(command) = hook.get("command").and_then(Value::as_str) {
        return invokes_atuin_hook(command, harness);
    }

    let Some(executable) = hook.get("exec").and_then(Value::as_str) else {
        return false;
    };
    let args = hook.get("args").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut argv = vec![Some(executable)];
    argv.extend(args.iter().map(Value::as_str));

    argv.into_iter()
        .collect::<Option<Vec<_>>>()
        .is_some_and(|argv| is_atuin_hook_argv(argv, harness))
}

fn is_atuin_hook_argv<'a>(argv: impl IntoIterator<Item = &'a str>, harness: &str) -> bool {
    let argv: Vec<_> = argv.into_iter().collect();
    let [executable, "hook", name] = argv.as_slice() else {
        return false;
    };

    *name == harness
        && executable.rsplit(['/', '\\']).next().is_some_and(|name| {
            name.eq_ignore_ascii_case("atuin") || name.eq_ignore_ascii_case("atuin.exe")
        })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn add_hook_entries_replaces_legacy_commands_without_duplicates() {
        let command = "/opt/atuin/bin/atuin hook claude-code";
        let hook = json!({"type": "command", "command": command});
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

        assert!(add_hook_entries(hooks_map, "^Bash$", "claude-code", &hook).unwrap());
        assert!(!add_hook_entries(hooks_map, "^Bash$", "claude-code", &hook).unwrap());

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

    /// Installed again, atuin's entry stays where it is, ahead of the user's added since.
    #[rstest]
    fn add_hook_entries_keeps_an_installed_entry_in_place() {
        let command = "/opt/atuin/bin/atuin hook codex";
        let hook = json!({"type": "command", "command": command});
        let installed = json!({
            "matcher": "^Bash$",
            "hooks": [{"type": "command", "command": command}],
        });
        let theirs = json!({"matcher": "", "hooks": [{"type": "command", "command": "echo hi"}]});
        let stale = json!({"matcher": "^Bash$", "hooks": [{"type": "command", "command": "atuin hook codex"}]});
        let mut hooks = json!({
            "PreToolUse": [installed, theirs],
            "PostToolUse": [stale, installed, theirs],
            "PostToolUseFailure": [installed],
        });

        assert!(
            add_hook_entries(hooks.as_object_mut().unwrap(), "^Bash$", "codex", &hook).unwrap()
        );
        assert_eq!(
            hooks,
            json!({
                "PreToolUse": [installed, theirs],
                "PostToolUse": [installed, theirs],
                "PostToolUseFailure": [installed],
            })
        );
    }

    /// Reinstalling Copilot's hook as `exec` replaces the quoted `command` that PowerShell rejects.
    #[rstest]
    fn add_hook_entries_replaces_a_command_with_exec() {
        let stale = json!({
            "matcher": "^Bash$",
            "hooks": [{"type": "command", "command": r#""C:\Users\me\atuin.exe" hook copilot"#}],
        });
        let hook =
            hook_entry(Path::new(r"C:\Users\me\atuin.exe"), "copilot", HookStyle::Exec).unwrap();
        let installed = json!({"matcher": "^Bash$", "hooks": [hook]});
        let mut hooks = json!({"PreToolUse": [stale]});
        let hooks_map = hooks.as_object_mut().unwrap();

        assert!(add_hook_entries(hooks_map, "^Bash$", "copilot", &hook).unwrap());
        assert!(!add_hook_entries(hooks_map, "^Bash$", "copilot", &hook).unwrap());

        assert_eq!(hooks["PreToolUse"], json!([installed]));
    }

    #[rstest]
    fn exec_hook_entry_runs_the_executable_without_a_shell() {
        let hook =
            hook_entry(Path::new(r"C:\Program Files\Atuin\atuin.exe"), "copilot", HookStyle::Exec)
                .unwrap();

        assert_eq!(
            hook,
            json!({
                "type": "command",
                "exec": r"C:\Program Files\Atuin\atuin.exe",
                "args": ["hook", "copilot"],
            })
        );
        assert!(hook_invokes_atuin(&hook, "copilot"));
        assert!(!hook_invokes_atuin(&hook, "codex"));
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
