use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;

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
        hook_command: &'static str,
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
        hook_command: "atuin hook claude-code",
        matcher: "Bash",
    },
};

const CODEX: AgentSpec = AgentSpec {
    aliases: &["codex"],
    actor_name: "codex",
    path_root: PathRoot::Home,
    install_kind: InstallKind::JsonHooks {
        config_path: &[".codex", "hooks.json"],
        hook_command: "atuin hook codex",
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
            hook_command: _,
            matcher: _,
        } => {
            let config_path = agent.path(config_path);

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

            add_hook_entries(hooks, &agent)?;

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

fn add_hook_entries(hooks: &mut Value, agent: &Agent) -> Result<()> {
    let InstallKind::JsonHooks {
        config_path: _,
        hook_command,
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

        let already_installed = arr.iter().any(|entry| {
            entry.get("hooks").and_then(Value::as_array).is_some_and(|hooks| {
                hooks
                    .iter()
                    .any(|hook| hook.get("command").and_then(Value::as_str) == Some(hook_command))
            })
        });

        if already_installed {
            eprintln!("hooks.{event_type}: already installed, skipping");
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

    #[test]
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
    #[test]
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
    #[test]
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

    #[test]
    fn parse_top_level_hook_command() {
        let cmd = Atuin::try_parse_from(["atuin", "hook", "codex"]).unwrap();

        assert!(matches!(
            cmd.atuin,
            AtuinCmd::Client(client::Cmd::Hook(Cmd { action: None, agent: Some(agent) }))
                if agent == "codex"
        ));
    }
}
