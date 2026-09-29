//! `atuin ai resume [QUERY]`: pick a captured AI coding-agent session and resume it.
//!
//! - A QUERY that is a session id (or a unique prefix of one) resumes it directly, no picker.
//!   From the shell widget, `enter_accept` decides whether it runs or lands on the command line;
//!   a session that can't be resumed opens the picker on it, so the reason shows. An id that
//!   names several sessions (a prefix of several, or one id several harnesses have) opens the
//!   picker to pick one; without a terminal to run it on, they are listed on stderr instead, and
//!   it fails.
//! - A session whose transcript isn't on this machine (recorded on another host, or deleted) is
//!   restored from the synced messages first: the harness's own transcript is written out, then
//!   resumed with the usual command. That happens only once a session is chosen (enter or tab in
//!   the picker, or named by id), in this process, before the command is run or handed to the
//!   shell widget, so the command the widget puts on the command line works as it stands.
//! - A transcript that is here but behind is caught up with sync the same way, and a session
//!   that went on separately on several machines resumes the branch picked in the chooser (the
//!   newest, by id, with a note naming the others); see [`crate::resume_tui::catchup`]. A
//!   session another host wrote to in the last few minutes asks first on a terminal (a note
//!   otherwise): resuming it here branches it.
//! - `--branch <selector>` names the branch of such a session to resume instead (and made the
//!   tip, as the chooser does), or to continue with `--in`: `this` for this machine's, `@<host>`
//!   for another's, or the start of the branch's id (what the picker's ctrl-y copies; see
//!   [`catchup::pick_branch`]). Anything that names no single branch fails, listing them.
//! - `--in <harness>` continues the session the id names in another harness instead: it is
//!   written out there as a new session (its tool calls flattened into notes; see
//!   [`atuin_common::harnesstools::continuation`]) and that is resumed, the same way. In the
//!   picker, accepting a session asks where to resume it: its own harness first, then the others
//!   installed here (see [`crate::resume_tui::chooser`]; `[ai.sessions] resume_chooser = false`
//!   skips that).
//! - From the shell widget (`--shell-widget`), the result goes to stderr using the history
//!   search's protocol: `__atuin_accept__:<cmd>` to run it, plain `<cmd>` to edit it, nothing to
//!   leave the command line alone.
//! - Standalone on a terminal, resuming changes directory and execs the harness.
//! - With `--print`, or when stdout is not a terminal, the command is printed instead.
//!
//! The picker reads the sidecar database directly and never waits for the daemon.

use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;

use atuin_client::ai_session::{HarnessKind, Head, SourceId};
use atuin_client::settings::{AiSessionFilterMode, Settings};
use atuin_client::theme::ThemeManager;
use clap::Args;
use eyre::{Result, bail, eyre};

use crate::resume_tui::catchup::{self, Synced};
use crate::resume_tui::fake::{FakeResumer, FakeSource};
use crate::resume_tui::resumer::{HarnessResumer, shell_line};
use crate::resume_tui::sidecar::{HostNameSource, SidecarSource};
use crate::resume_tui::source::{Relation, harness_label};
use crate::resume_tui::{
    Outcome, Picker, ResumeContext, ResumePlan, Resumer, SessionRow, SessionSource,
};

const ACCEPT_PREFIX: &str = "__atuin_accept__:";

/// Ids shorter than this are treated as search text, so a short word can't resume by accident.
const MIN_ID_PREFIX: usize = 6;

#[derive(Args, Debug)]
pub struct Cmd {
    /// Search text, or a session id (or unique id prefix) to resume without the picker.
    #[arg(value_name = "QUERY")]
    query: Vec<String>,

    /// Print the resume command instead of running it.
    #[arg(long)]
    print: bool,

    /// Continue the session QUERY names (by id, or a unique id prefix) in another harness: it is
    /// written out as a new session there, tool calls flattened into notes, and resumed.
    #[arg(long = "in", value_enum, value_name = "HARNESS")]
    continue_in: Option<ContinueIn>,

    /// The branch of a session that went on separately on several machines to resume (or
    /// continue, with `--in`): `this` for this machine's, `@<host>` for another host's, or the
    /// start of the branch's id, as the picker's ctrl-y copies it. Needs a session id.
    #[arg(long, value_name = "BRANCH")]
    branch: Option<String>,

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

/// A harness to continue a session in (`--in`).
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum ContinueIn {
    Claude,
    Codex,
    Opencode,
    Pi,
}

impl ContinueIn {
    fn kind(self) -> HarnessKind {
        match self {
            Self::Claude => HarnessKind::ClaudeCode,
            Self::Codex => HarnessKind::Codex,
            Self::Opencode => HarnessKind::Opencode,
            Self::Pi => HarnessKind::Pi,
        }
    }
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
    let exact: Vec<usize> =
        (0..found.len()).filter(|&i| found[i].handle.session.as_ref() == query).collect();
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
            row.title.text,
            row.handle.session,
        ));
    }
    note
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
                "{named} is a subagent, which can't be resumed, and the session it works for ({}) \
                 isn't in the session database",
                parent.session
            );
        };
        row = up;
    }
    let note = (row.handle.session != named).then(|| {
        format!(
            "{named} is a subagent, which can't be resumed: using the session it works for \
             instead, {} ({:?})",
            row.handle.session, row.title.text
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

    if let Some(into) = cmd.continue_in {
        let (plan, status) = continue_plan(
            source.as_ref(),
            resumer.as_ref(),
            &context,
            query.trim(),
            into,
            cmd.branch.as_deref(),
        )
        .await?;
        // The widget reads stderr for the command: nothing else may go there.
        if output != Output::Widget {
            eprintln!("atuin: {status}");
        }
        return finish(direct_outcome(plan, output, settings.enter_accept), output);
    }

    let mut preselect = None;
    let target = direct_target(source.as_ref(), query.trim()).await?;
    let head = match (&cmd.branch, &target) {
        (None, _) => None,
        (Some(selector), Some((row, _))) => {
            Some(branch_head(source.as_ref(), row, &context, selector).await?)
        }
        (Some(_), None) => bail!(
            "`--branch` picks a branch of the session an id names: no single session has the id \
             {:?}",
            query.trim()
        ),
    };
    // An id naming several sessions has the picker ask which: without a terminal, say which.
    if target.is_none() && looks_like_id(query.trim()) && !picker_has_a_terminal() {
        let found = source.find_by_id(query.trim()).await?;
        if found.len() > 1 {
            let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
            eprintln!("atuin: {}", ambiguous_note(query.trim(), &found, offset));
            // The list says it all: no error report after it.
            std::process::exit(1);
        }
    }
    if let Some((row, note)) = target {
        // The widget reads stderr for the command: nothing else may go there.
        if let Some(note) = note
            && output != Output::Widget
        {
            eprintln!("atuin: {note}");
        }
        if !confirm_live_elsewhere(source.as_ref(), &row, &context, output, head.as_ref()).await? {
            return Ok(());
        }
        let plan = match resumer.sync(source.as_ref(), &row, head.as_ref()).await {
            Ok(synced) => {
                // The widget reads stderr for the command: nothing else may go there.
                if output != Output::Widget {
                    for note in sync_notes(source.as_ref(), &row, &synced, &context).await {
                        eprintln!("atuin: {note}");
                    }
                }
                Ok(synced.plan)
            }
            Err(why) => Err(why),
        };
        match plan {
            Ok(plan) => return finish(direct_outcome(plan, output, settings.enter_accept), output),
            // The branch named can't be resumed as asked: say why rather than offer another.
            Err(why) if head.is_some() => bail!("can't resume {}: {why}", row.handle.session),
            // Open the picker on it instead, so the reason shows (and another can be picked).
            Err(why) => preselect = Some((row, why)),
        }
    }

    let mut themes = ThemeManager::new(settings.theme.debug, None);
    let theme = themes.load_theme(settings.theme.name.as_str(), settings.theme.max_depth);
    let (outcome, note) = Picker {
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
    if let Some(note) = note
        && output != Output::Widget
    {
        eprintln!("atuin: {note}");
    }
    finish(outcome, output)
}

/// Other hosts' names by id (simple form), for naming `heads`' hosts: read (which may be slow)
/// only when one of them is another host's; `row`'s own host is always known.
async fn head_host_names(
    source: &dyn SessionSource,
    row: &SessionRow,
    context: &ResumeContext,
    heads: &[&Head],
) -> HashMap<String, String> {
    let elsewhere = heads
        .iter()
        .any(|h| h.host.is_some_and(|host| host.0.as_simple().to_string() != context.host_id));
    let mut names = if elsewhere {
        source.host_names().await.unwrap_or_default()
    } else {
        HashMap::new()
    };
    names.entry(row.host_id.clone()).or_insert_with(|| row.hostname.clone());
    names
}

/// `head`'s host as the picker names it: `this machine`, `@MacBook-Pro-3`.
fn head_host(head: &Head, context: &ResumeContext, names: &HashMap<String, String>) -> String {
    catchup::host_label(head, &context.host_id, &|id| names.get(id).cloned())
}

/// The head `selector` names of `row` (`--branch`; see [`catchup::pick_branch`]), among its
/// heads as they are now.
async fn branch_head(
    source: &dyn SessionSource,
    row: &SessionRow,
    context: &ResumeContext,
    selector: &str,
) -> Result<SourceId> {
    let heads = match source.heads(&row.handle).await? {
        Some(heads) => heads.heads,
        None => row.heads.clone(),
    };
    let all: Vec<&Head> = heads.iter().collect();
    let names = head_host_names(source, row, context, &all).await;
    let now = time::OffsetDateTime::now_utc();
    catchup::pick_branch(&heads, selector, now, &|h| head_host(h, context, &names))
        .map(|head| head.source_id.clone())
        .map_err(|why| eyre!("--branch: session {}: {why}", row.handle.session))
}

/// What to say before resuming `head` when another host wrote to it in the last few minutes (see
/// [`catchup::live_elsewhere`]); `None` when none did.
fn live_elsewhere_note(
    head: &Head,
    context: &ResumeContext,
    now: time::OffsetDateTime,
    names: &HashMap<String, String>,
) -> Option<String> {
    let ago = catchup::live_elsewhere(head, &context.host_id, now)?;
    let ago = match ago.whole_minutes() {
        0 => "just now".to_owned(),
        n => format!("{n}m ago"),
    };
    Some(format!(
        "still active on {} ({ago}): resuming here will branch the session",
        head_host(head, context, names)
    ))
}

/// Ask `question` on `out`, reading the answer from `input`: only yes is yes.
fn ask(question: &str, input: &mut impl io::BufRead, out: &mut impl Write) -> io::Result<bool> {
    write!(out, "{question} [y/N] ")?;
    out.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Before resuming `row` by id (at `head`, else its newest): when another host may still be
/// working on it, say that resuming here will branch it, on a terminal as a question (`false`
/// when the answer is no), else as a note on stderr. Not from the widget, which reads stderr for
/// the command.
async fn confirm_live_elsewhere(
    source: &dyn SessionSource,
    row: &SessionRow,
    context: &ResumeContext,
    output: Output,
    head: Option<&SourceId>,
) -> Result<bool> {
    if output == Output::Widget {
        return Ok(true);
    }
    let Some(heads) = source.heads(&row.handle).await.ok().flatten() else {
        return Ok(true);
    };
    let picked = head.and_then(|p| heads.heads.iter().find(|h| &h.source_id == p));
    let Some(resumed) = picked.or(heads.latest()) else {
        return Ok(true);
    };
    let now = time::OffsetDateTime::now_utc();
    if catchup::live_elsewhere(resumed, &context.host_id, now).is_none() {
        return Ok(true);
    }
    let names = head_host_names(source, row, context, &[resumed]).await;
    let Some(note) = live_elsewhere_note(resumed, context, now, &names) else {
        return Ok(true);
    };
    if io::stdin().is_terminal() && io::stderr().is_terminal() {
        let question = format!("atuin: {note}. Resume anyway?");
        Ok(ask(&question, &mut io::stdin().lock(), &mut io::stderr())?)
    } else {
        eprintln!("atuin: {note}");
        Ok(true)
    }
}

/// What to tell the user about catching `row` up: what was done (`caught up 136 messages from
/// @MacBook-Pro-3`), and for a session that went on separately on several machines, the
/// branches not resumed.
async fn sync_notes(
    source: &dyn SessionSource,
    row: &SessionRow,
    synced: &Synced,
    context: &ResumeContext,
) -> Vec<String> {
    let heads: Vec<&Head> = synced.head.iter().chain(&synced.others).collect();
    let names = head_host_names(source, row, context, &heads).await;
    let host = |h: &Head| head_host(h, context, &names);
    let now = time::OffsetDateTime::now_utc();
    synced
        .status(row.handle.harness, &host)
        .into_iter()
        .chain(synced.other_branches(now, &host))
        .collect()
}

/// `atuin ai resume <id> --in <harness> [--branch <selector>]`: write the session `query` names
/// (the branch `branch` picks, else its newest) out as a new session of `into`, and the plan
/// that resumes it, with the status line saying what was flattened.
async fn continue_plan(
    source: &dyn SessionSource,
    resumer: &dyn Resumer,
    context: &ResumeContext,
    query: &str,
    into: ContinueIn,
    branch: Option<&str>,
) -> Result<(ResumePlan, String)> {
    let Some((row, redirected)) = direct_target(source, query).await? else {
        bail!("`--in` continues the session an id names: no single session has the id {query:?}");
    };
    let head = match branch {
        Some(selector) => Some(branch_head(source, &row, context, selector).await?),
        None => None,
    };
    let continued = resumer
        .continue_in(source, &row, into.kind(), head.as_ref())
        .await
        .map_err(|why| eyre::eyre!("can't continue {}: {why}", row.handle.session))?;
    let mut status = continued.status();
    if let Some(redirected) = redirected {
        status = format!("{redirected}; {status}");
    }
    if let Some(note) = &continued.note {
        status.push_str(&format!("; {note}"));
    }
    Ok((continued.plan, status))
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
    use crate::resume_tui::fake;

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

    /// An id several harnesses have names no session to resume directly, and without a
    /// terminal to pick one on, each is listed.
    #[rstest]
    #[tokio::test]
    async fn an_id_several_harnesses_have_is_ambiguous() {
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

        let resumer = FakeResumer::default();
        let (plan, status) =
            continue_plan(&source, &resumer, &fake::context(), query, ContinueIn::Codex, None)
                .await
                .unwrap();
        assert_eq!(plan.args[1], format!("continued-{target}"));
        assert_eq!(status.contains("is a subagent"), redirected, "{status}");
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

    /// `atuin ai resume <id> --in <harness>` continues the session the id names, and plans
    /// resuming the new session; a query that names no single session is an error.
    #[rstest]
    #[tokio::test]
    async fn continuing_by_id_plans_the_new_session() {
        let source = FakeSource::new();
        let resumer = FakeResumer::default();
        let (plan, status) =
            continue_plan(&source, &resumer, &fake::context(), "7f3c9a12", ContinueIn::Codex, None)
                .await
                .unwrap();
        assert_eq!(plan.program, "codex");
        assert_eq!(plan.args, ["resume", "continued-7f3c9a12-5be0-4d7e-9c41-0a8e2b6f4d10"]);
        assert_eq!(
            status,
            "continuing in Codex: 42 tool calls flattened to notes, reasoning dropped"
        );

        let err =
            continue_plan(&source, &resumer, &fake::context(), "fix flaky", ContinueIn::Pi, None)
                .await
                .unwrap_err();
        assert!(err.to_string().contains("no single session has the id"), "{err}");

        let cli = Cli::try_parse_from(["resume", "7f3c9a12", "--in", "opencode"]).unwrap();
        assert_eq!(cli.cmd.continue_in, Some(ContinueIn::Opencode));
        assert!(Cli::try_parse_from(["resume", "7f3c9a12", "--in", "cursor"]).is_err());
    }

    /// Resuming by id a session another host wrote to minutes ago says it will branch it.
    #[rstest]
    #[case::minutes_ago(fake::OTHER_HOST_ID, 2, true)]
    #[case::a_while_ago(fake::OTHER_HOST_ID, 10, false)]
    #[case::this_host(fake::THIS_HOST_ID, 1, false)]
    fn a_session_live_elsewhere_is_noted(
        #[case] host: &str,
        #[case] minutes: i64,
        #[case] noted: bool,
    ) {
        let mut heads = fake::diverged_branches().heads;
        heads.heads[0].host = Some(fake::host(host));
        heads.heads[0].last_at = fake::now() - time::Duration::minutes(minutes);
        let names = HashMap::from([(fake::OTHER_HOST_ID.to_owned(), "buildbox.lan".to_owned())]);
        let note = live_elsewhere_note(&heads.heads[0], &fake::context(), fake::now(), &names);
        assert_eq!(note.is_some(), noted, "{note:?}");
        if noted {
            assert_eq!(
                note.unwrap(),
                "still active on @buildbox (2m ago): resuming here will branch the session"
            );
        }
    }

    /// On a terminal it asks, and only yes goes on.
    #[rstest]
    #[case("y\n", true)]
    #[case("YES\n", true)]
    #[case("\n", false)]
    #[case("n\n", false)]
    #[case("", false)]
    fn asking_takes_only_yes(#[case] answer: &str, #[case] yes: bool) {
        let mut out = Vec::new();
        assert_eq!(ask("Resume anyway?", &mut answer.as_bytes(), &mut out).unwrap(), yes);
        assert_eq!(String::from_utf8(out).unwrap(), "Resume anyway? [y/N] ");
    }

    /// A diverged session resumed by id resumes the latest branch, saying what was caught up
    /// and naming the branches it didn't resume.
    #[rstest]
    #[tokio::test]
    async fn resuming_a_diverged_session_by_id_names_the_other_branches() {
        use crate::resume_tui::catchup::Caught;

        let source = FakeSource::new();
        let row = direct_match(&source, fake::DIVERGED).await.unwrap().unwrap();
        let heads = fake::diverged_branches().heads.heads;
        let plan = FakeResumer::default().plan(&row).await.unwrap().plan;
        let synced = Synced {
            plan,
            caught: Caught::Switched { messages: 40 },
            head: Some(heads[0].clone()),
            others: vec![heads[1].clone()],
        };
        let notes = sync_notes(&source, &row, &synced, &fake::context()).await;
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(
            notes[0],
            "switched to @buildbox's branch: 40 messages added beside this machine's"
        );
        assert!(
            notes[1].starts_with(
                "this session went on separately on several machines; resuming @buildbox's branch \
                 (the latest). Not resumed: this machine · "
            ),
            "{}",
            notes[1]
        );
        assert!(notes[1].ends_with(" · 24 msgs"), "{}", notes[1]);
    }

    /// A [`FakeResumer`] noting which head each catch-up and continuation was asked for.
    #[derive(Default)]
    struct Recording {
        inner: FakeResumer,
        heads: parking_lot::Mutex<Vec<Option<SourceId>>>,
    }

    #[async_trait::async_trait]
    impl Resumer for Recording {
        async fn plan(
            &self,
            session: &SessionRow,
        ) -> Result<crate::resume_tui::resumer::Resume, crate::resume_tui::resumer::NotResumable>
        {
            self.inner.plan(session).await
        }

        async fn restore(
            &self,
            source: &dyn SessionSource,
            session: &SessionRow,
            restore: &crate::resume_tui::resumer::Restore,
        ) -> Result<ResumePlan, crate::resume_tui::resumer::NotResumable> {
            self.inner.restore(source, session, restore).await
        }

        async fn continue_in(
            &self,
            source: &dyn SessionSource,
            session: &SessionRow,
            target: HarnessKind,
            head: Option<&SourceId>,
        ) -> Result<crate::resume_tui::resumer::Continued, crate::resume_tui::resumer::NotResumable>
        {
            self.heads.lock().push(head.cloned());
            self.inner.continue_in(source, session, target, head).await
        }
    }

    /// `--branch` names a branch of the session by host (`this`, `@host`) or by the start of its
    /// id; anything naming no single branch fails, listing them.
    #[rstest]
    #[case::this_machine("this", Ok("h24"))]
    #[case::another_host("@buildbox", Ok("b40"))]
    #[case::its_full_name("@buildbox.lan", Ok("b40"))]
    #[case::by_id("h24", Ok("h24"))]
    #[case::no_such_host("@MacBook-Pro-3", Err("no branch is \"@MacBook-Pro-3\""))]
    #[tokio::test]
    async fn branches_are_named_by_host_or_id(
        #[case] selector: &str,
        #[case] want: Result<&str, &str>,
    ) {
        let source = FakeSource::new();
        let row = direct_match(&source, fake::DIVERGED).await.unwrap().unwrap();
        let head = branch_head(&source, &row, &fake::context(), selector).await;
        match want {
            Ok(id) => assert_eq!(head.unwrap().as_ref(), id),
            Err(why) => {
                let err = head.unwrap_err().to_string();
                assert!(err.starts_with(&format!("--branch: session {}: ", fake::DIVERGED)));
                assert!(err.contains(why), "{err}");
                assert!(err.contains("\n  b40  @buildbox · "), "lists the branches: {err}");
                assert!(err.contains("\n  h24  this machine · "), "lists the branches: {err}");
            }
        }
    }

    /// `--in` with `--branch` continues the branch named; without, the newest.
    #[rstest]
    #[case::named(Some("this"), Some("h24"))]
    #[case::the_newest(None, None)]
    #[tokio::test]
    async fn continuing_by_id_continues_the_branch_named(
        #[case] branch: Option<&str>,
        #[case] head: Option<&str>,
    ) {
        let source = FakeSource::new();
        let resumer = Recording::default();
        let context = fake::context();
        let continued =
            continue_plan(&source, &resumer, &context, fake::DIVERGED, ContinueIn::Pi, branch);
        continued.await.unwrap();
        let heads = resumer.heads.lock().clone();
        assert_eq!(heads, [head.map(|h| SourceId::from(h.to_owned()))]);

        let err =
            continue_plan(&source, &resumer, &context, fake::DIVERGED, ContinueIn::Pi, Some("zz"));
        let err = err.await.unwrap_err().to_string();
        assert!(err.contains("no branch is \"zz\""), "{err}");
        assert_eq!(resumer.heads.lock().len(), 1, "nothing continued");

        let cli =
            Cli::try_parse_from(["resume", "b2c4d6e8", "--in", "pi", "--branch", "@buildbox"])
                .unwrap();
        assert_eq!(cli.cmd.branch.as_deref(), Some("@buildbox"));
        assert_eq!(cli.cmd.continue_in, Some(ContinueIn::Pi));
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
