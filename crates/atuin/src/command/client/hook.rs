use std::io::Read;
use std::path::{Path, PathBuf};

use atuin_client::settings::Settings;
use atuin_common::utils::home_dir;
use clap::{Parser, Subcommand};
use eyre::{Context, Result, bail};
use serde_json::Value;

use super::history;

mod event;
mod wire;

use event::HookEvent;

const HOOK_EVENT_TYPES: &[&str] = &["PreToolUse", "PostToolUse", "PostToolUseFailure"];
const PI_EXTENSION_SOURCE: &str = include_str!("../../../contrib/pi/atuin.ts");

enum InstallKind {
    JsonHooks {
        config_path: &'static [&'static str],
        /// Agent name passed to `atuin hook <agent>`
        hook_agent: &'static str,
        matcher: &'static str,
    },
    PiExtension {
        extension_path: &'static [&'static str],
    },
}

struct AgentSpec {
    aliases: &'static [&'static str],
    actor_name: &'static str,
    install_kind: InstallKind,
}

const CLAUDE_CODE: AgentSpec = AgentSpec {
    aliases: &["claude-code", "claude"],
    actor_name: "claude-code",
    install_kind: InstallKind::JsonHooks {
        config_path: &[".claude", "settings.json"],
        hook_agent: "claude-code",
        matcher: "Bash",
    },
};

const CODEX: AgentSpec = AgentSpec {
    aliases: &["codex"],
    actor_name: "codex",
    install_kind: InstallKind::JsonHooks {
        config_path: &[".codex", "hooks.json"],
        hook_agent: "codex",
        matcher: "^Bash$",
    },
};

const PI: AgentSpec = AgentSpec {
    aliases: &["pi"],
    actor_name: "pi",
    install_kind: InstallKind::PiExtension {
        extension_path: &[".pi", "agent", "extensions", "atuin.ts"],
    },
};

const AGENTS: &[&AgentSpec] = &[&CLAUDE_CODE, &CODEX, &PI];

struct Agent(&'static AgentSpec);

impl Agent {
    fn from_name(name: &str) -> Result<Self> {
        AGENTS
            .iter()
            .copied()
            .find(|spec| spec.aliases.contains(&name))
            .map(Self)
            .ok_or_else(|| {
                eyre::eyre!("unknown agent: {name}. Supported agents: claude-code, codex, pi")
            })
    }

    fn actor_name(&self) -> &'static str {
        self.0.actor_name
    }

    fn path(path: &'static [&'static str]) -> PathBuf {
        path.iter()
            .fold(home_dir(), |path, segment| path.join(segment))
    }

    fn install_kind(&self) -> &InstallKind {
        &self.0.install_kind
    }
}

#[derive(Subcommand, Debug)]
enum Action {
    /// Install hooks for an AI agent to capture commands in atuin history
    Install {
        /// Agent to install hooks for (e.g., "claude-code")
        #[arg(value_name = "AGENT")]
        agent: String,
    },
}

#[derive(Parser, Debug)]
#[command(infer_subcommands = true, args_conflicts_with_subcommands = true)]
pub struct Cmd {
    #[command(subcommand)]
    action: Option<Action>,

    /// Which agent's hook format to parse (e.g., "claude-code")
    #[arg(value_name = "AGENT", hide = true)]
    agent: Option<String>,
}

impl Cmd {
    pub async fn run(self, settings: &Settings) -> Result<()> {
        match (self.action, self.agent) {
            (Some(Action::Install { agent }), None) => install(&agent),
            (None, Some(agent)) => handle(&agent, settings).await,
            (None, None) => {
                bail!("expected `atuin hook <agent>` or `atuin hook install <agent>`");
            }
            (Some(_), Some(_)) => {
                bail!("hook action cannot be combined with a positional agent");
            }
        }
    }
}

fn id_file_path(tool_use_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("atuin-hook-{tool_use_id}"))
}

async fn handle(agent_name: &str, settings: &Settings) -> Result<()> {
    let agent = Agent::from_name(agent_name)?;

    if matches!(agent.install_kind(), InstallKind::PiExtension { .. }) {
        bail!("`atuin hook pi` is not supported. Use `atuin hook install pi` and reload pi.");
    }

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    if input.trim().is_empty() {
        return Ok(());
    }

    match HookEvent::from_json_str(&input)? {
        Some(HookEvent::Start {
            command,
            intent,
            tool_use_id,
        }) => {
            if let Some(history_id) = history::start_history_entry(
                settings,
                &command,
                Some(agent.actor_name()),
                intent.as_deref(),
            )
            .await?
            {
                std::fs::write(id_file_path(&tool_use_id), &history_id)?;
            }
        }
        Some(HookEvent::End { tool_use_id, exit }) => {
            let id_path = id_file_path(&tool_use_id);

            if let Ok(history_id) = std::fs::read_to_string(&id_path) {
                let history_id = history_id.trim();
                if !history_id.is_empty() {
                    let _ = history::end_history_entry(settings, history_id, exit, None).await;
                }
                let _ = std::fs::remove_file(&id_path);
            }
        }
        None => {}
    }

    Ok(())
}

fn install(agent_name: &str) -> Result<()> {
    let agent = Agent::from_name(agent_name)?;

    match agent.install_kind() {
        InstallKind::JsonHooks {
            config_path,
            hook_agent: _,
            matcher: _,
        } => {
            let config_path = Agent::path(config_path);

            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let mut root: Value = if config_path.exists() {
                let content = std::fs::read_to_string(&config_path)?;
                serde_json::from_str(&content)?
            } else {
                Value::Object(serde_json::Map::new())
            };

            let hooks = root
                .as_object_mut()
                .ok_or_else(|| eyre::eyre!("config is not a JSON object"))?
                .entry("hooks")
                .or_insert_with(|| Value::Object(serde_json::Map::new()));

            let hook_command = resolve_hook_command(&agent)?;
            add_hook_entries(hooks, &agent, &hook_command)?;

            let content = serde_json::to_string_pretty(&root)?;
            std::fs::write(&config_path, content)?;

            eprintln!(
                "\nAtuin hooks installed for {}. Config: {}\nHook command: {hook_command}",
                agent.actor_name(),
                config_path.display()
            );
        }
        InstallKind::PiExtension { extension_path } => {
            let extension_path = Agent::path(extension_path);

            if let Some(parent) = extension_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let already_installed = std::fs::read_to_string(&extension_path)
                .is_ok_and(|existing| existing == PI_EXTENSION_SOURCE);

            if already_installed {
                eprintln!("pi extension: already installed, skipping");
            } else {
                std::fs::write(&extension_path, PI_EXTENSION_SOURCE)?;
                eprintln!("pi extension: installed atuin extension");
            }

            eprintln!(
                "\nAtuin extension installed for {}. Extension: {}\nReload pi with `/reload` or restart pi.",
                agent.actor_name(),
                extension_path.display()
            );
        }
    }

    Ok(())
}

/// Resolve the absolute path of this `atuin` binary and build the hook command string.
///
/// Agent runtimes often spawn hooks with a minimal PATH (and default shells that never ran
/// `atuin init`), so a bare `atuin ...` command fails with exit 127. Pinning to
/// `current_exe()` matches how the daemon is spawned.
fn resolve_hook_command(agent: &Agent) -> Result<String> {
    let InstallKind::JsonHooks { hook_agent, .. } = agent.install_kind() else {
        bail!("agent does not use JSON hooks");
    };

    let exe =
        std::env::current_exe().wrap_err("could not locate atuin executable for hook install")?;
    Ok(format_hook_command(&exe, hook_agent))
}

fn format_hook_command(exe: &Path, hook_agent: &str) -> String {
    format!("{} hook {hook_agent}", quote_exe(exe))
}

/// Quote an executable path for the platform shell that will run the hook.
///
/// Agent hook commands are executed by the host shell, so quoting must match it:
/// POSIX (single-quote) rules on Unix and `cmd.exe` (double-quote) rules on Windows.
#[cfg(windows)]
fn quote_exe(exe: &Path) -> String {
    let raw = exe.to_string_lossy();
    if raw.contains(|c: char| c.is_whitespace()) {
        // cmd.exe does not understand POSIX single quotes; wrap in double quotes.
        format!("\"{raw}\"")
    } else {
        raw.into_owned()
    }
}

#[cfg(not(windows))]
fn quote_exe(exe: &Path) -> String {
    shlex::try_quote(exe.to_string_lossy().as_ref()).map_or_else(
        |_| format!("'{}'", exe.display()),
        std::borrow::Cow::into_owned,
    )
}

/// True when `command` is an Atuin-managed hook entry for `hook_agent`
/// (`atuin hook <agent>` or an absolute path to an `atuin` binary with the same args).
fn is_managed_hook_command(command: &str, hook_agent: &str) -> bool {
    let bare = format!("atuin hook {hook_agent}");
    if command == bare {
        return true;
    }

    let suffix = format!(" hook {hook_agent}");
    let Some(binary) = command.strip_suffix(&suffix) else {
        return false;
    };

    let binary = binary
        .trim()
        .trim_matches('\'')
        .trim_matches('"')
        .trim_end_matches(['\\', '/']);

    let name = binary.rsplit(['/', '\\']).next().unwrap_or(binary);

    name == "atuin" || name.eq_ignore_ascii_case("atuin.exe")
}

fn add_hook_entries(hooks: &mut Value, agent: &Agent, hook_command: &str) -> Result<()> {
    let InstallKind::JsonHooks {
        config_path: _,
        hook_agent,
        matcher,
    } = agent.install_kind()
    else {
        bail!("agent does not use JSON hooks");
    };

    for event_type in HOOK_EVENT_TYPES {
        let event_hooks = hooks
            .as_object_mut()
            .ok_or_else(|| eyre::eyre!("hooks is not a JSON object"))?
            .entry(*event_type)
            .or_insert_with(|| Value::Array(Vec::new()));

        let arr = event_hooks
            .as_array_mut()
            .ok_or_else(|| eyre::eyre!("hooks.{event_type} is not an array"))?;

        // Rewrite every Atuin-managed entry to the absolute command and drop any
        // duplicates so reinstalling never leaves stale bare `atuin hook ...` lines.
        let mut seen_managed = false;
        for entry in arr.iter_mut() {
            let Some(hooks_arr) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };

            hooks_arr.retain_mut(|hook| {
                let Some(command) = hook.get("command").and_then(Value::as_str) else {
                    return true;
                };

                if !is_managed_hook_command(command, hook_agent) {
                    return true;
                }

                if seen_managed {
                    // A managed entry already survived; drop this duplicate.
                    return false;
                }

                seen_managed = true;
                if command != hook_command {
                    hook["command"] = Value::String(hook_command.to_string());
                }
                true
            });
        }

        // Prune wrapper entries whose `hooks` array is now empty.
        arr.retain(|entry| {
            entry
                .get("hooks")
                .and_then(Value::as_array)
                .is_none_or(|hooks| !hooks.is_empty())
        });

        if seen_managed {
            eprintln!("hooks.{event_type}: ensured absolute atuin hook command");
            continue;
        }

        arr.push(serde_json::json!({
            "matcher": matcher,
            "hooks": [{"type": "command", "command": hook_command}],
        }));
        eprintln!("hooks.{event_type}: installed atuin hook");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Atuin,
        command::{AtuinCmd, client},
    };
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn parse_hook_agent_command() {
        let cmd = Cmd::try_parse_from(["hook", "codex"]).unwrap();

        assert!(matches!(
            (cmd.action, cmd.agent.as_deref()),
            (None, Some("codex"))
        ));
    }

    #[test]
    fn parse_hook_install_command() {
        let cmd = Cmd::try_parse_from(["hook", "install", "codex"]).unwrap();

        match (cmd.action, cmd.agent) {
            (Some(Action::Install { agent }), None) => assert_eq!(agent, "codex"),
            other => panic!("unexpected parsed command: {other:?}"),
        }
    }

    #[test]
    fn parse_hook_install_pi_command() {
        let cmd = Cmd::try_parse_from(["hook", "install", "pi"]).unwrap();

        match (cmd.action, cmd.agent) {
            (Some(Action::Install { agent }), None) => assert_eq!(agent, "pi"),
            other => panic!("unexpected parsed command: {other:?}"),
        }
    }

    #[test]
    fn agent_from_name_supports_pi() {
        let agent = Agent::from_name("pi").unwrap();
        assert_eq!(agent.actor_name(), "pi");
        assert!(matches!(
            agent.install_kind(),
            InstallKind::PiExtension { .. }
        ));
    }

    #[test]
    fn parse_top_level_hook_command() {
        let cmd = Atuin::try_parse_from(["atuin", "hook", "codex"]).unwrap();

        assert!(matches!(
            cmd.atuin,
            AtuinCmd::Client(client::Cmd::Hook(Cmd { action: None, agent: Some(agent) }))
                if agent == "codex"
        ));
    }

    #[test]
    fn format_hook_command_uses_absolute_path() {
        let path = PathBuf::from("/opt/homebrew/bin/atuin");
        assert_eq!(
            format_hook_command(&path, "claude-code"),
            "/opt/homebrew/bin/atuin hook claude-code"
        );
    }

    #[test]
    fn format_hook_command_quotes_spaces() {
        let path = PathBuf::from("/Users/Ada Lovelace/bin/atuin");
        let cmd = format_hook_command(&path, "codex");
        assert!(cmd.contains("hook codex"), "{cmd}");
        assert!(
            cmd.starts_with('\'') || cmd.starts_with('"'),
            "expected quoted path: {cmd}"
        );
    }

    #[test]
    fn is_managed_hook_command_matches_bare_and_absolute() {
        assert!(is_managed_hook_command(
            "atuin hook claude-code",
            "claude-code"
        ));
        assert!(is_managed_hook_command(
            "/usr/local/bin/atuin hook claude-code",
            "claude-code"
        ));
        assert!(is_managed_hook_command(
            "'/Users/Ada/bin/atuin' hook claude-code",
            "claude-code"
        ));
        assert!(is_managed_hook_command(
            r#""C:\Program Files\atuin\atuin.exe" hook claude-code"#,
            "claude-code"
        ));
        assert!(!is_managed_hook_command("atuin hook codex", "claude-code"));
        assert!(!is_managed_hook_command(
            "/usr/bin/echo hook claude-code",
            "claude-code"
        ));
    }

    #[test]
    fn add_hook_entries_upgrades_bare_command_to_absolute() {
        let agent = Agent::from_name("claude-code").unwrap();
        let mut hooks = serde_json::json!({
            "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "atuin hook claude-code"}]
            }],
            "PostToolUse": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "atuin hook claude-code"}]
            }],
            "PostToolUseFailure": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "atuin hook claude-code"}]
            }]
        });

        let desired = "/opt/homebrew/bin/atuin hook claude-code";
        add_hook_entries(&mut hooks, &agent, desired).unwrap();

        for event in HOOK_EVENT_TYPES {
            let command = hooks[event][0]["hooks"][0]["command"].as_str().unwrap();
            assert_eq!(command, desired);
            assert_eq!(hooks[event].as_array().unwrap().len(), 1);
        }
    }

    #[test]
    fn add_hook_entries_is_idempotent_for_desired_command() {
        let agent = Agent::from_name("codex").unwrap();
        let desired = "/usr/bin/atuin hook codex";
        let mut hooks = serde_json::json!({
            "PreToolUse": [{
                "matcher": "^Bash$",
                "hooks": [{"type": "command", "command": desired}]
            }],
            "PostToolUse": [{
                "matcher": "^Bash$",
                "hooks": [{"type": "command", "command": desired}]
            }],
            "PostToolUseFailure": [{
                "matcher": "^Bash$",
                "hooks": [{"type": "command", "command": desired}]
            }]
        });

        add_hook_entries(&mut hooks, &agent, desired).unwrap();

        for event in HOOK_EVENT_TYPES {
            assert_eq!(hooks[event].as_array().unwrap().len(), 1);
            assert_eq!(
                hooks[event][0]["hooks"][0]["command"].as_str().unwrap(),
                desired
            );
        }
    }

    #[test]
    fn add_hook_entries_dedups_multiple_managed_commands() {
        let agent = Agent::from_name("claude-code").unwrap();
        let desired = "/opt/homebrew/bin/atuin hook claude-code";
        let mut hooks = serde_json::json!({
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [{"type": "command", "command": "atuin hook claude-code"}]
                },
                {
                    "matcher": "Bash",
                    "hooks": [{"type": "command", "command": "/usr/local/bin/atuin hook claude-code"}]
                }
            ],
            "PostToolUse": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "atuin hook claude-code"}]
            }],
            "PostToolUseFailure": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "atuin hook claude-code"}]
            }]
        });

        add_hook_entries(&mut hooks, &agent, desired).unwrap();

        // Only one managed entry survives, rewritten to the absolute command,
        // and the now-empty wrapper is pruned.
        let pre = hooks["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 1);
        assert_eq!(pre[0]["hooks"][0]["command"].as_str().unwrap(), desired);
    }

    #[test]
    fn add_hook_entries_preserves_unmanaged_commands() {
        let agent = Agent::from_name("codex").unwrap();
        let desired = "/usr/bin/atuin hook codex";
        let mut hooks = serde_json::json!({
            "PreToolUse": [{
                "matcher": "^Bash$",
                "hooks": [
                    {"type": "command", "command": "/usr/bin/echo hi"},
                    {"type": "command", "command": "atuin hook codex"}
                ]
            }],
            "PostToolUse": [],
            "PostToolUseFailure": []
        });

        add_hook_entries(&mut hooks, &agent, desired).unwrap();

        let commands: Vec<&str> = hooks["PreToolUse"][0]["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["command"].as_str().unwrap())
            .collect();
        assert!(commands.contains(&"/usr/bin/echo hi"));
        assert!(commands.contains(&desired));
    }
}
