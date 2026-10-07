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
//! - `--in <harness>` continues the session the id names in another harness instead: it is
//!   written out there as a new session (its tool calls flattened into notes; see
//!   [`atuin_common::harnesstools::continuation`]) and that is resumed, the same way. In the
//!   picker, accepting a session asks where to resume it: its own harness first, then the others
//!   installed here (see [`crate::resume_tui::chooser`]; `[ai.sessions] resume_chooser = false`
//!   skips that).
//! - A session whose transcript is here is caught up with sync first (see
//!   [`crate::resume_tui::catchup`]): when it went on on another machine, the messages this copy
//!   lacks are appended. When that can't be done (an agent here has it open, it went another way
//!   here, or this copy has messages sync hasn't got), nothing is written, and it fails saying
//!   so: `--as-is` resumes this copy as it is, `--fork` forks it instead, and `--switch` (for a
//!   copy that went another way, all of it synced) switches it to another branch, in place,
//!   under the same id: the history it shares with that branch is kept as it is, and the
//!   branch's messages since are appended from sync. The branch it was on stays in atuin, and the
//!   copy as it was is kept as a backup in atuin's data directory.
//! - `--fork` forks it instead: it is written out as a new session of its own harness, with the
//!   same history, linked to it as a fork (see [`atuin_common::harnesstools::fork`]), and that is
//!   resumed. The original is left as it is. A session that went on separately on several
//!   machines forks from its newest branch.
//! - `--branch` picks the branch to catch up to, to switch to, or to fork from: `this`,
//!   `@<host id>`, or the start of its head's id.
//! - `--as-is`, `--switch` and `--branch` need an id naming a single session that can resume here:
//!   anything else is an error (listing the sessions an id names), never the picker, which would
//!   drop them.
//! - From the shell widget (`--shell-widget`), the result goes to stderr using the history
//!   search's protocol: `__atuin_accept__:<cmd>` to run it, plain `<cmd>` to edit it, nothing to
//!   leave the command line alone.
//! - Standalone on a terminal, resuming changes directory and execs the harness.
//! - With `--print`, or when stdout is not a terminal, the command is printed instead.
//!
//! The picker reads the sidecar database directly and never waits for the daemon.

use std::io::{self, IsTerminal, Write};
use std::sync::Arc;

use atuin_client::ai_session::{Analysis, AtuinSessionId, HarnessKind, SourceId};
use atuin_client::settings::{AiSessionFilterMode, Settings};
use atuin_client::theme::ThemeManager;
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use clap::Args;
use eyre::{Result, bail};

use super::session::one_line;
use crate::resume_tui::catchup::{self, CatchUp, Held};
use crate::resume_tui::resumer::{HarnessResumer, NotResumable, Resume, quote, shell_line};
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

    /// Continue the session QUERY names (by id, or a unique id prefix) in another agent: it is
    /// written out as a new session there, tool calls flattened into notes, and resumed.
    #[arg(long = "in", value_enum, value_name = "HARNESS")]
    continue_in: Option<ContinueIn>,

    /// Fork the session QUERY names (by id, or a unique id prefix): it is written out as a new
    /// session of the same agent, with the same history, and resumed. The original is left as
    /// it is.
    #[arg(long, conflicts_with = "continue_in")]
    fork: bool,

    /// Resume this machine's copy of the session QUERY names as it is, without catching it up
    /// with sync.
    #[arg(long, conflicts_with_all = ["fork", "continue_in"])]
    as_is: bool,

    /// Switch this machine's copy of the session QUERY names to another branch (the one
    /// `--branch` names, else the newest it can be switched to), in place, and resume it: the
    /// history it shares with that branch is kept as it is, and the branch's messages since are
    /// appended from sync. The branch it was on stays in atuin, for `--fork` to bring back, and
    /// the copy as it was is kept as a backup. Only when sync holds all of it and no agent here
    /// has it open.
    #[arg(long, conflicts_with_all = ["fork", "continue_in", "as_is"])]
    switch: bool,

    /// The branch of the session QUERY names to catch up to, to switch to with `--switch`, or to
    /// fork from with `--fork`: `this`, `@<host id>`, or the start of its head's id. The newest
    /// by default.
    #[arg(long, value_name = "BRANCH", conflicts_with_all = ["continue_in", "as_is"])]
    branch: Option<String>,

    /// The filter the picker opens in (default: global).
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
    /// The flag given that only resumes the session an id names (`--switch`, `--branch`,
    /// `--as-is`), which the picker would drop: the first, as errors name it.
    fn single_session_flag(&self) -> Option<&'static str> {
        self.switch
            .then_some("--switch")
            .or(self.branch.as_ref().map(|_| "--branch"))
            .or(self.as_is.then_some("--as-is"))
    }

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
    let note = format!(
        "{query} names {} sessions; run `atuin ai resume {query}` on a terminal to pick one:",
        found.len()
    );
    note + &listing(found, offset)
}

/// `found`, each on a line of its own, with its harness, when it last changed (in `offset`) and
/// its title.
fn listing(found: &[SessionRow], offset: time::UtcOffset) -> String {
    let format = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]");
    let width = found.iter().map(|r| harness_label(r.handle.harness).len()).max().unwrap_or(0);
    let mut note = String::new();
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

/// What to say when `flag` (`--switch`, `--branch`, `--as-is`) is given with a `query` that names
/// no single session: the picker would drop the flag, so it is an error, listing what `query`
/// names.
fn be_more_specific(
    flag: &str,
    query: &str,
    found: &[SessionRow],
    offset: time::UtcOffset,
) -> String {
    let query = query.escape_non_printable();
    match found.len() {
        0 => format!("`{flag}` resumes the session an id names: no session has the id \"{query}\""),
        n => format!(
            "`{flag}` resumes the session an id names: {query} names {n} sessions; be more \
             specific:{}",
            listing(found, offset)
        ),
    }
}

/// The session named, `row`, can't be resumed here, for `why`: the picker opens on it instead, so
/// the reason shows (and another can be picked). With `flag` (`--switch`, `--branch`, `--as-is`)
/// given, which the picker would drop, an error saying why.
fn picker_on(
    row: SessionRow,
    why: NotResumable,
    flag: Option<&str>,
) -> Result<(SessionRow, NotResumable)> {
    if let Some(flag) = flag {
        let message = format!("can't resume {} with `{flag}`: {why}", row.handle.session);
        bail!("{}", message.escape_non_printable());
    }
    Ok((row, why))
}

/// With `flag` (`--switch`, `--branch`, `--as-is`) given and `query` naming no single session, the
/// error saying to be more specific ([`be_more_specific`]).
async fn no_single_session_for(
    source: &dyn SessionSource,
    query: &str,
    flag: Option<&str>,
) -> Result<()> {
    let Some(flag) = flag else {
        return Ok(());
    };
    let found = if looks_like_id(query) {
        source.find_by_id(query).await?
    } else {
        Vec::new()
    };
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    bail!("{}", be_more_specific(flag, query, &found, offset))
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

    if cmd.fork || cmd.continue_in.is_some() {
        let (source, resumer, query) = (source.as_ref(), resumer.as_ref(), query.trim());
        let (plan, status) = match cmd.continue_in {
            Some(into) => continue_plan(source, resumer, query, into).await?,
            None => {
                let branch = cmd.branch.as_deref();
                fork_plan(source, resumer, query, branch, &context.host_id).await?
            }
        };
        // The widget reads stderr for the command: nothing else may go there.
        if output != Output::Widget {
            eprintln!("atuin: {}", status.escape_non_printable());
        }
        return finish(direct_outcome(plan, output, settings.enter_accept), output);
    }

    let mut preselect = None;
    let target = direct_target(source.as_ref(), query.trim()).await?;
    // These only resume the session an id names: the picker would drop them.
    let flag = cmd.single_session_flag();
    if target.is_none() {
        no_single_session_for(source.as_ref(), query.trim(), flag).await?;
    }
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
        let branch = cmd.branch.as_deref();
        let (source, resumer) = (source.as_ref(), resumer.as_ref());
        let how = if cmd.switch {
            Direct::Switch
        } else if cmd.as_is {
            Direct::AsIs
        } else {
            Direct::CatchUp
        };
        let plan = direct_plan(source, resumer, &row, how, branch, &context.host_id).await?;
        match plan {
            Ok((plan, status)) => {
                // The widget reads stderr for the command: nothing else may go there.
                if let Some(status) = status
                    && output != Output::Widget
                {
                    eprintln!("atuin: {}", status.escape_non_printable());
                }
                return finish(direct_outcome(plan, output, settings.enter_accept), output);
            }
            Err(why) => preselect = Some(picker_on(row, why, flag)?),
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
        matches,
    }
    .run()
    .await?;
    if let Some(note) = note
        && output != Output::Widget
    {
        eprintln!("atuin: {}", note.escape_non_printable());
    }
    finish(outcome, output)
}

/// How `atuin ai resume <id>` resumes the session in its own agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direct {
    /// Caught up with sync first.
    CatchUp,
    /// As it is (`--as-is`).
    AsIs,
    /// Switched to another branch first (`--switch`).
    Switch,
}

/// `atuin ai resume <id>`: the plan resuming `row` in its own agent, caught up with sync first
/// (`how`: or as it is, or switched) to the head `branch` names, and what to say of it; inside,
/// why it can't resume here. A choice to make (see [`Held`]) is an error saying how to make it.
async fn direct_plan(
    source: &dyn SessionSource,
    resumer: &dyn Resumer,
    row: &SessionRow,
    how: Direct,
    branch: Option<&str>,
    here: &str,
) -> Result<Result<(ResumePlan, Option<String>), NotResumable>> {
    if how == Direct::AsIs {
        match resumer.plan(row).await {
            Ok(Resume {
                plan,
                restore: None,
                ..
            }) => return Ok(Ok((plan, None))),
            Err(why) => return Ok(Err(why)),
            // No copy here to resume as it is: it is restored.
            Ok(_) => {}
        }
    }
    let head = match branch {
        Some(selector) => Some(pick_head(source, row, selector, here).await?),
        None => None,
    };
    if how == Direct::Switch {
        return Ok(resumer.switch(source, row, head.as_ref()).await.map(|switched| {
            let status = switched.status();
            (switched.plan, Some(status))
        }));
    }
    Ok(match resumer.catch_up(source, row, head.as_ref()).await {
        Ok(CatchUp::Ready { plan, status }) => Ok((plan, status)),
        Ok(CatchUp::Choice(held)) => bail!("{}", choice_note(row, &held, here)),
        Err(why) => Err(why),
    })
}

/// The head of `row` that `selector` names (`--branch`).
async fn pick_head(
    source: &dyn SessionSource,
    row: &SessionRow,
    selector: &str,
    here: &str,
) -> Result<SourceId> {
    let analysis = source.analyse(&row.handle).await?;
    let heads = analysis.as_ref().map(Analysis::heads).unwrap_or_default();
    let now = time::OffsetDateTime::now_utc();
    match catchup::pick_branch(heads, selector, now, here) {
        Ok(head) => Ok(head.source_id.clone()),
        Err(why) => {
            // Line by line: the listing of branches keeps its lines.
            let lines: Vec<String> =
                why.lines().map(|l| l.escape_non_printable().to_string()).collect();
            bail!("--branch: {}", lines.join("\n"))
        }
    }
}

/// What to say when catching `row` up needs a choice: why, and the flags that make it. Ids are
/// agent-supplied: shown without their control characters.
fn choice_note(row: &SessionRow, held: &Held, here: &str) -> String {
    let id = quote(row.handle.session.as_ref());
    let id = id.escape_non_printable();
    let status = held.status();
    let mut note =
        format!("{}: `--as-is` resumes this copy as it is", status.escape_non_printable());
    if let Some(branch) = held.branches.get(held.chosen) {
        let pick = if held.chosen == 0 {
            String::new()
        } else {
            format!(" --branch {}", quote(&branch.selector).escape_non_printable())
        };
        note.push_str(&format!(
            ", `atuin ai resume {id} --fork{pick}` forks it from {}'s branch",
            branch.host
        ));
    }
    // Without `--branch`, `--switch` goes to the newest branch it can.
    if let Some((n, branch)) = held.branches.iter().enumerate().find(|(_, b)| b.switch) {
        let pick = if n == 0 {
            String::new()
        } else {
            format!(" --branch {}", quote(&branch.selector).escape_non_printable())
        };
        note.push_str(&format!(
            ", `atuin ai resume {id} --switch{pick}` switches this copy to {}'s branch",
            branch.host
        ));
    }
    if held.branches.len() > 1 {
        note.push_str("; `--branch` picks another:");
        let now = time::OffsetDateTime::now_utc();
        for branch in &held.branches {
            let line = catchup::describe(&branch.head, now, here);
            note.push_str(&format!("\n  {}  {line}", branch.selector.escape_non_printable()));
        }
    }
    note
}

/// `atuin ai resume <id> --in <harness>`: write the session `query` names out as a new session
/// of `into`, and the plan that resumes it, with the status line saying what was flattened.
async fn continue_plan(
    source: &dyn SessionSource,
    resumer: &dyn Resumer,
    query: &str,
    into: ContinueIn,
) -> Result<(ResumePlan, String)> {
    let Some((row, redirected)) = direct_target(source, query).await? else {
        bail!("`--in` continues the session an id names: no single session has the id {query:?}");
    };
    let continued = resumer.continue_in(source, &row, into.kind()).await.map_err(|why| {
        let message = format!("can't continue {}: {why}", row.handle.session);
        eyre::eyre!("{}", message.escape_non_printable())
    })?;
    let mut status = continued.status();
    if let Some(redirected) = redirected {
        status = format!("{redirected}; {status}");
    }
    if let Some(note) = &continued.note {
        status.push_str(&format!("; {note}"));
    }
    Ok((continued.plan, status))
}

/// `atuin ai resume <id> --fork`: write the session `query` names out as a fork of it, and the
/// plan that resumes that, with the status line saying so.
async fn fork_plan(
    source: &dyn SessionSource,
    resumer: &dyn Resumer,
    query: &str,
    branch: Option<&str>,
    here: &str,
) -> Result<(ResumePlan, String)> {
    let Some((row, redirected)) = direct_target(source, query).await? else {
        bail!("`--fork` forks the session an id names: no single session has the id {query:?}");
    };
    let head = match branch {
        Some(selector) => Some(pick_head(source, &row, selector, here).await?),
        None => None,
    };
    let from = catchup::fork_from(source, &row.handle, head.as_ref()).await;
    let forked = async { resumer.fork(source, &row, from?).await };
    let forked = forked.await.map_err(|why| {
        let message = format!("can't fork {}: {why}", row.handle.session);
        eyre::eyre!("{}", message.escape_non_printable())
    })?;
    let mut status = forked.status();
    if let Some(redirected) = redirected {
        status = format!("{redirected}; {status}");
    }
    if let Some(note) = &forked.note {
        status.push_str(&format!("; {note}"));
    }
    Ok((forked.plan, status))
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
    use crate::resume_tui::fake::{self, FakeResumer, FakeSource};

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

    /// `--branch` and `--as-is` only resume the session an id names: with an id naming several,
    /// or none, it is an error listing them, never the picker, which would drop the flag.
    #[rstest]
    #[tokio::test]
    async fn a_flag_on_no_single_session_says_to_be_more_specific() {
        use atuin_client::ai_session::HarnessKind;

        let mut claude = fake::row(HarnessKind::ClaudeCode, "7aaabc31-1631", "fix the flaky test");
        claude.updated_at = time::macros::datetime!(2026-09-27 14:03 UTC);
        let mut pi = fake::row(HarnessKind::Pi, "7aaabc31-1631", "plan the dotfiles sync");
        pi.updated_at = time::macros::datetime!(2026-09-26 09:12 UTC);
        assert_eq!(
            be_more_specific("--branch", "7aaabc31", &[claude, pi], time::UtcOffset::UTC),
            [
                "`--branch` resumes the session an id names: 7aaabc31 names 2 sessions; be more \
                 specific:",
                "  Claude Code  2026-09-27 14:03  fix the flaky test  (7aaabc31-1631)",
                "  Pi           2026-09-26 09:12  plan the dotfiles sync  (7aaabc31-1631)",
            ]
            .join("\n")
        );
        assert_eq!(
            be_more_specific("--as-is", "flaky", &[], time::UtcOffset::UTC),
            "`--as-is` resumes the session an id names: no session has the id \"flaky\""
        );

        let source = FakeSource::new();
        let ambiguous = "ses_4b8e2f1a9c3d7e";
        assert!(direct_target(&source, ambiguous).await.unwrap().is_none());
        for flag in ["--switch", "--branch", "--as-is"] {
            let err = no_single_session_for(&source, ambiguous, Some(flag)).await.unwrap_err();
            let err = err.to_string();
            assert!(err.contains("be more specific"), "{err}");
            assert!(err.contains("ses_4b8e2f1a9c3d7e6f"), "{err}");
            let err = no_single_session_for(&source, "flaky", Some(flag)).await.unwrap_err();
            assert!(err.to_string().contains("no session has the id"), "{err}");
        }
        // Without them, the picker opens on the matches.
        assert!(no_single_session_for(&source, ambiguous, None).await.is_ok());
    }

    /// A session named that can't be resumed opens the picker on it, but not with `--branch` or
    /// `--as-is`, which the picker would drop: an error saying why.
    #[rstest]
    #[case::plain(None, None)]
    #[case::branch(Some("--branch"), Some("can't resume s with `--branch`: catching it up"))]
    #[case::as_is(Some("--as-is"), Some("can't resume s with `--as-is`: catching it up"))]
    fn a_session_that_cant_resume_keeps_its_flags(
        #[case] flag: Option<&str>,
        #[case] err: Option<&str>,
    ) {
        use atuin_client::ai_session::HarnessKind;

        let row = fake::row(HarnessKind::ClaudeCode, "s", "t");
        let why = NotResumable::CatchUp("no longer a head".to_owned());
        match (picker_on(row, why, flag), err) {
            (Ok((row, _)), None) => assert_eq!(row.handle.session.as_ref(), "s"),
            (Err(e), Some(want)) => assert!(e.to_string().starts_with(want), "{e}"),
            (got, _) => panic!("{got:?}"),
        }
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

        let resumer = FakeResumer::default();
        let (plan, status) =
            continue_plan(&source, &resumer, query, ContinueIn::Codex).await.unwrap();
        assert_eq!(plan.args[1], format!("continued-{target}"));
        assert_eq!(status.contains("is a subagent"), redirected, "{status}");
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

    /// `atuin ai resume <id> --in <harness>` continues the session the id names, and plans
    /// resuming the new session; a query that names no single session is an error.
    #[rstest]
    #[tokio::test]
    async fn continuing_by_id_plans_the_new_session() {
        let source = FakeSource::new();
        let resumer = FakeResumer::default();
        let (plan, status) =
            continue_plan(&source, &resumer, "7f3c9a12", ContinueIn::Codex).await.unwrap();
        assert_eq!(plan.program, "codex");
        assert_eq!(plan.args, ["resume", "continued-7f3c9a12-5be0-4d7e-9c41-0a8e2b6f4d10"]);
        assert_eq!(status, "continuing in Codex: 42 tool calls become notes, reasoning dropped");

        let err = continue_plan(&source, &resumer, "fix flaky", ContinueIn::Pi).await.unwrap_err();
        assert!(err.to_string().contains("no single session has the id"), "{err}");

        let cli = Cli::try_parse_from(["resume", "7f3c9a12", "--in", "opencode"]).unwrap();
        assert_eq!(cli.cmd.continue_in, Some(ContinueIn::Opencode));
        assert!(Cli::try_parse_from(["resume", "7f3c9a12", "--in", "cursor"]).is_err());
    }

    /// `atuin ai resume <id> --fork` forks the session the id names and plans resuming the fork,
    /// with `--print` too; it can't go with `--in`.
    #[rstest]
    #[tokio::test]
    async fn forking_by_id_plans_the_fork() {
        let source = FakeSource::new();
        let resumer = FakeResumer::default();
        let (plan, status) =
            fork_plan(&source, &resumer, "7f3c9a12", None, fake::THIS_HOST_ID).await.unwrap();
        let fork = "forked-7f3c9a12-5be0-4d7e-9c41-0a8e2b6f4d10";
        assert!(plan.args.iter().any(|a| a == fork), "{plan:?}");
        assert_eq!(status, "forked into a new Claude Code session");

        let err =
            fork_plan(&source, &resumer, "fix flaky", None, fake::THIS_HOST_ID).await.unwrap_err();
        assert!(err.to_string().contains("no single session has the id"), "{err}");

        let cli = Cli::try_parse_from(["resume", "7f3c9a12", "--fork", "--print"]).unwrap();
        assert!(cli.cmd.fork);
        assert_eq!(cli.cmd.output(), Output::Print);
        assert!(Cli::try_parse_from(["resume", "7f3c9a12", "--fork", "--in", "pi"]).is_err());
    }

    /// The fake session `7f3c9a12…`, with sync holding two branches of it: this host's `a`-`b`-
    /// `x`-`y`, the newest, and another's `a`-`b`-`c`-`d`.
    async fn branched() -> (FakeSource, SessionRow) {
        let row = direct_match(&FakeSource::new(), "7f3c9a12").await.unwrap().unwrap();
        let rows = fake::synced_rows_of(&row.handle, true);
        (FakeSource::new().with_synced(&row.handle, rows), row)
    }

    /// `atuin ai resume <id>` catches the session's copy here up with sync first, saying what it
    /// wrote. A choice to make fails, saying why and naming the flags that make it; `--as-is`
    /// resumes the copy as it is, and `--branch` names no branch the session hasn't got.
    #[rstest]
    #[tokio::test]
    async fn resuming_by_id_catches_up_or_says_what_to_choose() {
        use crate::resume_tui::catchup::{Why, branches};

        let (source, row) = branched().await;
        let plan = FakeResumer::default().plan(&row).await.unwrap().plan;
        let status = "caught up: 2 messages from @00000002".to_owned();
        let ready = FakeResumer {
            catch_up: Some(CatchUp::Ready {
                plan: plan.clone(),
                status: Some(status.clone()),
            }),
            ..FakeResumer::default()
        };
        let here = fake::THIS_HOST_ID;
        let got = direct_plan(&source, &ready, &row, Direct::CatchUp, None, here).await.unwrap();
        assert_eq!(got, Ok((plan.clone(), Some(status))));

        let analysis = source.analyse(&row.handle).await.unwrap().unwrap();
        let copy = fake::local_tip(&["a", "b", "c", "d"], Some("d"));
        let held = FakeResumer {
            catch_up: Some(CatchUp::Choice(Box::new(Held {
                why: Why::Live,
                harness: HarnessKind::ClaudeCode,
                plan: plan.clone(),
                branches: branches(&analysis, Some(&copy), here),
                chosen: 0,
            }))),
            ..FakeResumer::default()
        };
        let err = direct_plan(&source, &held, &row, Direct::CatchUp, None, here).await.unwrap_err();
        let err = err.to_string();
        let id = row.handle.session.as_ref();
        let want = format!(
            "Claude Code is running this session here: `--as-is` resumes this copy as it is, \
             `atuin ai resume {id} --fork` forks it from this machine's branch; `--branch` picks \
             another:\n  y  this machine · "
        );
        assert!(err.starts_with(&want), "{err}");
        assert!(err.contains("\n  d  @00000002 · "), "{err}");

        let got = direct_plan(&source, &held, &row, Direct::AsIs, None, here).await.unwrap();
        assert_eq!(got, Ok((plan, None)), "as it is");
        let err = direct_plan(&source, &held, &row, Direct::CatchUp, Some("nope"), here)
            .await
            .unwrap_err();
        assert!(err.to_string().starts_with("--branch: no branch is \"nope\""), "{err}");
    }

    /// `--fork` forks a session that went on separately on several machines from its newest
    /// branch, or the one `--branch` names, from the rows on it.
    #[rstest]
    #[case::the_newest(None, &["a", "b", "x", "y"])]
    #[case::by_host(Some("@00000002"), &["a", "b", "c", "d"])]
    #[case::by_head(Some("d"), &["a", "b", "c", "d"])]
    #[tokio::test]
    async fn forking_by_id_forks_from_a_branch(
        #[case] branch: Option<&str>,
        #[case] want: &[&str],
    ) {
        let (source, _) = branched().await;
        let resumer = FakeResumer::default();
        let here = fake::THIS_HOST_ID;
        fork_plan(&source, &resumer, "7f3c9a12", branch, here).await.unwrap();
        let rows = resumer.forks.lock()[0].rows.clone().unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.source_id.as_str()).collect();
        assert_eq!(ids, want);
    }

    /// `--as-is`, `--switch` and `--branch` go with resuming (and `--branch` with `--fork` and
    /// `--switch`), not with each other or `--in`; `--print` still prints.
    #[rstest]
    #[case::switch(&["--switch", "--print"], true)]
    #[case::switch_to_a_branch(&["--switch", "--branch", "@00000002"], true)]
    #[case::switch_and_fork(&["--switch", "--fork"], false)]
    #[case::switch_and_in(&["--switch", "--in", "pi"], false)]
    #[case::switch_and_as_is(&["--switch", "--as-is"], false)]
    #[case::as_is(&["--as-is", "--print"], true)]
    #[case::branch(&["--branch", "@00000002"], true)]
    #[case::fork_from_a_branch(&["--fork", "--branch", "d", "--print"], true)]
    #[case::as_is_and_fork(&["--as-is", "--fork"], false)]
    #[case::as_is_and_branch(&["--as-is", "--branch", "d"], false)]
    #[case::branch_and_in(&["--branch", "d", "--in", "pi"], false)]
    fn catch_up_flags(#[case] flags: &[&str], #[case] ok: bool) {
        let args = ["resume", "7f3c9a12"].iter().chain(flags);
        let cli = Cli::try_parse_from(args);
        assert_eq!(cli.is_ok(), ok, "{flags:?}");
        if let Ok(cli) = cli {
            assert_eq!(cli.cmd.print, flags.contains(&"--print"));
            // Named first when the id names no single session, or the session can't resume.
            let named = ["--switch", "--branch", "--as-is"].into_iter().find(|f| flags.contains(f));
            assert_eq!(cli.cmd.single_session_flag(), named, "{flags:?}");
        }
    }

    /// `--switch` switches the copy here of the session an id names to the branch `--branch`
    /// names (else the newest it can), saying so; one that can't be switched is an error saying
    /// why. A choice catching up needs names `--switch` (with the branch it would take) where
    /// the copy can be switched.
    #[rstest]
    #[tokio::test]
    async fn switching_by_id_switches_the_copy_or_says_why_not() {
        use crate::resume_tui::catchup::{Why, branches};

        let (source, row) = branched().await;
        let here = fake::THIS_HOST_ID;
        let resumer = FakeResumer::default();
        let got = direct_plan(&source, &resumer, &row, Direct::Switch, Some("@00000002"), here)
            .await
            .unwrap()
            .unwrap();
        let said = format!(
            "switched to @00000002's branch: 4 messages (your copy is at {})",
            fake::SWITCHED_BACKUP
        );
        assert_eq!(got.1, Some(said));
        assert_eq!(*resumer.switches.lock(), [SourceId::from("d".to_owned())]);

        let fresh = FakeResumer::default();
        let refused = direct_plan(&source, &fresh, &row, Direct::Switch, None, here);
        let why = refused.await.unwrap().unwrap_err();
        assert_eq!(why, NotResumable::Switch("no head named".to_owned()));
        let err = picker_on(row.clone(), why, Some("--switch")).unwrap_err().to_string();
        assert!(err.starts_with("can't resume 7f3c9a12"), "{err}");
        assert!(err.contains("with `--switch`: switching it to another branch failed"), "{err}");

        let analysis = source.analyse(&row.handle).await.unwrap().unwrap();
        let copy = fake::local_tip(&["a", "b", "x", "y"], Some("y"));
        let mut branches = branches(&analysis, Some(&copy), here);
        branches[1].switch = true;
        let held = Held {
            why: Why::Diverged,
            harness: HarnessKind::ClaudeCode,
            plan: FakeResumer::default().plan(&row).await.unwrap().plan,
            branches,
            chosen: 0,
        };
        let note = choice_note(&row, &held, here);
        let id = row.handle.session.as_ref();
        let want = format!(
            ", `atuin ai resume {id} --switch --branch d` switches this copy to @00000002's branch"
        );
        assert!(note.contains(&want), "{note}");
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
