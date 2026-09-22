use std::io::Read;
use std::path::PathBuf;
use std::str::FromStr;

use atuin_client::history::{AuthorKind, HistoryId};
use atuin_client::settings::Settings;
use atuin_common::harnesstools::{AnyHarness, Harness, InstallHookError};
use clap::{Parser, Subcommand};
use eyre::{Result, bail};
use tracing::instrument;

use super::history;

mod event;
mod wire;

use event::HookEvent;

use crate::i18n::fl;

#[derive(Subcommand, Debug)]
enum Action {
    #[command(about = fl!("cmd-hook-install"))]
    Install {
        #[arg(value_name = "AGENT", help = fl!("arg-hook-install-agent"))]
        agent: String,
    },
}

#[derive(Parser, Debug)]
#[command(infer_subcommands = true, args_conflicts_with_subcommands = true)]
pub struct Cmd {
    #[command(subcommand)]
    action: Option<Action>,

    #[arg(value_name = "AGENT", hide = true, help = fl!("arg-hook-agent"))]
    agent: Option<String>,
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: &Settings) -> Result<()> {
        match (self.action, self.agent) {
            (Some(Action::Install { agent }), None) => install(&agent).await,
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

/// How the user makes an agent pick up a freshly installed extension. Only the
/// extension-based agents need a nudge; command-hook agents load on next run.
fn reload_hint(harness_name: &str) -> Option<&'static str> {
    match harness_name {
        "pi" => Some("Reload pi with `/reload` or restart pi."),
        "opencode" => Some("Restart opencode to load the plugin."),
        _ => None,
    }
}

async fn install(agent_name: &str) -> Result<()> {
    let harness = AnyHarness::from_name(agent_name)?;

    match harness.install_hooks().await {
        Ok(path) => {
            eprintln!("Atuin hooks installed for {}. Path: {}", harness.name(), path.display());
            if let Some(hint) = reload_hint(harness.name()) {
                eprintln!("{hint}");
            }
        }
        Err(InstallHookError::AlreadyInstalled) => {
            eprintln!("{}: hooks already installed, skipping", harness.name());
        }
        Err(err) => return Err(err.into()),
    }

    Ok(())
}

async fn handle(agent_name: &str, settings: &Settings) -> Result<()> {
    let harness = AnyHarness::from_name(agent_name)?;

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
                Some(harness.name()),
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

    /// A harness whose author is missing from `KNOWN_AGENTS` would be installable
    /// but invisible to `$all-agent`, and would pollute `$all-user` with its commands.
    #[test]
    fn every_harness_author_is_a_known_agent() {
        for harness in AnyHarness::all() {
            assert!(
                is_known_agent(harness.name()),
                "{} is missing from KNOWN_AGENTS",
                harness.name()
            );
        }
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
