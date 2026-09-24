//! `atuin ai session` -- a client for the daemon's `ai.session.AiSession` service.
//!
//! Thin wrappers around [`AiClient`] plus rendering. The daemon speaks protobuf, so every subcommand
//! resolves a selector to a session, calls the matching RPC, and renders the raw messages either as
//! human-readable text or as JSON/NDJSON for scripting.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};

use atuin_client::settings::Settings;
use atuin_common::string::highlighted::{HighlightedStr, HighlightedTextProto};
use atuin_daemon::AiClient;
use atuin_daemon::grpc::ai_agent::pb as agent;
use atuin_daemon::grpc::ai_session::pb::{
    SearchSessionsMatch, get_session_event, import_sessions_event, tail_sessions_event,
};
use chrono::{DateTime, Utc};
use chrono_humanize::HumanTime;
use clap::{Args, Subcommand, ValueEnum};
use eyre::{Result, bail, eyre};
use futures::{StreamExt, TryStreamExt};
use serde::Serialize;

#[derive(Args, Debug)]
pub struct Cmd {
    #[command(subcommand)]
    cmd: SubCmd,

    /// How output is rendered.
    #[arg(long, value_enum, default_value_t = Style::Auto, global = true)]
    style: Style,
}

#[derive(Subcommand, Debug)]
enum SubCmd {
    /// List captured sessions, newest first.
    List,

    /// Show a session and its messages. Accepts a session id or `latest`.
    Show {
        /// Session id (a unique prefix is enough), or `latest` for the most recently active session.
        #[arg(value_name = "ID|latest")]
        session: String,
    },

    /// Print a session's rendered transcript. Accepts a session id or `latest`.
    Transcript {
        /// Session id (a unique prefix is enough), or `latest` for the most recently active session.
        #[arg(value_name = "ID|latest")]
        session: String,
    },

    #[command(about = "Full-text search across captured sessions, most relevant first.")]
    Search {
        #[arg(
            value_name = "QUERY",
            help = "Words to look for in session titles, message text, reasoning, and tool calls"
        )]
        query: String,
        #[arg(long, value_enum, help = "Only search sessions from this harness")]
        harness: Option<HarnessArg>,
        #[arg(
            long,
            default_value_t = 10,
            value_name = "N",
            help = "Maximum sessions to return; 0 for unbounded"
        )]
        limit: u32,
    },

    /// Follow sessions and messages as they are recorded (until interrupted).
    Tail,

    Import {
        #[arg(long, value_parser = parse_harness)]
        harness: Option<agent::HarnessKind>,
    },
}

// Only harnesses a capture path can actually produce are offered as filters (see AnyHarness);
// Copilot has no capture source yet, so advertising it would return empty for every query.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum HarnessArg {
    ClaudeCode,
    Codex,
    Opencode,
    Pi,
}

impl HarnessArg {
    fn to_pb(self) -> agent::HarnessKind {
        match self {
            Self::ClaudeCode => agent::HarnessKind::ClaudeCode,
            Self::Codex => agent::HarnessKind::Codex,
            Self::Opencode => agent::HarnessKind::Opencode,
            Self::Pi => agent::HarnessKind::Pi,
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Style {
    /// Pretty when writing to a terminal, plain otherwise.
    Auto,
    Plain,
    Pretty,
    /// A single JSON document.
    Json,
    /// Newline-delimited JSON: one object per line.
    Ndjson,
}

impl Style {
    /// Resolve `Auto` against whether stdout is a terminal.
    fn resolve(self) -> Self {
        match self {
            Self::Auto if io::stdout().is_terminal() => Self::Pretty,
            Self::Auto => Self::Plain,
            other => other,
        }
    }

    fn is_json(self) -> bool {
        matches!(self, Self::Json | Self::Ndjson)
    }
}

pub async fn run(cmd: Cmd, settings: &Settings) -> Result<()> {
    if !settings.ai.capture_sessions {
        // stderr so it never pollutes piped stdout (json/ndjson).
        eprintln!(
            "note: AI session capture is off. Enable it with `ai.capture_sessions = true` in your \
             atuin config to record new sessions."
        );
    }

    let style = cmd.style.resolve();
    let mut client = AiClient::from_settings(settings).await?;

    let result = match cmd.cmd {
        SubCmd::List => list(&mut client, style).await,
        SubCmd::Show { session } => show(&mut client, &session, style).await,
        SubCmd::Transcript { session } => transcript(&mut client, &session, style).await,
        SubCmd::Search {
            query,
            harness,
            limit,
        } => search(&mut client, &query, harness.map(HarnessArg::to_pb), limit, style).await,
        SubCmd::Tail => tail(&mut client, style).await,
        SubCmd::Import { harness } => import(&mut client, harness, style).await,
    };

    // A downstream reader that closes the pipe (e.g. `atuin ai session list | head`) makes the next
    // write fail with BrokenPipe. That's a clean end of output, not an error, so swallow it rather
    // than print a scary message and exit non-zero.
    match result {
        Err(err) if is_broken_pipe(&err) => Ok(()),
        result => result,
    }
}

/// True when `err`'s cause chain is a broken-pipe I/O error, whether raised directly by a
/// `write!`/`writeln!` or wrapped inside a `serde_json::to_writer` failure.
fn is_broken_pipe(err: &eyre::Report) -> bool {
    err.chain().any(|cause| {
        if let Some(io) = cause.downcast_ref::<io::Error>() {
            io.kind() == io::ErrorKind::BrokenPipe
        } else if let Some(json) = cause.downcast_ref::<serde_json::Error>() {
            json.io_error_kind() == Some(io::ErrorKind::BrokenPipe)
        } else {
            false
        }
    })
}

// --- subcommands --------------------------------------------------------------------------------

async fn list(client: &mut AiClient, style: Style) -> Result<()> {
    let sessions: Vec<agent::Session> = client.list_sessions(None).await?.try_collect().await?;

    let stdout = io::stdout();
    let mut out = stdout.lock();

    match style {
        Style::Json => {
            let records: Vec<SessionJson> = sessions.iter().map(session_json).collect();
            serde_json::to_writer(&mut out, &records)?;
            writeln!(out)?;
        }
        Style::Ndjson => {
            for s in &sessions {
                serde_json::to_writer(&mut out, &session_json(s))?;
                writeln!(out)?;
            }
        }
        _ => {
            if sessions.is_empty() {
                writeln!(out, "No sessions captured yet.")?;
                return Ok(());
            }
            writeln!(
                out,
                "{:<14} {:<12} {:<16} {:>5}  TITLE",
                "SESSION", "HARNESS", "UPDATED", "MSGS"
            )?;
            for s in &sessions {
                writeln!(
                    out,
                    "{:<14} {:<12} {:<16} {:>5}  {}",
                    short_id(&s.session_id),
                    harness_name(s.harness),
                    age(s.updated_at.as_ref()),
                    s.message_count,
                    one_line(title_of(s), 80),
                )?;
            }
        }
    }

    Ok(())
}

async fn show(client: &mut AiClient, selector: &str, style: Style) -> Result<()> {
    let handle = resolve(client, selector).await?;

    // Drain the stream first: the leading event is the session, the rest are messages.
    let mut stream = client.get_session(handle).await?;
    let mut session: Option<agent::Session> = None;
    let mut messages: Vec<agent::Message> = Vec::new();
    while let Some(event) = stream.next().await {
        match event?.event {
            Some(get_session_event::Event::Session(s)) => session = Some(s),
            Some(get_session_event::Event::Message(m)) => messages.push(m),
            None => {}
        }
    }
    let session = session.ok_or_else(|| eyre!("the daemon returned no session"))?;

    let stdout = io::stdout();
    let mut out = stdout.lock();

    match style {
        Style::Json => {
            let detail = SessionDetailJson {
                session: session_json(&session),
                messages: messages.iter().map(message_json).collect(),
            };
            serde_json::to_writer(&mut out, &detail)?;
            writeln!(out)?;
        }
        Style::Ndjson => {
            serde_json::to_writer(&mut out, &ShowEventJson::Session(session_json(&session)))?;
            writeln!(out)?;
            for m in &messages {
                serde_json::to_writer(&mut out, &ShowEventJson::Message(message_json(m)))?;
                writeln!(out)?;
            }
        }
        _ => {
            write_session_header(&mut out, &session)?;
            for m in &messages {
                write_message_text(&mut out, m)?;
            }
        }
    }

    Ok(())
}

async fn transcript(client: &mut AiClient, selector: &str, style: Style) -> Result<()> {
    let session = resolve(client, selector).await?;
    let handle = session.clone();

    let mut stream = client.get_transcript(handle).await?;
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        text.push_str(&chunk?.chunk);
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();

    if style.is_json() {
        let record = TranscriptJson {
            harness: harness_name(session.harness).to_owned(),
            session_id: session.session_id,
            transcript: text,
        };
        serde_json::to_writer(&mut out, &record)?;
        writeln!(out)?;
    } else {
        write!(out, "{}", sanitize(&text))?;
        if !text.ends_with('\n') {
            writeln!(out)?;
        }
    }

    Ok(())
}

async fn search(
    client: &mut AiClient,
    query: &str,
    harness: Option<agent::HarnessKind>,
    limit: u32,
    style: Style,
) -> Result<()> {
    let matches: Vec<SearchSessionsMatch> =
        client.search_sessions(query, harness, limit).await?.try_collect().await?;

    let stdout = io::stdout();
    let mut out = stdout.lock();

    match style {
        Style::Json => {
            let records =
                matches.iter().map(SearchMatchJson::from_match).collect::<Result<Vec<_>>>()?;
            serde_json::to_writer(&mut out, &records)?;
            writeln!(out)?;
        }
        Style::Ndjson => {
            for m in &matches {
                serde_json::to_writer(&mut out, &SearchMatchJson::from_match(m)?)?;
                writeln!(out)?;
            }
        }
        _ => {
            if matches.is_empty() {
                writeln!(out, "No sessions matched `{query}`.")?;
                return Ok(());
            }
            writeln!(out, "{:<14} {:<12} {:<16}  MATCH", "SESSION", "HARNESS", "UPDATED")?;
            for m in &matches {
                let session = m
                    .session
                    .as_ref()
                    .ok_or_else(|| eyre!("the daemon returned a match without a session"))?;
                let title = m
                    .title
                    .as_ref()
                    .map(HighlightedStr::try_from)
                    .transpose()?
                    .map(HighlightedStr::plain)
                    .filter(|t| !t.trim().is_empty());
                let label = match title {
                    Some(label) => label,
                    None => m
                        .preview
                        .as_ref()
                        .map(HighlightedStr::try_from)
                        .transpose()?
                        .map(HighlightedStr::plain)
                        .unwrap_or(Cow::Borrowed("")),
                };
                writeln!(
                    out,
                    "{:<14} {:<12} {:<16}  {}",
                    short_id(&session.session_id),
                    harness_name(session.harness),
                    age(session.updated_at.as_ref()),
                    one_line(label.as_ref(), 80),
                )?;
            }
        }
    }

    Ok(())
}

async fn tail(client: &mut AiClient, style: Style) -> Result<()> {
    let mut stream = client.tail_sessions(None).await?;

    // The daemon streams session-state deltas (Started/Updated) alongside messages. We fold the
    // deltas into a metadata cache used for headers but never print them: the human tail is a
    // message log grouped by session, not an echo of every state change. Each event re-locks
    // stdout and flushes so the terminal shows activity as it arrives.
    // Keyed by the full identity (harness, session_id): a native id is only unique within a
    // harness, so two harnesses can share one and must not collapse into the same header.
    let mut sessions: HashMap<(i32, String), agent::Session> = HashMap::new();
    let mut active: Option<(i32, String)> = None;

    // Color only in the pretty (terminal) view, and never when NO_COLOR is set.
    let color = matches!(style, Style::Pretty) && std::env::var_os("NO_COLOR").is_none();

    while let Some(event) = stream.next().await {
        let Some(event) = event?.event else {
            continue;
        };
        let stdout = io::stdout();
        let mut out = stdout.lock();

        if style.is_json() {
            let record = match &event {
                tail_sessions_event::Event::SessionStarted(s) => {
                    TailEventJson::SessionStarted(session_json(s))
                }
                tail_sessions_event::Event::SessionUpdated(s) => {
                    TailEventJson::SessionUpdated(session_json(s))
                }
                tail_sessions_event::Event::Message(m) => TailEventJson::Message(message_json(m)),
                tail_sessions_event::Event::Lagged(l) => {
                    TailEventJson::Lagged { dropped: l.dropped }
                }
            };
            serde_json::to_writer(&mut out, &record)?;
            writeln!(out)?;
            out.flush()?;
            continue;
        }

        match &event {
            tail_sessions_event::Event::SessionStarted(s)
            | tail_sessions_event::Event::SessionUpdated(s) => {
                sessions.insert((s.harness, s.session_id.clone()), s.clone());
            }
            tail_sessions_event::Event::Message(m) => {
                // Skip content-less records (meta/summary lines) so the tail stays legible.
                let Some(summary) = message_summary(m) else {
                    continue;
                };
                let (role_text, role_ansi) = display_role(m, &summary);
                if let Style::Plain = style {
                    writeln!(
                        out,
                        "{}  {:<12}  {:<11}  {:<9}  {}",
                        clock(m.timestamp.as_ref()),
                        short_id(&m.session_id),
                        harness_name(m.harness),
                        role_text,
                        summary.render(false),
                    )?;
                } else {
                    let key = (m.harness, m.session_id.clone());
                    if active.as_ref() != Some(&key) {
                        if active.is_some() {
                            writeln!(out)?;
                        }
                        writeln!(
                            out,
                            "{}",
                            tail_header(&m.session_id, m.harness, sessions.get(&key), color)
                        )?;
                        active = Some(key);
                    }
                    let time = paint(&clock(m.timestamp.as_ref()), Ansi::Dim, color);
                    let role = paint(&format!("{role_text:<9}"), role_ansi, color);
                    writeln!(out, "  {time}  {role}  {}", summary.render(color))?;
                }
            }
            tail_sessions_event::Event::Lagged(l) => {
                writeln!(out, "! lagged (dropped {} events)", l.dropped)?;
            }
        }

        out.flush()?;
    }

    Ok(())
}

async fn import(
    client: &mut AiClient,
    harness: Option<agent::HarnessKind>,
    style: Style,
) -> Result<()> {
    let mut stream = client.import_sessions(harness).await?;

    // `--style json` is one document, so its per-session progress and final summary are collected
    // and written once at the end; `--style ndjson` and the human styles stream event by event.
    let mut json_sessions: Vec<serde_json::Value> = Vec::new();
    let mut json_summary: Option<serde_json::Value> = None;

    while let Some(event) = stream.next().await {
        let Some(event) = event?.event else {
            continue;
        };

        if style.is_json() {
            let record = match &event {
                import_sessions_event::Event::Progress(p) => serde_json::json!({
                    "kind": "progress",
                    "harness": harness_name(p.harness),
                    "session_id": p.session_id,
                    "imported": p.imported,
                    "skipped": p.skipped,
                }),
                import_sessions_event::Event::Summary(s) => serde_json::json!({
                    "kind": "summary",
                    "sessions": s.sessions,
                    "imported": s.imported,
                    "skipped": s.skipped,
                    "failed": s.failed,
                }),
            };
            match &event {
                import_sessions_event::Event::Progress(_) if matches!(style, Style::Json) => {
                    json_sessions.push(record);
                }
                import_sessions_event::Event::Summary(_) if matches!(style, Style::Json) => {
                    json_summary = Some(record);
                }
                _ => {
                    // ndjson: one value per line.
                    let stdout = io::stdout();
                    let mut out = stdout.lock();
                    serde_json::to_writer(&mut out, &record)?;
                    writeln!(out)?;
                    out.flush()?;
                }
            }
            continue;
        }

        let stdout = io::stdout();
        let mut out = stdout.lock();
        match &event {
            import_sessions_event::Event::Progress(p) => {
                writeln!(
                    out,
                    "{:<14} {:<12} imported {:>5}  skipped {:>5}",
                    short_id(&p.session_id),
                    harness_name(p.harness),
                    p.imported,
                    p.skipped,
                )?;
            }
            import_sessions_event::Event::Summary(s) => {
                writeln!(
                    out,
                    "done: {} sessions, {} imported, {} skipped, {} failed",
                    s.sessions, s.imported, s.skipped, s.failed,
                )?;
            }
        }
        out.flush()?;
    }

    if matches!(style, Style::Json) {
        let doc = serde_json::json!({ "sessions": json_sessions, "summary": json_summary });
        let stdout = io::stdout();
        let mut out = stdout.lock();
        serde_json::to_writer(&mut out, &doc)?;
        writeln!(out)?;
    }

    Ok(())
}

// --- selector resolution ------------------------------------------------------------------------

/// Turn a `latest`/id selector into a full session handle by matching it against the session list
/// (the harness is only known from the listing, so an id alone cannot address a session).
async fn resolve(client: &mut AiClient, selector: &str) -> Result<agent::HarnessSession> {
    let mut stream = client.list_sessions(None).await?;
    // `latest` only needs the newest session, which the daemon streams first, so take a single
    // item instead of draining the whole stream. Any id/prefix selector needs the full list to
    // match and disambiguate.
    let sessions: Vec<agent::Session> = if selector.eq_ignore_ascii_case("latest") {
        stream.try_next().await?.into_iter().collect()
    } else {
        stream.try_collect().await?
    };
    select_session(sessions, selector)
}

/// Pure selector logic, split out from the RPC so it can be tested directly.
fn select_session(sessions: Vec<agent::Session>, selector: &str) -> Result<agent::HarnessSession> {
    if selector.eq_ignore_ascii_case("latest") {
        // The daemon lists newest-first, so the first entry is the most recent.
        let latest =
            sessions.into_iter().next().ok_or_else(|| eyre!("no sessions captured yet"))?;
        return Ok(handle_of(&latest));
    }

    // `list` prints ids truncated to 12 chars, so accept a unique id prefix as well as a full id.
    let mut matches = sessions.into_iter().filter(|s| s.session_id.starts_with(selector));
    let first = matches
        .next()
        .ok_or_else(|| eyre!("no session with id `{selector}`. Run `atuin ai session list`."))?;
    if matches.next().is_some() {
        bail!("id `{selector}` matches more than one session; use a longer or full id");
    }
    Ok(handle_of(&first))
}

fn handle_of(session: &agent::Session) -> agent::HarnessSession {
    agent::HarnessSession {
        harness: session.harness,
        session_id: session.session_id.clone(),
    }
}

// --- human rendering ----------------------------------------------------------------------------

fn write_session_header(out: &mut dyn Write, s: &agent::Session) -> io::Result<()> {
    writeln!(out, "session   {}", sanitize(&s.session_id))?;
    writeln!(out, "harness   {}", harness_name(s.harness))?;
    if let Some(title) = &s.title {
        writeln!(out, "title     {}", sanitize(title))?;
    }
    if let Some(cwd) = &s.cwd {
        writeln!(out, "cwd       {}", sanitize(cwd))?;
    }
    if let Some(branch) = &s.git_branch {
        writeln!(out, "branch    {}", sanitize(branch))?;
    }
    if let Some(model) = &s.model {
        writeln!(out, "model     {}", sanitize(model))?;
    }
    writeln!(out, "started   {}", age(s.started_at.as_ref()))?;
    writeln!(out, "updated   {}", age(s.updated_at.as_ref()))?;
    writeln!(out, "messages  {}", s.message_count)?;
    if let Some(t) = &s.tokens {
        writeln!(
            out,
            "tokens    in {} / out {} / cache {}+{}",
            t.input, t.output, t.cache_read, t.cache_write
        )?;
    }
    writeln!(out)
}

fn write_message_text(out: &mut dyn Write, m: &agent::Message) -> io::Result<()> {
    writeln!(out, "── {} · {} ──", role_name(m.role), age(m.timestamp.as_ref()))?;
    for block in &m.content {
        match &block.block {
            Some(agent::content_block::Block::Text(t)) => writeln!(out, "{}", sanitize(t))?,
            Some(agent::content_block::Block::Thinking(t)) => {
                writeln!(out, "[thinking] {}", sanitize(t))?;
            }
            // An empty input or content means capture did not keep it; print the tag alone.
            Some(agent::content_block::Block::ToolCall(tc)) => {
                write!(out, "[tool-call {}]", sanitize(&tc.name))?;
                if !tc.input.is_empty() {
                    write!(out, " {}", sanitize(&tc.input))?;
                }
                writeln!(out)?;
            }
            Some(agent::content_block::Block::ToolResult(tr)) => {
                let tag = if tr.is_error {
                    "tool-error"
                } else {
                    "tool-result"
                };
                write!(out, "[{tag}]")?;
                if !tr.content.is_empty() {
                    write!(out, " {}", sanitize(&tr.content))?;
                }
                writeln!(out)?;
            }
            None => {}
        }
    }
    writeln!(out)
}

fn short_id(session_id: &str) -> &str {
    session_id.get(..12).unwrap_or(session_id)
}

fn title_of(s: &agent::Session) -> &str {
    s.title.as_deref().or(s.preview.as_deref()).unwrap_or("")
}

const SUMMARY_WIDTH: usize = 80;

/// The renderable essence of a message line, kept color-free so `message_summary` stays pure and
/// testable; color is applied only in [`Summary::render`].
enum Summary {
    /// Prose: assistant/user/system text.
    Text(String),
    /// Assistant reasoning / thinking.
    Thinking(String),
    /// A tool invocation, by name.
    ToolCall(String),
    /// A tool result and whether it errored.
    ToolResult {
        is_error: bool,
        body: String,
    },
}

impl Summary {
    /// The display string, with ANSI color when `color` is set.
    fn render(&self, color: bool) -> String {
        match self {
            Self::Text(t) => t.clone(),
            Self::Thinking(t) => format!("{} {t}", paint("»", Ansi::Dim, color)),
            Self::ToolCall(name) => format!("{} {name}", paint("⚙", Ansi::Blue, color)),
            Self::ToolResult { is_error, body } => {
                let mark = if *is_error {
                    paint("✗", Ansi::Red, color)
                } else {
                    paint("✓", Ansi::Green, color)
                };
                if body.is_empty() {
                    mark
                } else {
                    format!("{mark} {body}")
                }
            }
        }
    }
}

/// A one-line summary of a message for the `tail` log, or `None` when the message carries nothing
/// worth a line (meta/summary records with no renderable content) so the caller can skip it.
fn message_summary(m: &agent::Message) -> Option<Summary> {
    for block in &m.content {
        match &block.block {
            Some(agent::content_block::Block::Text(t)) => {
                let line = one_line(t, SUMMARY_WIDTH);
                if !line.is_empty() {
                    return Some(Summary::Text(line));
                }
            }
            Some(agent::content_block::Block::Thinking(t)) => {
                let line = one_line(t, SUMMARY_WIDTH);
                if !line.is_empty() {
                    return Some(Summary::Thinking(line));
                }
            }
            Some(agent::content_block::Block::ToolCall(tc)) => {
                // Fold like the sibling arms: the tool name is captured content and must not carry
                // control chars into the `tail` view.
                return Some(Summary::ToolCall(one_line(&tc.name, SUMMARY_WIDTH)));
            }
            Some(agent::content_block::Block::ToolResult(tr)) => {
                return Some(Summary::ToolResult {
                    is_error: tr.is_error,
                    body: one_line(&tr.content, SUMMARY_WIDTH),
                });
            }
            None => {}
        }
    }
    None
}

/// The role label to display: the harness's own string when the enum cannot name it (e.g. codex
/// `developer`), otherwise the standard role name.
fn message_role(m: &agent::Message) -> String {
    // role_label is free-form text captured from the harness, so strip any control chars before it
    // reaches the `tail` view; the enum fallback (role_name) is already a fixed string.
    m.role_label
        .as_deref()
        .filter(|s| !s.is_empty())
        .map_or_else(|| role_name(m.role).to_owned(), |s| sanitize(s).into_owned())
}

/// The role text and its color for a rendered `tail` line. A tool result is labelled `tool`
/// whatever the envelope role, since some harnesses model tool output as a user turn.
fn display_role(m: &agent::Message, summary: &Summary) -> (String, Ansi) {
    if matches!(summary, Summary::ToolResult { .. }) {
        ("tool".to_owned(), role_color(agent::Role::Tool as i32))
    } else {
        (message_role(m), role_color(m.role))
    }
}

/// The session-group header line printed the first time a session appears in the pretty `tail`
/// view and whenever the active session changes.
fn tail_header(
    session_id: &str,
    harness: i32,
    session: Option<&agent::Session>,
    color: bool,
) -> String {
    let bullet = paint("●", harness_color(harness), color);
    let id = paint(short_id(session_id), Ansi::Bold, color);
    let harness_label = paint(harness_name(harness), Ansi::Dim, color);
    let title = session.map(title_of).map(|t| one_line(t, SUMMARY_WIDTH)).unwrap_or_default();
    if title.is_empty() {
        format!("{bullet} {id} · {harness_label}")
    } else {
        format!("{bullet} {id} · {harness_label} · {title}")
    }
}

/// Local wall-clock `HH:MM:SS` for a protobuf timestamp, for live `tail` lines.
fn clock(ts: Option<&prost_types::Timestamp>) -> String {
    to_datetime(ts)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".to_owned())
}

// --- color --------------------------------------------------------------------------------------

/// A minimal ANSI palette. `tail` uses raw codes rather than a dependency, applies them only in the
/// pretty (terminal) view, and suppresses them when `NO_COLOR` is set.
#[derive(Clone, Copy)]
enum Ansi {
    Dim,
    Bold,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
}

impl Ansi {
    fn code(self) -> &'static str {
        match self {
            Self::Dim => "2",
            Self::Bold => "1",
            Self::Red => "31",
            Self::Green => "32",
            Self::Yellow => "33",
            Self::Blue => "34",
            Self::Magenta => "35",
            Self::Cyan => "36",
        }
    }
}

/// Wrap `text` in an ANSI style when `color` is set, otherwise return it unchanged.
fn paint(text: &str, style: Ansi, color: bool) -> String {
    if color {
        format!("\x1b[{}m{text}\x1b[0m", style.code())
    } else {
        text.to_owned()
    }
}

fn role_color(role: i32) -> Ansi {
    match agent::Role::try_from(role) {
        Ok(agent::Role::Assistant) => Ansi::Cyan,
        Ok(agent::Role::User) => Ansi::Yellow,
        Ok(agent::Role::System) => Ansi::Magenta,
        Ok(agent::Role::Tool) => Ansi::Blue,
        Ok(agent::Role::Unknown) | Err(_) => Ansi::Dim,
    }
}

fn harness_color(harness: i32) -> Ansi {
    match agent::HarnessKind::try_from(harness) {
        Ok(agent::HarnessKind::ClaudeCode) => Ansi::Magenta,
        Ok(agent::HarnessKind::Codex) => Ansi::Green,
        Ok(agent::HarnessKind::Copilot) => Ansi::Blue,
        Ok(agent::HarnessKind::Opencode) => Ansi::Cyan,
        Ok(agent::HarnessKind::Pi) => Ansi::Yellow,
        Ok(agent::HarnessKind::Unknown) | Err(_) => Ansi::Dim,
    }
}

/// Collapse all whitespace (including embedded newlines) to single spaces and truncate to `max`
/// characters with an ellipsis. Session titles/previews are captured from multi-line prompts, so the
/// human table and `tail` views must flatten them or a single entry spills across many rows.
fn one_line(text: &str, max: usize) -> String {
    // Whitespace-fold, then drop any control chars left over (ESC, BEL, C1): captured session
    // content is untrusted and must not emit terminal escape sequences into the compact views.
    let collapsed: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    if collapsed.chars().count() > max {
        format!("{}…", collapsed.chars().take(max).collect::<String>())
    } else {
        collapsed
    }
}

/// Replace terminal control characters in captured session content with spaces, so a recorded
/// session can never emit escape sequences (colour, cursor moves, a window-title change) when it is
/// printed to a TTY. `\n`/`\t` are kept so multi-line bodies still lay out; the compact
/// `list`/`search`/`tail` views fold whitespace through [`one_line`] instead.
fn sanitize(text: &str) -> Cow<'_, str> {
    let is_escape = |c: char| c.is_control() && c != '\n' && c != '\t';
    if text.contains(is_escape) {
        Cow::Owned(
            text.chars()
                .map(|c| {
                    if is_escape(c) {
                        ' '
                    } else {
                        c
                    }
                })
                .collect(),
        )
    } else {
        Cow::Borrowed(text)
    }
}

/// A humanized age (e.g. "2 hours ago") for a protobuf timestamp, or "-" when absent.
fn age(ts: Option<&prost_types::Timestamp>) -> String {
    to_datetime(ts).map_or_else(|| "-".to_owned(), |dt| HumanTime::from(dt).to_string())
}

fn to_datetime(ts: Option<&prost_types::Timestamp>) -> Option<DateTime<Utc>> {
    let ts = ts?;
    DateTime::from_timestamp(ts.seconds, u32::try_from(ts.nanos).unwrap_or(0))
}

fn rfc3339(ts: Option<&prost_types::Timestamp>) -> Option<String> {
    to_datetime(ts).map(|dt| dt.to_rfc3339())
}

fn parse_harness(value: &str) -> Result<agent::HarnessKind, String> {
    match value {
        "claude-code" => Ok(agent::HarnessKind::ClaudeCode),
        "codex" => Ok(agent::HarnessKind::Codex),
        "pi" => Ok(agent::HarnessKind::Pi),
        other => Err(format!("unknown harness `{other}` (expected claude-code, codex, or pi)")),
    }
}

/// The kebab display label for a harness discriminant. Kept exhaustive over every `HarnessKind`
/// (including ones no capture path yet produces) so a stored value always renders. Shared with the
/// MCP session-search renderer.
pub fn harness_name(harness: i32) -> &'static str {
    match agent::HarnessKind::try_from(harness) {
        Ok(agent::HarnessKind::ClaudeCode) => "claude-code",
        Ok(agent::HarnessKind::Codex) => "codex",
        Ok(agent::HarnessKind::Copilot) => "copilot",
        Ok(agent::HarnessKind::Opencode) => "opencode",
        Ok(agent::HarnessKind::Pi) => "pi",
        Ok(agent::HarnessKind::Unknown) | Err(_) => "unknown",
    }
}

fn role_name(role: i32) -> &'static str {
    match agent::Role::try_from(role) {
        Ok(agent::Role::User) => "user",
        Ok(agent::Role::Assistant) => "assistant",
        Ok(agent::Role::System) => "system",
        Ok(agent::Role::Tool) => "tool",
        Ok(agent::Role::Unknown) | Err(_) => "unknown",
    }
}

fn stop_reason_name(stop_reason: i32) -> &'static str {
    match agent::StopReason::try_from(stop_reason) {
        Ok(agent::StopReason::EndTurn) => "end_turn",
        Ok(agent::StopReason::ToolUse) => "tool_use",
        Ok(agent::StopReason::MaxTokens) => "max_tokens",
        Ok(agent::StopReason::Aborted) => "aborted",
        Ok(agent::StopReason::Error) => "error",
        Ok(agent::StopReason::Unknown) | Err(_) => "unknown",
    }
}

// --- JSON view structs --------------------------------------------------------------------------
//
// The protobuf types are not serde-serializable (and leak enum ints, raw uuid bytes and prost
// timestamps), so JSON output goes through these owned views with a stable, documented shape.

#[derive(Serialize)]
struct TokensJson {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

#[derive(Serialize)]
struct HandleJson {
    harness: String,
    session_id: String,
}

#[derive(Serialize)]
struct SessionJson {
    harness: String,
    session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<HandleJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
    message_count: u64,
    tokens: TokensJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentJson {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Serialize)]
struct MessageJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git_branch: Option<String>,
    content: Vec<ContentJson>,
    tokens: TokensJson,
    stop_reason: String,
}

#[derive(Serialize)]
struct SessionDetailJson {
    session: SessionJson,
    messages: Vec<MessageJson>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ShowEventJson {
    Session(SessionJson),
    Message(MessageJson),
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TailEventJson {
    SessionStarted(SessionJson),
    SessionUpdated(SessionJson),
    Message(MessageJson),
    Lagged {
        dropped: u64,
    },
}

#[derive(Serialize)]
struct TranscriptJson {
    harness: String,
    session_id: String,
    transcript: String,
}

#[derive(Serialize)]
struct HighlightJson {
    text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    matches: Vec<[usize; 2]>,
}

#[derive(Serialize)]
struct SearchMatchJson {
    session: SessionJson,
    score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<HighlightJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<HighlightJson>,
}

impl HighlightJson {
    fn from_proto(proto: &HighlightedTextProto) -> Result<Self> {
        let highlighted = HighlightedStr::try_from(proto)?;
        let plain = highlighted.to_plain();
        Ok(Self {
            text: plain.text.into_owned(),
            matches: plain.ranges.iter().map(|r| [r.start, r.end]).collect(),
        })
    }
}

impl SearchMatchJson {
    fn from_match(m: &SearchSessionsMatch) -> Result<Self> {
        let session = m
            .session
            .as_ref()
            .ok_or_else(|| eyre!("the daemon returned a match without a session"))?;
        Ok(Self {
            session: session_json(session),
            score: m.score,
            title: m.title.as_ref().map(HighlightJson::from_proto).transpose()?,
            preview: m.preview.as_ref().map(HighlightJson::from_proto).transpose()?,
        })
    }
}

fn tokens_json(tokens: Option<&agent::Tokens>) -> TokensJson {
    tokens.map_or(
        TokensJson {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
        },
        |t| TokensJson {
            input: t.input,
            output: t.output,
            cache_read: t.cache_read,
            cache_write: t.cache_write,
        },
    )
}

fn handle_json(handle: &agent::HarnessSession) -> HandleJson {
    HandleJson {
        harness: harness_name(handle.harness).to_owned(),
        session_id: handle.session_id.clone(),
    }
}

fn session_json(s: &agent::Session) -> SessionJson {
    SessionJson {
        harness: harness_name(s.harness).to_owned(),
        session_id: s.session_id.clone(),
        parent: s.parent.as_ref().map(handle_json),
        cwd: s.cwd.clone(),
        git_branch: s.git_branch.clone(),
        model: s.model.clone(),
        started_at: rfc3339(s.started_at.as_ref()),
        updated_at: rfc3339(s.updated_at.as_ref()),
        message_count: s.message_count,
        tokens: tokens_json(s.tokens.as_ref()),
        title: s.title.clone(),
        preview: s.preview.clone(),
    }
}

fn content_json(block: &agent::ContentBlock) -> Option<ContentJson> {
    Some(match block.block.as_ref()? {
        agent::content_block::Block::Text(t) => ContentJson::Text { text: t.clone() },
        agent::content_block::Block::Thinking(t) => ContentJson::Thinking { text: t.clone() },
        agent::content_block::Block::ToolCall(tc) => ContentJson::ToolCall {
            id: tc.id.clone(),
            name: tc.name.clone(),
            input: tc.input.clone(),
        },
        agent::content_block::Block::ToolResult(tr) => ContentJson::ToolResult {
            tool_use_id: tr.tool_use_id.clone(),
            content: tr.content.clone(),
            is_error: tr.is_error,
        },
    })
}

fn message_json(m: &agent::Message) -> MessageJson {
    let id =
        m.id.as_ref().and_then(|u| uuid::Uuid::from_slice(&u.value).ok()).map(|u| u.to_string());
    MessageJson {
        id,
        role: message_role(m),
        timestamp: rfc3339(m.timestamp.as_ref()),
        model: m.model.clone(),
        cwd: m.cwd.clone(),
        git_branch: m.git_branch.clone(),
        content: m.content.iter().filter_map(content_json).collect(),
        tokens: tokens_json(m.tokens.as_ref()),
        stop_reason: stop_reason_name(m.stop_reason).to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn session(harness: agent::HarnessKind, id: &str) -> agent::Session {
        agent::Session {
            harness: harness as i32,
            session_id: id.to_owned(),
            parent: None,
            cwd: None,
            git_branch: None,
            model: None,
            started_at: None,
            updated_at: None,
            message_count: 0,
            tokens: None,
            title: None,
            preview: None,
        }
    }

    #[rstest]
    #[case("claude-code", agent::HarnessKind::ClaudeCode)]
    #[case("codex", agent::HarnessKind::Codex)]
    #[case("pi", agent::HarnessKind::Pi)]
    fn parse_harness_maps_names(#[case] input: &str, #[case] want: agent::HarnessKind) {
        assert_eq!(parse_harness(input).unwrap(), want);
    }

    #[rstest]
    fn parse_harness_rejects_unknown_harnesses() {
        assert!(parse_harness("opencode").is_err());
    }

    #[rstest]
    fn latest_picks_the_first_listed() {
        let sessions = vec![
            session(agent::HarnessKind::Codex, "newest"),
            session(agent::HarnessKind::Pi, "old"),
        ];
        assert_eq!(select_session(sessions, "latest").unwrap().session_id, "newest");
    }

    #[rstest]
    fn latest_on_empty_is_an_error() {
        assert!(select_session(Vec::new(), "latest").is_err());
    }

    #[rstest]
    fn latest_resolves_from_a_single_session() {
        // `resolve` now hands `select_session` just the newest session for `latest`, so a
        // one-element list must still resolve.
        let handle =
            select_session(vec![session(agent::HarnessKind::ClaudeCode, "only")], "latest")
                .unwrap();
        assert_eq!(handle.session_id, "only");
        assert_eq!(handle.harness, agent::HarnessKind::ClaudeCode as i32);
    }

    #[rstest]
    fn exact_id_resolves_the_harness_from_the_listing() {
        let sessions = vec![
            session(agent::HarnessKind::Codex, "aaa"),
            session(agent::HarnessKind::ClaudeCode, "bbb"),
        ];
        let handle = select_session(sessions, "bbb").unwrap();
        assert_eq!(handle.session_id, "bbb");
        assert_eq!(handle.harness, agent::HarnessKind::ClaudeCode as i32);
    }

    #[rstest]
    fn id_prefix_resolves_a_session() {
        // `list` prints ids truncated, so a copied prefix must resolve.
        let sessions = vec![
            session(agent::HarnessKind::Codex, "abcdef0123456789"),
            session(agent::HarnessKind::ClaudeCode, "fedcba9876543210"),
        ];
        let handle = select_session(sessions, "abcdef012345").unwrap();
        assert_eq!(handle.session_id, "abcdef0123456789");
        assert_eq!(handle.harness, agent::HarnessKind::Codex as i32);
    }

    #[rstest]
    fn ambiguous_prefix_is_an_error() {
        let sessions = vec![
            session(agent::HarnessKind::Codex, "abc111"),
            session(agent::HarnessKind::ClaudeCode, "abc222"),
        ];
        assert!(select_session(sessions, "abc").is_err());
    }

    #[rstest]
    fn unknown_id_is_an_error() {
        let sessions = vec![session(agent::HarnessKind::Codex, "aaa")];
        assert!(select_session(sessions, "zzz").is_err());
    }

    #[rstest]
    fn ambiguous_id_across_harnesses_is_an_error() {
        let sessions = vec![
            session(agent::HarnessKind::Codex, "dup"),
            session(agent::HarnessKind::ClaudeCode, "dup"),
        ];
        assert!(select_session(sessions, "dup").is_err());
    }

    #[rstest]
    fn session_json_has_a_stable_shape() {
        let mut s = session(agent::HarnessKind::ClaudeCode, "abcdef0123456789");
        s.message_count = 3;
        s.tokens = Some(agent::Tokens {
            input: 10,
            output: 20,
            cache_read: 1,
            cache_write: 2,
        });
        s.title = Some("hello".to_owned());

        let v = serde_json::to_value(session_json(&s)).unwrap();
        assert_eq!(v["harness"], "claude-code");
        assert_eq!(v["session_id"], "abcdef0123456789");
        assert_eq!(v["message_count"], 3);
        assert_eq!(v["tokens"]["input"], 10);
        assert_eq!(v["tokens"]["output"], 20);
        assert_eq!(v["title"], "hello");
        // Absent optionals are omitted rather than serialized as null.
        assert!(v.get("cwd").is_none());
    }

    #[rstest]
    #[case(agent::HarnessKind::ClaudeCode as i32, "claude-code")]
    #[case(agent::HarnessKind::Pi as i32, "pi")]
    #[case(999, "unknown")]
    fn harness_name_maps_known_and_unknown(#[case] raw: i32, #[case] expected: &str) {
        assert_eq!(harness_name(raw), expected);
    }

    fn msg(role: agent::Role, blocks: Vec<agent::content_block::Block>) -> agent::Message {
        agent::Message {
            role: role as i32,
            content: blocks.into_iter().map(|b| agent::ContentBlock { block: Some(b) }).collect(),
            ..Default::default()
        }
    }

    #[rstest]
    fn summary_prefers_text() {
        let m = msg(agent::Role::Assistant, vec![agent::content_block::Block::Text(
            "hello world".to_owned(),
        )]);
        assert_eq!(message_summary(&m).unwrap().render(false), "hello world");
    }

    #[rstest]
    fn summary_labels_a_tool_call() {
        let m = msg(agent::Role::Assistant, vec![agent::content_block::Block::ToolCall(
            agent::ToolCall {
                name: "Edit".to_owned(),
                ..Default::default()
            },
        )]);
        assert_eq!(message_summary(&m).unwrap().render(false), "⚙ Edit");
    }

    #[rstest]
    fn tail_render_strips_control_chars_from_tool_name_and_role_label() {
        // The tail view prints the captured tool-call name and free-form role label directly; a
        // recorded session must not smuggle terminal escapes through either sink.
        let mut m = msg(agent::Role::Assistant, vec![agent::content_block::Block::ToolCall(
            agent::ToolCall {
                name: "run\x1b]0;pwned\x07 now".to_owned(),
                ..Default::default()
            },
        )]);
        m.role_label = Some("dev\x1b[31mil".to_owned());

        let summary = message_summary(&m).expect("a tool call yields a summary");
        // render(false) omits our own colour codes, so any control char left is from the payload.
        assert!(
            !summary.render(false).chars().any(char::is_control),
            "the tool-call name must be folded before the tail view prints it"
        );
        let (role_text, _) = display_role(&m, &summary);
        assert!(!role_text.chars().any(char::is_control), "the role label must be sanitized");
    }

    #[rstest]
    #[case(false, "✓ ok")]
    #[case(true, "✗ boom")]
    fn summary_marks_a_tool_result(#[case] is_error: bool, #[case] expected: &str) {
        let content = if is_error {
            "boom"
        } else {
            "ok"
        };
        let m = msg(agent::Role::Tool, vec![agent::content_block::Block::ToolResult(
            agent::ToolResult {
                content: content.to_owned(),
                is_error,
                ..Default::default()
            },
        )]);
        assert_eq!(message_summary(&m).unwrap().render(false), expected);
    }

    #[rstest]
    fn summary_skips_content_less_messages() {
        assert!(message_summary(&msg(agent::Role::User, vec![])).is_none());
        let blank =
            msg(agent::Role::User, vec![agent::content_block::Block::Text("   ".to_owned())]);
        assert!(message_summary(&blank).is_none());
    }

    #[rstest]
    fn summary_marks_thinking() {
        let m = msg(agent::Role::Assistant, vec![agent::content_block::Block::Thinking(
            "pondering".to_owned(),
        )]);
        assert_eq!(message_summary(&m).unwrap().render(false), "» pondering");
    }

    #[rstest]
    fn role_label_overrides_the_enum() {
        let mut m = msg(agent::Role::Unknown, vec![]);
        m.role_label = Some("developer".to_owned());
        assert_eq!(message_role(&m), "developer");
        // A standard role with no label falls back to the enum name.
        assert_eq!(message_role(&msg(agent::Role::User, vec![])), "user");
    }

    #[rstest]
    fn tool_result_line_is_labelled_tool() {
        // Claude models a tool result as a user-turn message; the line should still say "tool".
        let m = msg(agent::Role::User, vec![agent::content_block::Block::ToolResult(
            agent::ToolResult {
                content: "ok".to_owned(),
                is_error: false,
                ..Default::default()
            },
        )]);
        let summary = message_summary(&m).unwrap();
        assert_eq!(display_role(&m, &summary).0, "tool");
    }

    #[rstest]
    fn summary_falls_through_empty_text_to_the_next_block() {
        let m = msg(agent::Role::Assistant, vec![
            agent::content_block::Block::Text(String::new()),
            agent::content_block::Block::ToolCall(agent::ToolCall {
                name: "Bash".to_owned(),
                ..Default::default()
            }),
        ]);
        assert_eq!(message_summary(&m).unwrap().render(false), "⚙ Bash");
    }

    #[rstest]
    fn header_includes_title_when_present() {
        let mut s = session(agent::HarnessKind::ClaudeCode, "abcdef0123456789");
        s.title = Some("My Session".to_owned());
        assert_eq!(
            tail_header("abcdef0123456789", agent::HarnessKind::ClaudeCode as i32, Some(&s), false),
            "● abcdef012345 · claude-code · My Session"
        );
    }

    #[rstest]
    fn header_omits_title_when_absent() {
        assert_eq!(
            tail_header("abcdef0123456789", agent::HarnessKind::ClaudeCode as i32, None, false),
            "● abcdef012345 · claude-code"
        );
    }

    #[rstest]
    fn color_wraps_only_when_enabled() {
        assert_eq!(paint("x", Ansi::Dim, false), "x");
        assert_eq!(paint("x", Ansi::Dim, true), "\u{1b}[2mx\u{1b}[0m");

        let err = Summary::ToolResult {
            is_error: true,
            body: "boom".to_owned(),
        };
        assert_eq!(err.render(false), "✗ boom");
        let painted = err.render(true);
        assert!(painted.contains("\u{1b}[31m"), "error mark should be red");
        assert!(painted.ends_with("boom"), "body stays uncolored");
    }

    #[rstest]
    #[case(HarnessArg::ClaudeCode, agent::HarnessKind::ClaudeCode)]
    #[case(HarnessArg::Codex, agent::HarnessKind::Codex)]
    #[case(HarnessArg::Opencode, agent::HarnessKind::Opencode)]
    #[case(HarnessArg::Pi, agent::HarnessKind::Pi)]
    fn harness_arg_maps_to_pb(#[case] arg: HarnessArg, #[case] expected: agent::HarnessKind) {
        assert_eq!(arg.to_pb(), expected);
    }

    #[rstest]
    fn sanitize_neutralizes_terminal_escapes_in_captured_content() {
        // A recorded session could carry a clear-screen + window-title-spoof sequence.
        let hostile = "hi\x1b[2J\x1b]0;pwned\x07 there";
        let safe = sanitize(hostile);
        assert!(!safe.chars().any(char::is_control), "no control chars may reach the terminal");
        assert!(safe.contains("hi") && safe.contains("there"), "printable text is preserved");
    }

    #[rstest]
    fn sanitize_keeps_newlines_and_tabs_for_multiline_bodies() {
        assert_eq!(sanitize("a\n\tb"), "a\n\tb");
        assert!(matches!(sanitize("plain"), Cow::Borrowed(_)), "clean text is not reallocated");
    }

    #[rstest]
    fn one_line_drops_control_characters() {
        assert!(!one_line("a\x1b[31mred\x07", 80).chars().any(char::is_control));
    }

    #[rstest]
    fn search_match_json_carries_plain_text_and_match_ranges() {
        let m = SearchSessionsMatch {
            session: Some(session(agent::HarnessKind::ClaudeCode, "abc")),
            title: Some(HighlightedTextProto {
                open: 0xE000,
                close: 0xE001,
                raw: "the \u{E000}build\u{E001}".to_owned(),
            }),
            preview: None,
            score: 2.5,
        };

        let v = serde_json::to_value(SearchMatchJson::from_match(&m).unwrap()).unwrap();
        assert_eq!(v["session"]["session_id"], "abc");
        assert_eq!(v["score"], 2.5);
        assert_eq!(v["title"]["text"], "the build");
        assert_eq!(v["title"]["matches"], serde_json::json!([[4, 9]]));
        assert!(v.get("preview").is_none(), "an absent preview is omitted, not null");
    }

    #[rstest]
    fn search_match_json_requires_a_session() {
        let m = SearchSessionsMatch {
            session: None,
            title: None,
            preview: None,
            score: 0.0,
        };
        assert!(SearchMatchJson::from_match(&m).is_err());
    }
}
