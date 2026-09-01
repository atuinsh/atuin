use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use atuin_client::history::{AuthorKind, HistoryId};
use atuin_client::settings::Settings;
use atuin_common::utils::home_dir;
use clap::{Parser, Subcommand};
use eyre::{Result, bail};
use serde_json::Value;
use tracing::instrument;

use super::history;

mod event;
mod wire;

use event::HookEvent;

const HOOK_EVENT_TYPES: &[&str] = &["PreToolUse", "PostToolUse", "PostToolUseFailure"];
const PI_EXTENSION_SOURCE: &str = include_str!("../../../contrib/pi/atuin.ts");
const OPENCODE_PLUGIN_SOURCE: &str = include_str!("../../../contrib/opencode/atuin.ts");

enum InstallKind {
    JsonHooks {
        config_path: &'static [&'static str],
        matcher: &'static str,
    },
    /// An agent that loads TypeScript extensions from a directory, rather than
    /// invoking `atuin hook <agent>` from its config.
    Extension {
        extension_path: &'static [&'static str],
        source: &'static str,
        /// How the user makes the agent pick the file up.
        reload_hint: &'static str,
    },
}

/// The directory an agent's [`InstallKind`] path is relative to.
enum PathRoot {
    Home,
    /// See [`xdg_config_home`].
    XdgConfig,
}

/// Resolve `$XDG_CONFIG_HOME` the way agents' XDG libraries do, treating an
/// empty value as unset. Taking it literally would resolve to a relative path
/// and install under the current directory, where the agent never looks.
fn xdg_config_home(var: Option<OsString>) -> PathBuf {
    var.filter(|value| !value.is_empty()).map_or_else(|| home_dir().join(".config"), PathBuf::from)
}

struct AgentSpec {
    aliases: &'static [&'static str],
    actor_name: &'static str,
    path_root: PathRoot,
    install_kind: InstallKind,
}

const CLAUDE_CODE: AgentSpec = AgentSpec {
    aliases: &["claude-code", "claude"],
    actor_name: "claude-code",
    path_root: PathRoot::Home,
    install_kind: InstallKind::JsonHooks {
        config_path: &[".claude", "settings.json"],
        matcher: "Bash",
    },
};

const CODEX: AgentSpec = AgentSpec {
    aliases: &["codex"],
    actor_name: "codex",
    path_root: PathRoot::Home,
    install_kind: InstallKind::JsonHooks {
        config_path: &[".codex", "hooks.json"],
        matcher: "^Bash$",
    },
};

const PI: AgentSpec = AgentSpec {
    aliases: &["pi"],
    actor_name: "pi",
    path_root: PathRoot::Home,
    install_kind: InstallKind::Extension {
        extension_path: &[".pi", "agent", "extensions", "atuin.ts"],
        source: PI_EXTENSION_SOURCE,
        reload_hint: "Reload pi with `/reload` or restart pi.",
    },
};

const OPENCODE: AgentSpec = AgentSpec {
    aliases: &["opencode"],
    actor_name: "opencode",
    path_root: PathRoot::XdgConfig,
    install_kind: InstallKind::Extension {
        extension_path: &["opencode", "plugins", "atuin.ts"],
        source: OPENCODE_PLUGIN_SOURCE,
        reload_hint: "Restart opencode to load the plugin.",
    },
};

const AGENTS: &[&AgentSpec] = &[&CLAUDE_CODE, &CODEX, &OPENCODE, &PI];

struct Agent(&'static AgentSpec);

impl Agent {
    fn from_name(name: &str) -> Result<Self> {
        AGENTS.iter().copied().find(|spec| spec.aliases.contains(&name)).map(Self).ok_or_else(
            || {
                eyre::eyre!(
                    "unknown agent: {name}. Supported agents: claude-code, codex, opencode, pi"
                )
            },
        )
    }

    fn actor_name(&self) -> &'static str {
        self.0.actor_name
    }

    fn path(&self, path: &'static [&'static str]) -> PathBuf {
        let root = match self.0.path_root {
            PathRoot::Home => home_dir(),
            PathRoot::XdgConfig => xdg_config_home(std::env::var_os("XDG_CONFIG_HOME")),
        };

        path.iter().fold(root, |path, segment| path.join(segment))
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
    #[instrument(level = "trace", skip_all, err)]
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

fn hook_command(agent: &Agent, executable: &Path) -> Result<String> {
    let executable = executable
        .to_str()
        .ok_or_else(|| eyre::eyre!("atuin executable path is not valid UTF-8"))?;

    #[cfg(windows)]
    let executable = format!(r#""{executable}""#);

    #[cfg(not(windows))]
    let executable = format!("'{}'", executable.replace('\'', "'\"'\"'"));

    Ok(format!("{executable} hook {}", agent.actor_name()))
}

fn invokes_atuin_hook(command: &str, agent: &Agent) -> bool {
    let Some(parts) = shlex::split(command) else {
        return false;
    };

    parts.len() == 3
        && Path::new(&parts[0])
            .file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("atuin"))
        && parts[1] == "hook"
        && parts[2] == agent.actor_name()
}

async fn handle(agent_name: &str, settings: &Settings) -> Result<()> {
    let agent = Agent::from_name(agent_name)?;

    if let InstallKind::Extension { reload_hint, .. } = agent.install_kind() {
        bail!(
            "`atuin hook {agent_name}` is not supported. Use `atuin hook install {agent_name}`. \
             {reload_hint}"
        );
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
                Some(AuthorKind::Agent),
                intent.as_deref(),
            )
            .await?
            {
                std::fs::write(id_file_path(&tool_use_id), history_id.to_string())?;
            }
        }
        Some(HookEvent::End { tool_use_id, exit }) => {
            let id_path = id_file_path(&tool_use_id);

            if let Ok(history_id) = std::fs::read_to_string(&id_path) {
                if let Ok(history_id) = HistoryId::from_str(history_id.trim()) {
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
            matcher: _,
        } => {
            let config_path = agent.path(config_path);
            let executable = std::env::current_exe()?;
            let hook_command = hook_command(&agent, &executable)?;

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

            add_hook_entries(hooks, &agent, &hook_command)?;

            let content = serde_json::to_string_pretty(&root)?;
            std::fs::write(&config_path, content)?;

            eprintln!(
                "\nAtuin hooks installed for {}. Config: {}",
                agent.actor_name(),
                config_path.display()
            );
        }
        InstallKind::Extension {
            extension_path,
            source,
            reload_hint,
        } => {
            let extension_path = agent.path(extension_path);
            let actor_name = agent.actor_name();

            if let Some(parent) = extension_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let already_installed =
                std::fs::read_to_string(&extension_path).is_ok_and(|existing| existing == *source);

            if already_installed {
                eprintln!("{actor_name} extension: already installed, skipping");
            } else {
                std::fs::write(&extension_path, source)?;
                eprintln!("{actor_name} extension: installed atuin extension");
            }

            eprintln!(
                "\nAtuin extension installed for {actor_name}. Extension: {}\n{reload_hint}",
                extension_path.display()
            );
        }
    }

    Ok(())
}

fn add_hook_entries(hooks: &mut Value, agent: &Agent, hook_command: &str) -> Result<()> {
    let InstallKind::JsonHooks {
        config_path: _,
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

        let mut already_installed = false;
        let mut updated_command = false;

        for entry in arr.iter_mut() {
            if entry.get("matcher").and_then(Value::as_str) != Some(matcher) {
                continue;
            }

            let remove_entry = {
                let Some(installed_hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut)
                else {
                    continue;
                };
                let had_hooks = !installed_hooks.is_empty();

                installed_hooks.retain_mut(|installed_hook| {
                    let Some(command) = installed_hook.get_mut("command") else {
                        return true;
                    };
                    let Some(command_str) = command.as_str() else {
                        return true;
                    };

                    if !invokes_atuin_hook(command_str, agent) {
                        return true;
                    }

                    if already_installed {
                        updated_command = true;
                        return false;
                    }

                    already_installed = true;
                    if command_str != hook_command {
                        *command = Value::String(hook_command.to_owned());
                        updated_command = true;
                    }
                    true
                });

                had_hooks && installed_hooks.is_empty()
            };

            if remove_entry {
                *entry = Value::Null;
            }
        }
        arr.retain(|entry| !entry.is_null());

        if already_installed {
            if updated_command {
                eprintln!("hooks.{event_type}: updated atuin executable path");
            } else {
                eprintln!("hooks.{event_type}: already installed, skipping");
            }
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
    use atuin_client::history::is_known_agent;
    use clap::Parser;
    use rstest::rstest;

    use super::*;
    use crate::Atuin;
    use crate::command::{AtuinCmd, client};

    #[rstest]
    fn parse_hook_agent_command() {
        let cmd = Cmd::try_parse_from(["hook", "codex"]).unwrap();

        assert!(matches!((cmd.action, cmd.agent.as_deref()), (None, Some("codex"))));
    }

    #[rstest]
    #[case::codex("codex")]
    #[case::opencode("opencode")]
    #[case::pi("pi")]
    fn parse_hook_install_command(#[case] agent_name: &str) {
        let cmd = Cmd::try_parse_from(["hook", "install", agent_name]).unwrap();

        match (cmd.action, cmd.agent) {
            (Some(Action::Install { agent }), None) => assert_eq!(agent, agent_name),
            other => panic!("unexpected parsed command: {other:?}"),
        }
    }

    #[rstest]
    #[case::opencode("opencode")]
    #[case::pi("pi")]
    fn agent_from_name_supports_extension_agents(#[case] agent_name: &str) {
        let agent = Agent::from_name(agent_name).unwrap();
        assert_eq!(agent.actor_name(), agent_name);
        assert!(matches!(agent.install_kind(), InstallKind::Extension { .. }));
    }

    /// An agent missing from `KNOWN_AGENTS` would be installable but invisible
    /// to `$all-agent`, and would pollute `$all-user` with its commands.
    #[rstest]
    fn every_agent_author_is_a_known_agent() {
        for spec in AGENTS {
            assert!(
                is_known_agent(spec.actor_name),
                "{} is missing from KNOWN_AGENTS",
                spec.actor_name
            );
        }
    }

    /// An empty `XDG_CONFIG_HOME` taken literally would resolve to a relative
    /// path, installing under the current directory instead of the agent's
    /// config directory.
    #[rstest]
    #[case::set(Some("/tmp/xdg"), PathBuf::from("/tmp/xdg"))]
    #[case::empty_is_unset(Some(""), home_dir().join(".config"))]
    #[case::unset(None, home_dir().join(".config"))]
    fn xdg_config_home_resolves(#[case] var: Option<&str>, #[case] expected: PathBuf) {
        assert_eq!(xdg_config_home(var.map(OsString::from)), expected);
    }

    /// opencode reads plugins from its XDG config directory, not from `$HOME`.
    #[rstest]
    fn opencode_plugin_is_rooted_in_the_xdg_config_dir() {
        let agent = Agent::from_name("opencode").unwrap();
        let InstallKind::Extension { extension_path, .. } = agent.install_kind() else {
            panic!("opencode does not install an extension");
        };

        let root = xdg_config_home(std::env::var_os("XDG_CONFIG_HOME"));
        let installed = agent.path(extension_path);

        assert!(installed.starts_with(&root), "{installed:?} is not under {root:?}");
        assert!(installed.ends_with("opencode/plugins/atuin.ts"));
    }

    #[rstest]
    fn parse_top_level_hook_command() {
        let cmd = Atuin::try_parse_from(["atuin", "hook", "codex"]).unwrap();

        assert!(matches!(
            cmd.atuin,
            AtuinCmd::Client(client::Cmd::Hook(Cmd { action: None, agent: Some(agent) }))
                if agent == "codex"
        ));
    }

    #[rstest]
    fn add_hook_entries_updates_legacy_commands_without_duplicates() {
        let agent = Agent::from_name("claude-code").unwrap();
        let command = "'/opt/atuin/bin/atuin' hook claude-code";
        let mut hooks = serde_json::json!({
            "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [
                    {"type": "command", "command": "\"$HOME/.atuin/bin/atuin\" hook claude-code"},
                    {"type": "command", "command": "printf keep-me"},
                ],
            }, {
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "atuin hook claude-code"}],
            }, {
                "matcher": "Read",
                "hooks": [{"type": "command", "command": "atuin hook claude-code"}],
            }],
        });

        add_hook_entries(&mut hooks, &agent, command).unwrap();
        add_hook_entries(&mut hooks, &agent, command).unwrap();

        for event_type in HOOK_EVENT_TYPES {
            let entries = hooks[event_type].as_array().unwrap();
            let commands: Vec<_> = entries
                .iter()
                .flat_map(|entry| entry["hooks"].as_array().unwrap())
                .filter_map(|hook| hook["command"].as_str())
                .collect();

            assert_eq!(commands.iter().filter(|value| **value == command).count(), 1);
            if event_type == &"PreToolUse" {
                assert!(commands.contains(&"printf keep-me"));
                assert!(commands.contains(&"atuin hook claude-code"));
                assert_eq!(entries.len(), 2);
            } else {
                assert_eq!(entries.len(), 1);
            }
        }
    }

    #[cfg(not(windows))]
    #[rstest]
    fn hook_command_quotes_posix_executable_paths() {
        let agent = Agent::from_name("codex").unwrap();
        let executable = Path::new("/opt/Atuin's bin/atuin");
        let command = hook_command(&agent, executable).unwrap();

        assert_eq!(command, "'/opt/Atuin'\"'\"'s bin/atuin' hook codex");
        assert!(invokes_atuin_hook(&command, &agent));
    }

    #[cfg(windows)]
    #[rstest]
    fn hook_command_quotes_windows_executable_paths() {
        let agent = Agent::from_name("codex").unwrap();
        let executable = Path::new(r"C:\Program Files\Atuin\atuin.exe");
        let command = hook_command(&agent, executable).unwrap();

        assert_eq!(command, r#""C:\Program Files\Atuin\atuin.exe" hook codex"#);
        assert!(invokes_atuin_hook(&command, &agent));
    }
}
