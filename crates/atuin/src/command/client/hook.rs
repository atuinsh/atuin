use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use atuin_client::history::{AuthorKind, HistoryId};
use atuin_client::settings::Settings;
use atuin_common::harnesstools::{AnyHarness, Harness, InstallHookError, codex};
use atuin_common::path::AbsPathBuf;
use atuin_common::path::PathBufExt as _;
use atuin_common::userio;
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

        #[arg(
            long,
            value_name = "PATH",
            help = fl!("arg-hook-install-executable")
        )]
        executable: Option<PathBuf>,
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
    pub async fn run(self, settings: &Arc<Settings>) -> Result<()> {
        match (self.action, self.agent) {
            (Some(Action::Install { agent, executable }), None) => {
                install(&agent, executable.as_deref()).await
            }
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
    std::env::temp_dir().joined(format!("atuin-hook-{tool_use_id}"))
}

/// The history entry a command's start opened, which only its end (or cancel) reads.
fn take_history_id(tool_use_id: &str) -> Option<HistoryId> {
    let id_path = id_file_path(tool_use_id);
    let history_id = std::fs::read_to_string(&id_path).ok()?;
    let _ = std::fs::remove_file(&id_path);
    HistoryId::from_str(history_id.trim()).ok()
}

/// Tells the user how to make an agent pick up freshly installed hooks. The extension-based
/// agents need a nudge, and so does Kiro, which reads its hook files when a session starts;
/// the other command-hook agents load them on the next run.
fn show_reload_hint(harness_name: &str) {
    match harness_name {
        "pi" => userio::log::info!("hook-reload-hint-pi"),
        "omp" => userio::log::info!("hook-reload-hint-omp"),
        "opencode" => userio::log::info!("hook-reload-hint-opencode"),
        "amp" => userio::log::info!("hook-reload-hint-amp"),
        "kiro" => userio::log::info!("hook-reload-hint-kiro"),
        _ => {}
    }
}

async fn install(agent_name: &str, executable: Option<&Path>) -> Result<()> {
    let harness = AnyHarness::from_name(agent_name)?;

    // Hooks embed the binary by absolute path. The running binary may live in a
    // version-manager directory that an upgrade removes, so allow a stable path.
    let executable = match executable {
        Some(path) => {
            // Installed hooks are recognised (and, for Codex, trusted) by the `atuin` file name,
            // so a renamed binary would never run.
            let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            let named_atuin =
                ["atuin", "atuin.exe"].iter().any(|n| file_name.eq_ignore_ascii_case(n));
            if !named_atuin {
                bail!("--executable must point to a binary named `atuin`");
            }
            PathBuf::from(AbsPathBuf::from_current_dir(path)?)
        }
        None => atuin_common::os::process::current_exe(),
    };

    match harness.install_hooks(&executable).await {
        Ok(path) => {
            userio::log::info!(
                "hook-installed",
                harness = harness.name(),
                path = path.display().to_string()
            );
            if harness.name() == "codex" {
                // Codex runs only the hooks the user trusted (see `codex::hook_trust`).
                let config = path.with_file_name("config.toml");
                userio::log::info!("hook-codex-trusted", config = config.display().to_string());
            }
            show_reload_hint(harness.name());
        }
        Err(InstallHookError::AlreadyInstalled) => {
            userio::log::info!("hook-already-installed", harness = harness.name());
        }
        Err(err) => return Err(err.into()),
    }

    if cfg!(feature = "ai") {
        userio::log::info!("hook-mcp-hint", name = harness.name());
    }

    Ok(())
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

    #[rstest]
    fn parse_hook_agent_command() {
        let cmd = Cmd::try_parse_from(["hook", "codex"]).unwrap();

        assert!(matches!((cmd.action, cmd.agent.as_deref()), (None, Some("codex"))));
    }

    #[rstest]
    #[case::codex("codex")]
    #[case::grok("grok")]
    #[case::opencode("opencode")]
    #[case::pi("pi")]
    fn parse_hook_install_command(#[case] agent_name: &str) {
        let cmd = Cmd::try_parse_from(["hook", "install", agent_name]).unwrap();

        match (cmd.action, cmd.agent) {
            (Some(Action::Install { agent, executable }), None) => {
                assert_eq!(agent, agent_name);
                assert!(executable.is_none());
            }
            other => panic!("unexpected parsed command: {other:?}"),
        }
    }

    #[rstest]
    fn parse_hook_install_executable_command() {
        let cmd = Cmd::try_parse_from([
            "hook",
            "install",
            "claude-code",
            "--executable",
            "/usr/bin/atuin",
        ])
        .unwrap();

        match (cmd.action, cmd.agent) {
            (Some(Action::Install { agent, executable }), None) => {
                assert_eq!(agent, "claude-code");
                assert_eq!(
                    executable.map(|path| path.to_string_lossy().into_owned()).as_deref(),
                    Some("/usr/bin/atuin")
                );
            }
            other => panic!("unexpected parsed command: {other:?}"),
        }
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
            AtuinCmd::Client(client::Cmd::Hook(Cmd { action: None, agent: Some(agent) }))
                if agent == "codex"
        ));
    }
}
