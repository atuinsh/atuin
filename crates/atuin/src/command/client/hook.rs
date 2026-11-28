use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use atuin_client::history::{AuthorKind, HistoryId};
use atuin_client::settings::Settings;
use atuin_common::harnesstools::{AnyHarness, Harness, codex};
use atuin_common::path::PathBufExt as _;
use clap::{Parser, Subcommand};
use eyre::Result;
use tracing::instrument;

use super::history;

mod event;
mod wire;

use event::HookEvent;

use crate::i18n::fl;

#[derive(Parser, Debug)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub struct Cmd {
    #[command(subcommand)]
    moved: Option<Moved>,

    #[arg(value_name = "AGENT", required = true, help = fl!("arg-hook-agent"))]
    agent: Option<String>,
}

/// What `atuin hook` did before setup did it, kept so that the scripts still running it say what
/// to run instead.
#[derive(Subcommand, Debug)]
enum Moved {
    #[command(hide = true)]
    Install {
        agent: String,
    },
}

impl Cmd {
    pub async fn run(self, settings: &Arc<Settings>) -> Result<()> {
        match (self.moved, self.agent) {
            (Some(Moved::Install { agent }), _) => Err(super::setup::moved_here(
                "atuin hook install",
                &format!("--agent-history --agents {agent}"),
            )),
            (None, Some(agent)) => handle(&agent, settings).await,
            (None, None) => Err(eyre::eyre!("name the agent whose hook this is")),
        }
    }
}

fn id_file_path(tool_use_id: &str) -> PathBuf {
    std::env::temp_dir().joined(format!("atuin-hook-{tool_use_id}"))
}

/// The history entry a command's start opened, which only its end (or cancel) reads.
fn take_history_id(tool_use_id: &str) -> Option<HistoryId> {
    let id_path = id_file_path(tool_use_id);
    let history_id = std::fs::read_to_string(&id_path).ok()?;
    let _ = std::fs::remove_file(&id_path);
    HistoryId::from_str(history_id.trim()).ok()
}

/// Grok Build executes Claude Code hooks from `~/.claude/settings.json` when Claude compatibility
/// is enabled, setting `GROK_HOOK_EVENT` in the environment of every hook it runs. When a
/// native `grok` hook is installed too, skip `atuin hook claude-code` so the two hooks do not
/// record duplicates or race on `$TMPDIR/atuin-hook-<tool_use_id>`; otherwise keep the event and
/// attribute it to `grok`.
fn resolve_hook_author(harness_name: &str) -> Option<&str> {
    let inside_grok = harness_name == "claude-code"
        && atuin_common::env::var_nonempty("GROK_HOOK_EVENT").is_some();
    resolve_hook_author_with(
        harness_name,
        inside_grok,
        inside_grok && atuin_common::harnesstools::grok::has_installed_hooks(),
    )
}

fn resolve_hook_author_with(
    harness_name: &str,
    inside_grok: bool,
    grok_hook_installed: bool,
) -> Option<&str> {
    if harness_name == "claude-code" && inside_grok {
        return (!grok_hook_installed).then_some("grok");
    }
    Some(harness_name)
}

#[instrument(level = "trace", skip_all, err)]
async fn handle(agent_name: &str, settings: &Arc<Settings>) -> Result<()> {
    let harness = AnyHarness::from_name(agent_name)?;

    // Read whatever the hook is skipped for, so the agent's write of the event never fails.
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let Some(author) = resolve_hook_author(harness.name()) else {
        return Ok(());
    };

    if input.trim().is_empty() {
        return Ok(());
    }

    match HookEvent::from_json_str(&input, harness.name())? {
        Some(HookEvent::Start {
            command,
            intent,
            tool_use_id,
            cwd,
            at,
        }) => {
            if let Some(history_id) = history::start_history_entry(
                settings,
                &command,
                Some(author),
                Some(AuthorKind::Agent),
                intent.as_deref(),
                cwd.as_deref(),
                at,
            )
            .await?
            {
                std::fs::write(id_file_path(&tool_use_id), history_id.to_string())?;
            }
        }
        Some(HookEvent::End {
            tool_use_id,
            exit,
            transcript,
            duration,
        }) => {
            let exit = exit
                .or_else(|| {
                    let transcript = transcript.filter(|_| harness.name() == "codex")?;
                    codex::session::command_exit_code(&transcript, &tool_use_id)
                })
                .unwrap_or(0);
            if let Some(history_id) = take_history_id(&tool_use_id) {
                let _ = history::end_history_entry(settings, history_id, exit, duration).await;
            }
        }
        Some(HookEvent::Cancel { tool_use_id }) => {
            if let Some(history_id) = take_history_id(&tool_use_id) {
                let _ = history::cancel_history_entry(settings, history_id).await;
            }
        }
        Some(HookEvent::SessionStart { session_id, cwd }) => {
            session_started(settings, author, session_id, &cwd).await;
        }
        None => {}
    }

    Ok(())
}

/// Tell the daemon that `author`'s session `id` started in `dir`. A daemon that doesn't hear of it
/// only learns of the session at its first message, so this never fails the agent's start.
#[cfg(feature = "daemon")]
async fn session_started(settings: &Settings, author: &str, id: String, dir: &Path) {
    use atuin_client::ai_session::{HarnessKind, HarnessSession, NativeSessionId};

    let Ok(harness) = AnyHarness::from_name(author) else {
        return;
    };
    let thread = HarnessSession {
        harness: HarnessKind::from(&harness),
        session: NativeSessionId::from(id),
    };
    if let Err(err) = super::daemon::thread_started(settings, thread, dir).await {
        tracing::warn!(?err, "the daemon didn't hear that a session started");
    }
}

/// Without a daemon, nothing hears of a session until its first message.
#[cfg(not(feature = "daemon"))]
async fn session_started(_settings: &Settings, _author: &str, _id: String, _dir: &Path) {}

#[cfg(test)]
mod tests {
    use atuin_client::history::is_known_agent;
    use clap::Parser;
    use rstest::rstest;

    use super::*;
    use crate::Atuin;
    use crate::command::{AtuinCmd, client};

    /// An agent's hook parses, and so does the install setup took over, to say what to run.
    #[rstest]
    #[case::hook(&["hook", "codex"], None, Some("codex"))]
    #[case::moved_install(&["hook", "install", "codex"], Some("codex"), None)]
    fn parse_hook_agent_command(
        #[case] args: &[&str],
        #[case] moved: Option<&str>,
        #[case] agent: Option<&str>,
    ) {
        let cmd = Cmd::try_parse_from(args).unwrap();
        let installs = cmd.moved.map(|Moved::Install { agent }| agent);
        assert_eq!((installs.as_deref(), cmd.agent.as_deref()), (moved, agent));
    }

    /// A harness whose author is missing from `KNOWN_AGENTS` would be installable
    /// but invisible to `$all-agent`, and would pollute `$all-user` with its commands.
    #[rstest]
    fn every_harness_author_is_a_known_agent() {
        for harness in AnyHarness::all() {
            assert!(
                is_known_agent(harness.name()),
                "{} is missing from KNOWN_AGENTS",
                harness.name()
            );
        }
    }

    #[rstest]
    #[case::claude_outside_grok("claude-code", false, false, Some("claude-code"))]
    #[case::claude_inside_grok_without_grok_hook("claude-code", true, false, Some("grok"))]
    #[case::claude_inside_grok_with_grok_hook("claude-code", true, true, None)]
    #[case::grok_own_hook("grok", true, true, Some("grok"))]
    #[case::codex_own_hook("codex", true, false, Some("codex"))]
    fn resolves_inherited_claude_hook_inside_grok(
        #[case] harness_name: &str,
        #[case] inside_grok: bool,
        #[case] grok_hook_installed: bool,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(
            resolve_hook_author_with(harness_name, inside_grok, grok_hook_installed),
            expected
        );
    }

    #[rstest]
    fn parse_top_level_hook_command() {
        let cmd = Atuin::try_parse_from(["atuin", "hook", "codex"]).unwrap();

        assert!(matches!(
            cmd.atuin,
            AtuinCmd::Client(client::Cmd::Hook(Cmd { moved: None, agent: Some(agent) }))
                if agent == "codex"
        ));
    }
}
