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
    let registration = HookRegistration::new(matcher, harness)?;

    if let Some(parent) = config_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut root: Value = match tokio::fs::read_to_string(config_path).await {
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

    if !registration.add_entries(hooks)? {
        return Err(InstallHookError::AlreadyInstalled);
    }

    tokio::fs::write(config_path, serde_json::to_string_pretty(&root)?).await?;

    Ok(())
}

/// Keeps the event-specific details together while registering Atuin's hooks.
struct HookRegistration<'a> {
    matcher: &'a str,
    harness: &'a str,
    command: String,
}

impl<'a> HookRegistration<'a> {
    fn new(matcher: &'a str, harness: &'a str) -> Result<Self, InstallHookError> {
        let executable = std::env::current_exe()?;
        let command = Self::command_for(&executable, harness)?;

        Ok(Self {
            matcher,
            harness,
            command,
        })
    }

    /// Update one Atuin hook per event, preserving its matcher and settings.
    /// Prefer an existing current command so reinstalling does not move it past unrelated hooks
    /// and invalidate Codex's position-based trust. Otherwise, update the first legacy command.
    /// Remove duplicate Atuin hooks and return whether `hooks` changed.
    fn add_entries(&self, hooks: &mut Map<String, Value>) -> Result<bool, InstallHookError> {
        let before = hooks.clone();
        for event_type in HOOK_EVENT_TYPES {
            let entries = hooks
                .entry(*event_type)
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .ok_or(InstallHookError::Malformed("a hook event is not an array"))?;

            let current_entry = entries.iter().position(|entry| {
                entry.get("hooks").and_then(Value::as_array).is_some_and(|entry_hooks| {
                    entry_hooks.iter().any(|hook| {
                        hook.get("command").and_then(Value::as_str) == Some(self.command.as_str())
                    })
                })
            });
            let mut found_hook = false;
            for (at, entry) in entries.iter_mut().enumerate() {
                if let Some(entry_hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                    entry_hooks.retain_mut(|hook| {
                        let Some(command) = hook.get("command").and_then(Value::as_str) else {
                            return true;
                        };
                        if command != self.command && !self.invokes_atuin_hook(command) {
                            return true;
                        }
                        if found_hook
                            || current_entry.is_some_and(|kept| at != kept)
                            || (current_entry.is_some() && command != self.command)
                        {
                            return false;
                        }

                        found_hook = true;
                        if let Some(hook) = hook.as_object_mut() {
                            hook.insert("command".to_owned(), Value::String(self.command.clone()));
                            if cfg!(windows) {
                                for field in ["commandWindows", "command_windows"] {
                                    let owned = hook
                                        .get(field)
                                        .and_then(Value::as_str)
                                        .is_some_and(|command| self.invokes_atuin_hook(command));
                                    if owned {
                                        hook.insert(
                                            field.to_owned(),
                                            Value::String(self.command.clone()),
                                        );
                                    }
                                }
                            }
                            hook.remove("args");
                        }
                        true
                    });
                }
            }
            entries.retain(|entry| {
                entry.get("hooks").and_then(Value::as_array).is_none_or(|hooks| !hooks.is_empty())
            });

            if !found_hook {
                entries.push(json!({
                    "matcher": self.matcher,
                    "hooks": [{"type": "command", "command": self.command}],
                }));
            }
        }

        Ok(*hooks != before)
    }

    /// Build the shell command that runs `atuin hook <harness>` through `executable`.
    fn command_for(executable: &Path, harness: &str) -> Result<String, InstallHookError> {
        let executable_str = executable
            .to_str()
            .ok_or_else(|| InstallHookError::NonUtf8Executable(executable.to_owned()))?;

        // shlex quotes for POSIX shells, but a Windows harness may run the hook through cmd.exe,
        // which only understands double quotes.
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

    fn invokes_atuin_hook(&self, command: &str) -> bool {
        invokes_atuin_hook(command, self.harness)
    }
}

/// Whether `command` runs `atuin hook <harness>`, through any path to the atuin executable.
pub fn invokes_atuin_hook(command: &str, harness: &str) -> bool {
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
    fn add_hook_entries_preserves_settings_and_removes_duplicate_atuin_hooks() {
        let command = "/opt/atuin/bin/atuin hook claude-code";
        let registration = HookRegistration {
            matcher: "^Bash$",
            harness: "claude-code",
            command: command.to_owned(),
        };
        let mut hooks = json!({
            "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [
                    {
                        "type": "command",
                        "command": "\"$HOME/.atuin/bin/atuin\" hook claude-code",
                        "timeout": 5_000,
                        "shell": "bash",
                    },
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

        assert!(registration.add_entries(hooks_map).unwrap());
        assert!(!registration.add_entries(hooks_map).unwrap());

        let installed = json!({
            "matcher": "^Bash$",
            "hooks": [{"type": "command", "command": command}],
        });
        assert_eq!(
            hooks,
            json!({
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [
                        {
                            "type": "command",
                            "command": command,
                            "timeout": 5_000,
                            "shell": "bash",
                        },
                        {"type": "command", "command": "printf keep-me"},
                    ],
                }, {
                    "matcher": "^Bash$",
                    "hooks": [{"type": "command", "command": "atuin hook codex"}],
                }],
                "PostToolUse": [installed],
                "PostToolUseFailure": [installed],
            })
        );
    }

    #[rstest]
    #[case::old_atuin(r#""C:/Old/atuin.exe" hook codex"#, true)]
    #[case::current_atuin(r#""C:/New/atuin.exe" hook codex"#, true)]
    #[case::other_command("echo other", false)]
    #[case::other_harness(r#""C:/Old/atuin.exe" hook claude-code"#, false)]
    fn reinstall_updates_only_owned_windows_overrides(
        #[case] windows_command: &str,
        #[case] owned: bool,
        #[values("commandWindows", "command_windows")] field: &str,
    ) {
        let command = r#""C:/New/atuin.exe" hook codex"#;
        let registration = HookRegistration {
            matcher: "^Bash$",
            harness: "codex",
            command: command.to_owned(),
        };
        let mut handler = json!({
            "type": "command", "command": "atuin hook codex", "timeout": 30,
        });
        handler[field] = json!(windows_command);
        let mut hooks = json!({"PreToolUse": [{"matcher": "Bash|Shell", "hooks": [handler]}]});
        assert!(registration.add_entries(hooks.as_object_mut().unwrap()).unwrap());
        let installed = &hooks["PreToolUse"][0]["hooks"][0];
        let expected = if cfg!(windows) && owned {
            command
        } else {
            windows_command
        };
        assert_eq!(installed["command"], command);
        assert_eq!(installed[field], expected);
        assert_eq!(installed["timeout"], 30);
        assert_eq!(hooks["PreToolUse"][0]["matcher"], "Bash|Shell");
        assert!(!registration.add_entries(hooks.as_object_mut().unwrap()).unwrap());
    }

    /// Installed again, atuin's entry stays where it is, ahead of the user's added since.
    #[rstest]
    fn add_hook_entries_keeps_an_installed_entry_in_place() {
        let command = "/opt/atuin/bin/atuin hook codex";
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

        let registration = HookRegistration {
            matcher: "^Bash$",
            harness: "codex",
            command: command.to_owned(),
        };
        assert!(registration.add_entries(hooks.as_object_mut().unwrap()).unwrap());
        assert!(!registration.add_entries(hooks.as_object_mut().unwrap()).unwrap());
        assert_eq!(
            hooks,
            json!({
                "PreToolUse": [installed, theirs],
                "PostToolUse": [installed, theirs],
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
        let command = HookRegistration::command_for(Path::new(executable), "codex").unwrap();

        assert_eq!(command, expected);
        assert!(
            HookRegistration {
                matcher: "^Bash$",
                harness: "codex",
                command: command.clone(),
            }
            .invokes_atuin_hook(&command)
        );
    }

    #[cfg(windows)]
    #[rstest]
    fn hook_command_quotes_windows_executable_paths() {
        let command =
            HookRegistration::command_for(Path::new(r"C:\Program Files\Atuin\atuin.exe"), "codex")
                .unwrap();

        assert_eq!(command, r#""C:\Program Files\Atuin\atuin.exe" hook codex"#);
        assert!(
            HookRegistration {
                matcher: "^Bash$",
                harness: "codex",
                command: command.clone(),
            }
            .invokes_atuin_hook(&command)
        );
    }

    #[rstest]
    #[case::bare("atuin hook codex", true)]
    #[case::absolute("'/opt/atuin/bin/atuin' hook codex", true)]
    #[case::windows_suffix("'/opt/atuin/bin/atuin.exe' hook codex", true)]
    #[case::wrapper("'/opt/atuin/bin/atuin.sh' hook codex", false)]
    #[case::backup("'/opt/atuin/bin/atuin.backup' hook codex", false)]
    #[case::different_harness("atuin hook claude-code", false)]
    fn recognizes_only_atuin_executables(#[case] command: &str, #[case] expected: bool) {
        let registration = HookRegistration {
            matcher: "^Bash$",
            harness: "codex",
            command: "atuin hook codex".to_owned(),
        };
        assert_eq!(registration.invokes_atuin_hook(command), expected);
    }
    #[rstest]
    #[case::pre_tool_use("PreToolUse")]
    #[case::post_tool_use("PostToolUse")]
    #[case::post_tool_use_failure("PostToolUseFailure")]
    fn current_custom_hook_is_preferred_to_an_earlier_legacy_hook(
        #[case] event: &str,
        #[values("codex", "claude-code")] harness: &str,
    ) {
        let command = format!("/opt/atuin/bin/atuin hook {harness}");
        let legacy = format!("atuin hook {harness}");
        let registration = HookRegistration {
            matcher: "^Bash$",
            harness,
            command: command.clone(),
        };
        let unrelated =
            json!({"matcher": "", "hooks": [{"type": "command", "command": "echo before"}]});
        let current = json!({
            "matcher": "Bash|Shell",
            "hooks": [
                {"type": "command", "command": "echo inside"},
                {"type": "command", "command": command, "timeout": 42, "shell": "bash"},
            ],
            "custom": true,
        });
        let after = json!({"matcher": "", "hooks": [{"type": "command", "command": "echo after"}]});
        let mut hooks = Map::new();
        hooks.insert(
            event.to_owned(),
            json!([
                {"matcher": "old", "hooks": [
                    {"type": "command", "command": legacy, "timeout": 5},
                    {"type": "command", "command": "echo keep"},
                ]},
                unrelated, current, after,
            ]),
        );

        assert!(registration.add_entries(&mut hooks).unwrap());
        assert_eq!(
            hooks[event],
            json!([
                {"matcher": "old", "hooks": [{"type": "command", "command": "echo keep"}]},
                unrelated, current, after,
            ])
        );
        assert!(!registration.add_entries(&mut hooks).unwrap());
    }

    #[rstest]
    fn current_hook_inside_a_mixed_group_keeps_its_settings() {
        let command = "/opt/atuin/bin/atuin hook codex";
        let registration = HookRegistration {
            matcher: "^Bash$",
            harness: "codex",
            command: command.to_owned(),
        };
        let current = json!({"type": "command", "command": command, "timeout": 42});
        let unrelated = json!({"type": "command", "command": "echo keep"});
        let mut hooks = json!({"PreToolUse": [{"matcher": "Shell", "hooks": [
            {"type": "command", "command": "atuin hook codex", "timeout": 5},
            current, unrelated,
        ]}]});

        assert!(registration.add_entries(hooks.as_object_mut().unwrap()).unwrap());
        assert_eq!(
            hooks["PreToolUse"],
            json!([{ "matcher": "Shell", "hooks": [current, unrelated] }])
        );
        assert!(!registration.add_entries(hooks.as_object_mut().unwrap()).unwrap());
    }

    #[rstest]
    #[tokio::test]
    async fn installing_twice_does_not_rewrite_the_config() {
        let temp = tempfile::tempdir().unwrap();
        let config_path = temp.path().join("settings.json");
        tokio::fs::write(&config_path, r#"{"other":true}"#).await.unwrap();

        install(&config_path, "^Bash$", "codex").await.unwrap();
        let content = format!("{}\n", tokio::fs::read_to_string(&config_path).await.unwrap());
        tokio::fs::write(&config_path, &content).await.unwrap();
        let repeated = install(&config_path, "^Bash$", "codex").await;

        assert!(matches!(repeated, Err(InstallHookError::AlreadyInstalled)));
        assert_eq!(tokio::fs::read_to_string(&config_path).await.unwrap(), content);
        let config: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(config["other"], true);
    }
}
