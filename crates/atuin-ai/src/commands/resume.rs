//! `atuin ai resume [QUERY]`: pick a captured AI coding-agent session and resume it.
//!
//! - A QUERY that is a session id (or a unique prefix of one) resumes it directly, no picker.
//!   From the shell widget, `enter_accept` decides whether it runs or lands on the command line;
//!   a session that can't be resumed opens the picker on it, so the reason shows. An id that
//!   names several sessions (a prefix of several, or one id several agents have) opens the
//!   picker on them, to pick one; without a terminal to run it on, it fails, listing them.
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

use atuin_client::ai_session::AtuinSessionId;
use atuin_client::settings::{AiSessionFilterMode, Settings};
use atuin_client::theme::ThemeManager;
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use clap::Args;
use eyre::{Result, bail};

use super::session::one_line;
use crate::resume_tui::resumer::{HarnessResumer, Resume, shell_line};
use crate::resume_tui::sidecar::SidecarSource;
use crate::resume_tui::source::{Relation, harness_label};
use crate::resume_tui::{
    Outcome, Picker, ResumeContext, ResumePlan, Resumer, SessionRow, SessionSource,
};

const ACCEPT_PREFIX: &str = "__atuin_accept__:";

/// Ids shorter than this are treated as search text, so a short word can't resume by accident.
const MIN_ID_PREFIX: usize = 6;

/// The most of a session's title a line of output outside the picker shows.
const TITLE_WIDTH: usize = 72;

/// A session's title (agent-supplied, so untrusted) for a line of output outside the picker: on
/// one line, without control characters, so it can't break the line or send the terminal escape
/// sequences.
fn title_line(title: &str) -> String {
    one_line(title, TITLE_WIDTH)
}

#[derive(Args, Debug)]
pub struct Cmd {
    /// Search text, or a session id (or unique id prefix) to resume without the picker.
    #[arg(value_name = "QUERY")]
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

/// The single session `query` names, if it is an exact id or a unique prefix (an id several
/// harnesses have names none).
async fn direct_match(source: &dyn SessionSource, query: &str) -> Result<Option<SessionRow>> {
    if !looks_like_id(query) {
        return Ok(None);
    }
    let mut found = source.find_by_id(query).await?;
    let atuin_id = query.parse::<AtuinSessionId>().ok();
    let exact: Vec<usize> = (0..found.len())
        .filter(|&i| {
            let row = &found[i];
            row.handle.session.as_ref() == query || atuin_id == Some(row.atuin_id)
        })
        .collect();
    if let [exact] = exact.as_slice() {
        return Ok(Some(found.swap_remove(*exact)));
    }
    Ok(if found.len() == 1 {
        found.pop()
    } else {
        None
    })
}

/// Whether the picker has a terminal to run on: stdout, or `/dev/tty` when stdout is captured.
fn picker_has_a_terminal() -> bool {
    io::stdout().is_terminal()
        || std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").is_ok()
}

/// What to say when `query` names several sessions and there's no terminal to pick one on: each
/// on a line of its own, with its harness, when it last changed (in `offset`) and its title.
fn ambiguous_note(query: &str, found: &[SessionRow], offset: time::UtcOffset) -> String {
    let format = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]");
    let width = found.iter().map(|r| harness_label(r.handle.harness).len()).max().unwrap_or(0);
    let mut note = format!(
        "{query} names {} sessions; run `atuin ai resume {query}` on a terminal to pick one:",
        found.len()
    );
    for row in found {
        let when = row.updated_at.to_offset(offset).format(&format).unwrap_or_default();
        note.push_str(&format!(
            "\n  {:<width$}  {when}  {}  ({})",
            harness_label(row.handle.harness),
            title_line(&row.title.text),
            row.handle.session.as_ref().escape_non_printable(),
        ));
    }
    note
}

/// The sessions `query` names when it is an id naming several (see [`direct_match`]), for the
/// picker to open on; empty when it names one or none. Without a terminal to pick one on, an
/// error listing them.
async fn ambiguous_matches(
    source: &dyn SessionSource,
    query: &str,
    terminal: bool,
) -> Result<Vec<SessionRow>> {
    if !looks_like_id(query) {
        return Ok(Vec::new());
    }
    let found = source.find_by_id(query).await?;
    if found.len() < 2 {
        return Ok(Vec::new());
    }
    if !terminal {
        let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
        bail!("{}", ambiguous_note(query, &found, offset));
    }
    Ok(found)
}

/// The session `query` names directly (see [`direct_match`]). A subagent never resumes, so one
/// named stands for the session it works for, with a note saying so.
async fn direct_target(
    source: &dyn SessionSource,
    query: &str,
) -> Result<Option<(SessionRow, Option<String>)>> {
    let Some(mut row) = direct_match(source, query).await? else {
        return Ok(None);
    };
    let named = row.handle.session.clone();
    // Subagents can spawn subagents; the bound only guards against a cycle in bad data.
    for _ in 0..16 {
        if row.relation != Relation::Subagent {
            break;
        }
        let Some(parent) = row.parent.clone() else {
            break;
        };
        let found = source.find_by_id(parent.session.as_ref()).await?;
        let Some(up) = found.into_iter().find(|r| r.handle == parent) else {
            bail!(
                "{} is a subagent, which can't be resumed, and the session it works for ({}) \
                 isn't in the session database",
                named.as_ref().escape_non_printable(),
                parent.session.as_ref().escape_non_printable()
            );
        };
        row = up;
    }
    let note = (row.handle.session != named).then(|| {
        format!(
            "{} is a subagent, which can't be resumed: using the session it works for instead, {} \
             ({:?})",
            named.as_ref().escape_non_printable(),
            row.handle.session.as_ref().escape_non_printable(),
            title_line(&row.title.text)
        )
    });
    Ok(Some((row, note)))
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

    let context = ResumeContext::current().await?;
    let path = Settings::ai_session_sidecar_path();
    let source: Arc<dyn SessionSource> = Arc::new(SidecarSource::open(&path, &context).await?);
    let resumer: Arc<dyn Resumer> =
        Arc::new(HarnessResumer::new(context.clone(), settings.ai.sessions.resume.clone()));

    let mut preselect = None;
    let target = direct_target(source.as_ref(), query.trim()).await?;
    // An id naming several sessions opens the picker on them, to pick one.
    let matches = if target.is_none() {
        ambiguous_matches(source.as_ref(), query.trim(), picker_has_a_terminal()).await?
    } else {
        Vec::new()
    };
    if let Some((row, note)) = target {
        // The widget reads stderr for the command: nothing else may go there.
        if let Some(note) = note
            && output != Output::Widget
        {
            eprintln!("atuin: {note}");
        }
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
                    eprintln!(
                        "atuin: restored the session from sync; {}",
                        note.escape_non_printable()
                    );
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
        matches,
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
            let line = printable_line(&plan)?;
            if run {
                eprintln!("{ACCEPT_PREFIX}{line}");
            } else {
                eprintln!("{line}");
            }
            Ok(())
        }
        Output::Exec if run => exec(&plan),
        Output::Print | Output::Exec => {
            let line = printable_line(&plan)?;
            let mut out = io::stdout().lock();
            writeln!(out, "{line}")?;
            Ok(())
        }
    }
}

/// The plan's shell line, to print. Quoted, it would still carry any control characters in the
/// session's id or directory (agent-supplied) to the terminal, and a newline would end the
/// widget's line early: such a plan is refused, showing them escaped.
fn printable_line(plan: &ResumePlan) -> Result<String> {
    let line = shell_line(plan);
    // A tab, quoted, is only a wide space.
    if line.contains(|c: char| c.is_control() && c != '\t') {
        bail!(
            "the session's resume command has control characters in it, so atuin won't print or \
             run it: {}",
            line.escape_non_printable()
        );
    }
    Ok(line)
}

/// Change into the session's directory and replace this process with the harness.
fn exec(plan: &ResumePlan) -> Result<()> {
    // On Windows, spawn the file the resumability check found (`claude.cmd`), which the bare name
    // would not resolve to. It is looked up as the check looked it up (a relative path against the
    // session's directory), and made absolute before changing directory, so a relative `PATH` entry
    // such as `.` finds what the check found; elsewhere the name is looked up as it was checked.
    #[cfg(windows)]
    use crate::resume_tui::resumer;
    #[cfg(windows)]
    let program = match resumer::find_program(&resumer::program_to_check(plan)) {
        Some(found) => std::path::absolute(found)?,
        None => plan.program.clone().into(),
    };
    #[cfg(not(windows))]
    let program = &plan.program;
    if let Some(cwd) = &plan.cwd {
        std::env::set_current_dir(cwd)?;
    }
    let mut command = std::process::Command::new(program);
    command.args(&plan.args);
    if let Some(cwd) = &plan.cwd {
        // Keep $PWD in step, so harnesses that read it see the session's directory.
        command.env("PWD", cwd);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = command.exec();
        bail!("failed to run {}: {err}", plan.command().escape_non_printable())
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
    use crate::resume_tui::fake::{self, FakeSource};

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
    #[case::whole(32)]
    #[case::past_the_timestamp(16)]
    #[tokio::test]
    async fn an_atuin_id_resumes_directly(#[case] len: usize) {
        let source = FakeSource::new();
        let wanted = source.find_by_id("7f3c9a12").await.unwrap().remove(0);
        let id = wanted.atuin_id.to_string();

        let row = direct_match(&source, &id[..len]).await.unwrap().unwrap();
        assert_eq!(row.handle, wanted.handle);
    }

    /// An id several agents have names no session to resume directly, and without a
    /// terminal to pick one on, each is listed.
    #[rstest]
    #[tokio::test]
    async fn an_id_several_agents_have_is_ambiguous() {
        use atuin_client::ai_session::HarnessKind;

        let mut claude = fake::row(HarnessKind::ClaudeCode, "7aaabc31-1631", "fix the flaky test");
        claude.updated_at = time::macros::datetime!(2026-09-27 14:03 UTC);
        let mut pi = fake::row(HarnessKind::Pi, "7aaabc31-1631", "plan the dotfiles sync");
        pi.updated_at = time::macros::datetime!(2026-09-26 09:12 UTC);
        let source = FakeSource::from_rows(vec![claude.clone(), pi.clone()]);
        assert!(direct_match(&source, "7aaabc31-1631").await.unwrap().is_none());
        assert_eq!(
            ambiguous_note("7aaabc31", &[claude, pi], time::UtcOffset::UTC),
            [
                "7aaabc31 names 2 sessions; run `atuin ai resume 7aaabc31` on a terminal to pick \
                 one:",
                "  Claude Code  2026-09-27 14:03  fix the flaky test  (7aaabc31-1631)",
                "  Pi           2026-09-26 09:12  plan the dotfiles sync  (7aaabc31-1631)",
            ]
            .join("\n")
        );
    }

    /// An id naming several sessions opens the picker on all of them, given a terminal to pick
    /// one on; without one, it fails, listing them. A unique id, or a query that isn't an id,
    /// pins nothing.
    #[rstest]
    #[tokio::test]
    async fn an_ambiguous_id_opens_the_picker_on_its_sessions() {
        let source = FakeSource::new();
        let matches = ambiguous_matches(&source, "ses_4b8e2f1a9c3d7e", true).await.unwrap();
        let ids: Vec<_> = matches.iter().map(|r| r.handle.session.as_ref()).collect();
        assert!(ids.len() > 1, "{ids:?}");
        assert!(ids.iter().all(|id| id.starts_with("ses_4b8e2f1a9c3d7e")), "{ids:?}");

        let err = ambiguous_matches(&source, "ses_4b8e2f1a9c3d7e", false).await.unwrap_err();
        let err = err.to_string();
        assert!(
            err.starts_with(&format!("ses_4b8e2f1a9c3d7e names {} sessions", ids.len())),
            "{err}"
        );
        assert!(ids.iter().all(|id| err.contains(id)), "{err}");

        for query in ["ses_4b8e2f1a9c3d7e6f", "7f3c9a12", "flaky"] {
            assert!(ambiguous_matches(&source, query, false).await.unwrap().is_empty(), "{query}");
        }
    }

    /// A subagent named by id stands for the session it works for, saying so; others are
    /// themselves. `agent-c9d0e1f2` works for a fork, which resumes like any session.
    #[rstest]
    #[case::subagent("agent-a1b2c3d4", "7f3c9a12-5be0-4d7e-9c41-0a8e2b6f4d10", true)]
    #[case::subagent_of_a_fork("agent-c9d0e1f2", "0c1d2e3f-8a9b-4c5d-9e0f-112233445566", true)]
    #[case::fork("0c1d2e3f", "0c1d2e3f-8a9b-4c5d-9e0f-112233445566", false)]
    #[tokio::test]
    async fn a_subagent_resumes_the_session_it_works_for(
        #[case] query: &str,
        #[case] target: &str,
        #[case] redirected: bool,
    ) {
        let source = FakeSource::new();
        let (row, note) = direct_target(&source, query).await.unwrap().unwrap();
        assert_eq!(row.handle.session.as_ref(), target);
        assert_eq!(note.is_some(), redirected, "{note:?}");
        if let Some(note) = note {
            assert!(note.starts_with(&format!("{query} is a subagent")), "{note}");
            assert!(note.contains(target), "{note}");
        }
    }

    /// Titles and ids are agent-supplied: the note for an ambiguous id shows each on one line,
    /// without the control characters that would let one move the cursor or retitle the
    /// terminal.
    #[rstest]
    #[tokio::test]
    async fn the_ambiguous_note_escapes_titles_and_ids() {
        use atuin_client::ai_session::HarnessKind;

        let hostile = "fix\nthe \x1b]0;pwned\x07flaky\r\ttest\u{9b}2J";
        let mut claude = fake::row(HarnessKind::ClaudeCode, "7aaabc31-1631", hostile);
        claude.updated_at = time::macros::datetime!(2026-09-27 14:03 UTC);
        let mut pi = fake::row(HarnessKind::Pi, "7aaabc31-1631\x1b[2J", "plan");
        pi.updated_at = time::macros::datetime!(2026-09-26 09:12 UTC);
        let note = ambiguous_note("7aaabc31", &[claude, pi], time::UtcOffset::UTC);
        assert!(!note.contains(|c: char| c.is_control() && c != '\n'), "{note:?}");
        assert_eq!(note.lines().count(), 3, "{note}");
        assert!(note.contains("  fix the ]0;pwnedflaky test2J  (7aaabc31-1631)"), "{note}");
        assert!(note.contains("  plan  (7aaabc31-1631^[[2J)"), "{note}");
        assert!(title_line(&"long ".repeat(40)).chars().count() <= TITLE_WIDTH + 1);
    }

    /// A plan whose id or directory carries control characters isn't printed (or handed to the
    /// widget) as it is; one without them is.
    #[rstest]
    #[case::plain("abc", true)]
    #[case::tab("abc\tdef", true)]
    #[case::escape("abc\x1b]0;pwned\x07", false)]
    #[case::newline("abc\nrm -rf ~", false)]
    fn control_characters_never_reach_the_terminal_in_a_command(
        #[case] id: &str,
        #[case] printable: bool,
    ) {
        let plan = ResumePlan {
            program: "claude".to_owned(),
            args: vec!["--resume".to_owned(), id.to_owned()],
            cwd: None,
            cwd_requirement: atuin_common::harnesstools::resume::CwdRequirement::Preferred,
            native_path: None,
        };
        match printable_line(&plan) {
            Ok(line) => {
                assert!(printable, "{line:?}");
                let id = atuin_common::harnesstools::resume::quote(id);
                assert_eq!(line, format!("claude --resume {id}"));
            }
            Err(err) => {
                assert!(!printable);
                let err = err.to_string();
                assert!(!err.contains(char::is_control), "{err:?}");
                assert!(err.contains("abc^"), "{err}");
            }
        }
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
