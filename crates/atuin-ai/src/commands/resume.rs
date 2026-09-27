//! `atuin ai resume [QUERY]`: pick a captured AI coding-agent session and resume it.
//!
//! - A QUERY that is a session id (or a unique prefix of one) resumes it directly, no picker.
//!   From the shell widget, `enter_accept` decides whether it runs or lands on the command line;
//!   a session that can't be resumed opens the picker on it, so the reason shows.
//! - A session whose transcript isn't on this machine (recorded on another host, or deleted) is
//!   restored from the synced messages first: the harness's own transcript is written out, then
//!   resumed with the usual command. That happens only once a session is chosen (enter or tab in
//!   the picker, or named by id), in this process, before the command is run or handed to the
//!   shell widget, so the command the widget puts on the command line works as it stands.
//! - From the shell widget (`--shell-widget`), the result goes to stderr using the history
//!   search's protocol: `__atuin_accept__:<cmd>` to run it, plain `<cmd>` to edit it, nothing to
//!   leave the command line alone.
//! - Standalone on a terminal, resuming changes directory and execs the harness.
//! - With `--print`, or when stdout is not a terminal, the command is printed instead.
//!
//! The picker reads the sidecar database directly and never waits for the daemon.

use std::io::{self, IsTerminal, Write};
use std::sync::Arc;

use atuin_client::settings::{AiSessionFilterMode, Settings};
use atuin_client::theme::ThemeManager;
use clap::Args;
use eyre::{Result, bail};

use crate::resume_tui::fake::{FakeResumer, FakeSource};
use crate::resume_tui::resumer::{HarnessResumer, Resume, shell_line};
use crate::resume_tui::sidecar::{HostNameSource, SidecarSource};
use crate::resume_tui::{
    Outcome, Picker, ResumeContext, ResumePlan, Resumer, SessionRow, SessionSource,
};

const ACCEPT_PREFIX: &str = "__atuin_accept__:";

/// Ids shorter than this are treated as search text, so a short word can't resume by accident.
const MIN_ID_PREFIX: usize = 6;

#[derive(Args, Debug)]
pub struct Cmd {
    /// Search text, or a session id (or unique id prefix) to resume without the picker.
    #[arg(value_name = "QUERY", trailing_var_arg = true)]
    query: Vec<String>,

    /// Print the resume command instead of running it.
    #[arg(long)]
    print: bool,

    /// The filter the picker opens in (default: workspace, widening to global).
    #[arg(long, value_enum)]
    filter_mode: Option<AiSessionFilterMode>,

    /// Height of the inline picker; 0 for fullscreen.
    #[arg(long)]
    inline_height: Option<u16>,

    /// Keymap mode, as passed by the shell widget.
    #[arg(long, value_enum, hide = true)]
    keymap_mode: Option<atuin_client::settings::KeymapMode>,

    /// Report the result on stderr for the shell widget.
    #[arg(long, hide = true)]
    shell_widget: bool,

    /// Browse built-in example sessions instead of the captured ones.
    #[arg(long, hide = true)]
    demo: bool,

    /// Read this sidecar database instead of the configured one (for debugging).
    #[arg(long, hide = true, value_name = "PATH")]
    db: Option<std::path::PathBuf>,
}

/// Where the result goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Output {
    /// The shell widget reads stderr.
    Widget,
    /// Print the command on stdout.
    Print,
    /// chdir and exec.
    Exec,
}

impl Cmd {
    fn output(&self) -> Output {
        if self.shell_widget {
            Output::Widget
        } else if self.print || !io::stdout().is_terminal() {
            Output::Print
        } else {
            Output::Exec
        }
    }

    /// The query, from the arguments or the widget's `ATUIN_QUERY`.
    fn query(&self) -> String {
        if self.query.is_empty() {
            std::env::var("ATUIN_QUERY").unwrap_or_default()
        } else {
            self.query.join(" ")
        }
    }
}

/// A query that looks like a session id: one word of id characters, long enough.
fn looks_like_id(query: &str) -> bool {
    query.len() >= MIN_ID_PREFIX
        && query.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        && query.chars().any(|c| c.is_ascii_digit())
}

/// The single session `query` names, if it is an exact id or a unique prefix.
async fn direct_match(source: &dyn SessionSource, query: &str) -> Result<Option<SessionRow>> {
    if !looks_like_id(query) {
        return Ok(None);
    }
    let mut found = source.find_by_id(query).await?;
    if let Some(exact) = found.iter().position(|r| r.handle.session.as_ref() == query) {
        return Ok(Some(found.swap_remove(exact)));
    }
    Ok(if found.len() == 1 {
        found.pop()
    } else {
        None
    })
}

pub async fn run(cmd: Cmd, settings: &Settings) -> Result<()> {
    let mut settings = settings.clone();
    if let Some(mode) = cmd.filter_mode {
        settings.ai.sessions.filter_mode = Some(mode);
    }
    if let Some(mode) = cmd.keymap_mode {
        settings.keymap_mode = mode;
    }
    let output = cmd.output();
    let query = cmd.query();

    let (context, source, resumer): (ResumeContext, Arc<dyn SessionSource>, Arc<dyn Resumer>) =
        if cmd.demo {
            (
                crate::resume_tui::fake::context(),
                Arc::new(FakeSource::new().relative_to(time::OffsetDateTime::now_utc())),
                Arc::new(FakeResumer::default()),
            )
        } else {
            let context = ResumeContext::current().await?;
            let path = cmd.db.clone().unwrap_or_else(Settings::ai_session_sidecar_path);
            let host_names = HostNameSource::new(&settings);
            let source = SidecarSource::open(&path, &context, Some(host_names)).await?;
            let resumer = HarnessResumer::new(context.clone(), settings.ai.sessions.resume.clone());
            (context, Arc::new(source), Arc::new(resumer))
        };

    let mut preselect = None;
    if let Some(row) = direct_match(source.as_ref(), query.trim()).await? {
        let plan = match resumer.plan(&row).await {
            Ok(Resume {
                plan,
                restore: None,
            }) => Ok(plan),
            Ok(Resume {
                restore: Some(restore),
                ..
            }) => {
                let plan = resumer.restore(source.as_ref(), &row, &restore).await;
                // The widget reads stderr for the command: nothing else may go there.
                if plan.is_ok()
                    && output != Output::Widget
                    && let Some(note) = &restore.note
                {
                    eprintln!("atuin: restored the session from sync; {note}");
                }
                plan
            }
            Err(why) => Err(why),
        };
        match plan {
            Ok(plan) => return finish(direct_outcome(plan, output, settings.enter_accept), output),
            // Open the picker on it instead, so the reason shows (and another can be picked).
            Err(why) => preselect = Some((row, why)),
        }
    }

    let mut themes = ThemeManager::new(settings.theme.debug, None);
    let theme = themes.load_theme(settings.theme.name.as_str(), settings.theme.max_depth);
    let outcome = Picker {
        settings: &settings,
        theme,
        source,
        resumer,
        context,
        query,
        inline_height: cmd.inline_height,
        preselect,
    }
    .run()
    .await?;
    finish(outcome, output)
}

/// What a session named by id does without the picker. Standalone, `atuin ai resume <id>` resumes
/// it. The widget passes whatever is on the command line, so there it follows `enter_accept` like
/// the picker's enter: without it, the command lands on the command line to be checked first.
fn direct_outcome(plan: ResumePlan, output: Output, enter_accept: bool) -> Outcome {
    if output == Output::Widget && !enter_accept {
        Outcome::Edit(plan)
    } else {
        Outcome::Resume(plan)
    }
}

fn finish(outcome: Outcome, output: Output) -> Result<()> {
    let (plan, run) = match outcome {
        Outcome::Cancelled => return Ok(()),
        Outcome::Resume(plan) => (plan, true),
        Outcome::Edit(plan) => (plan, false),
    };
    match output {
        Output::Widget => {
            let line = shell_line(&plan);
            if run {
                eprintln!("{ACCEPT_PREFIX}{line}");
            } else {
                eprintln!("{line}");
            }
            Ok(())
        }
        Output::Exec if run => exec(&plan),
        Output::Print | Output::Exec => {
            let mut out = io::stdout().lock();
            writeln!(out, "{}", shell_line(&plan))?;
            Ok(())
        }
    }
}

/// Change into the session's directory and replace this process with the harness.
fn exec(plan: &ResumePlan) -> Result<()> {
    let mut command = std::process::Command::new(&plan.program);
    command.args(&plan.args);
    if let Some(cwd) = &plan.cwd {
        std::env::set_current_dir(cwd)?;
        // Keep $PWD in step, so harnesses that read it see the session's directory.
        command.env("PWD", cwd);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = command.exec();
        bail!("failed to run {}: {err}", plan.command())
    }
    #[cfg(not(unix))]
    {
        let status = command.status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use rstest::rstest;

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        cmd: Cmd,
    }

    #[rstest]
    #[case("7f3c9a12", true)]
    #[case("ses_4b8e2f1a9c3d7e6f", true)]
    #[case("flaky", false)]
    #[case("7f3c9", false)]
    #[case("fix flaky", false)]
    #[case("release", false)]
    fn id_like_queries(#[case] query: &str, #[case] want: bool) {
        assert_eq!(looks_like_id(query), want);
    }

    #[rstest]
    #[tokio::test]
    async fn unique_prefix_and_exact_ids_resume_directly() {
        let source = FakeSource::new();
        let row = direct_match(&source, "7f3c9a12").await.unwrap().unwrap();
        assert_eq!(row.handle.session.as_ref(), "7f3c9a12-5be0-4d7e-9c41-0a8e2b6f4d10");

        // `ses_4b8e2f1a9c3d7e6f` is a prefix of nothing else; `ses_4b8e2f1a9c3d7e` is ambiguous.
        assert!(direct_match(&source, "ses_4b8e2f1a9c3d7e6f").await.unwrap().is_some());
        assert!(direct_match(&source, "ses_4b8e2f1a9c3d7e").await.unwrap().is_none());
        assert!(direct_match(&source, "flaky").await.unwrap().is_none());
    }

    #[rstest]
    #[case(Output::Widget, false, false)]
    #[case(Output::Widget, true, true)]
    #[case(Output::Exec, false, true)]
    #[case(Output::Print, false, true)]
    fn a_direct_match_from_the_widget_follows_enter_accept(
        #[case] output: Output,
        #[case] enter_accept: bool,
        #[case] runs: bool,
    ) {
        let plan = ResumePlan {
            program: "claude".to_owned(),
            args: vec!["--resume".to_owned(), "abc".to_owned()],
            cwd: None,
            cwd_requirement: atuin_common::harnesstools::resume::CwdRequirement::Preferred,
            native_path: None,
        };
        let outcome = direct_outcome(plan, output, enter_accept);
        assert_eq!(matches!(outcome, Outcome::Resume(_)), runs, "{outcome:?}");
    }

    #[rstest]
    fn parses_flags() {
        let cli = Cli::try_parse_from([
            "resume",
            "--print",
            "--filter-mode",
            "branch",
            "--inline-height",
            "12",
            "fix",
            "flaky",
        ])
        .unwrap();
        assert!(cli.cmd.print);
        assert_eq!(cli.cmd.filter_mode, Some(AiSessionFilterMode::Branch));
        assert_eq!(cli.cmd.inline_height, Some(12));
        assert_eq!(cli.cmd.query(), "fix flaky");
        assert_eq!(cli.cmd.output(), Output::Print);

        let cli = Cli::try_parse_from(["resume", "--shell-widget"]).unwrap();
        assert_eq!(cli.cmd.output(), Output::Widget);
    }
}
