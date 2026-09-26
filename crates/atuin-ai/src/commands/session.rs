//! `atuin ai session` -- a client for the daemon's `ai.session.AiSession` service.
//!
//! Thin wrappers around [`AiClient`] plus rendering. Every subcommand resolves a selector to a
//! session, calls the matching RPC, decodes what the daemon streams into the domain types, and
//! renders them either as human-readable text or as JSON/NDJSON for scripting.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};

use atuin_client::ai_session::{HarnessKind, HarnessSession, Message, Session, SessionMatch};
use atuin_client::settings::Settings;
use atuin_common::harnesstools::session::model::reasoning_label;
use atuin_common::harnesstools::session::{Content, Role, StopReason, Usage};
use atuin_common::string::highlighted::HighlightedString;
use atuin_daemon::AiClient;
use atuin_daemon::grpc::ai::session::pb::{
    get_session_event, import_sessions_event, tail_sessions_event,
};
use chrono::{DateTime, Utc};
use chrono_humanize::HumanTime;
use clap::{Args, Subcommand, ValueEnum};
use eyre::{Result, bail, eyre};
use futures::{StreamExt, TryStreamExt};
use serde::Serialize;
use time::OffsetDateTime;

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
        harness: Option<HarnessKind>,
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
    fn to_pb(self) -> HarnessKind {
        match self {
            Self::ClaudeCode => HarnessKind::ClaudeCode,
            Self::Codex => HarnessKind::Codex,
            Self::Opencode => HarnessKind::Opencode,
            Self::Pi => HarnessKind::Pi,
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
    let sessions: Vec<Session> = client
        .list_sessions(None)
        .await?
        .map(|session| Ok::<_, eyre::Report>(Session::try_from(session?)?))
        .try_collect()
        .await?;

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
                    short_id(s.handle.session.as_ref()),
                    harness_name(s.handle.harness),
                    age(s.updated_at),
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
    let mut session: Option<Session> = None;
    let mut messages: Vec<Message> = Vec::new();
    while let Some(event) = stream.next().await {
        match event?.event {
            Some(get_session_event::Event::Session(s)) => session = Some(s.try_into()?),
            Some(get_session_event::Event::Message(m)) => messages.push(m.try_into()?),
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

    let mut stream = client.get_transcript(session.clone()).await?;
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        text.push_str(&chunk?.chunk);
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();

    if style.is_json() {
        let record = TranscriptJson {
            harness: harness_name(session.harness).to_owned(),
            session_id: session.session.into(),
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
    harness: Option<HarnessKind>,
    limit: u32,
    style: Style,
) -> Result<()> {
    let matches: Vec<SessionMatch> = client
        .search_sessions(query, harness, limit)
        .await?
        .map(|m| Ok::<_, eyre::Report>(SessionMatch::try_from(m?)?))
        .try_collect()
        .await?;

    let stdout = io::stdout();
    let mut out = stdout.lock();

    match style {
        Style::Json => {
            let records: Vec<SearchMatchJson> = matches.iter().map(SearchMatchJson::from).collect();
            serde_json::to_writer(&mut out, &records)?;
            writeln!(out)?;
        }
        Style::Ndjson => {
            for m in &matches {
                serde_json::to_writer(&mut out, &SearchMatchJson::from(m))?;
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
                let title = m.title.to_plain().text;
                let label = if title.trim().is_empty() {
                    m.preview.to_plain().text
                } else {
                    title
                };
                writeln!(
                    out,
                    "{:<14} {:<12} {:<16}  {}",
                    short_id(m.session.handle.session.as_ref()),
                    harness_name(m.session.handle.harness),
                    age(m.session.updated_at),
                    one_line(&label, 80),
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
    // Keyed by the full handle: a native id is only unique within a harness, so two harnesses can
    // share one and must not collapse into the same header.
    let mut sessions: HashMap<HarnessSession, Session> = HashMap::new();
    let mut active: Option<HarnessSession> = None;

    // Color only in the pretty (terminal) view, and never when NO_COLOR is set.
    let color = matches!(style, Style::Pretty) && std::env::var_os("NO_COLOR").is_none();

    while let Some(event) = stream.next().await {
        let Some(event) = event?.event else {
            continue;
        };
        let stdout = io::stdout();
        let mut out = stdout.lock();

        if style.is_json() {
            let record = match event {
                tail_sessions_event::Event::SessionStarted(s) => {
                    TailEventJson::SessionStarted(session_json(&s.try_into()?))
                }
                tail_sessions_event::Event::SessionUpdated(s) => {
                    TailEventJson::SessionUpdated(session_json(&s.try_into()?))
                }
                tail_sessions_event::Event::Message(m) => {
                    TailEventJson::Message(message_json(&m.try_into()?))
                }
                tail_sessions_event::Event::Lagged(l) => {
                    TailEventJson::Lagged { dropped: l.dropped }
                }
            };
            serde_json::to_writer(&mut out, &record)?;
            writeln!(out)?;
            out.flush()?;
            continue;
        }

        match event {
            tail_sessions_event::Event::SessionStarted(s)
            | tail_sessions_event::Event::SessionUpdated(s) => {
                let s = Session::try_from(s)?;
                sessions.insert(s.handle.clone(), s);
            }
            tail_sessions_event::Event::Message(m) => {
                let m = Message::try_from(m)?;
                // Skip content-less records (meta/summary lines) so the tail stays legible.
                let Some(summary) = message_summary(&m) else {
                    continue;
                };
                let (role_text, role_ansi) = display_role(&m, &summary);
                if let Style::Plain = style {
                    writeln!(
                        out,
                        "{}  {:<12}  {:<11}  {:<9}  {}",
                        clock(m.timestamp),
                        short_id(m.session.session.as_ref()),
                        harness_name(m.session.harness),
                        role_text,
                        summary.render(false),
                    )?;
                } else {
                    if active.as_ref() != Some(&m.session) {
                        if active.is_some() {
                            writeln!(out)?;
                        }
                        writeln!(
                            out,
                            "{}",
                            tail_header(&m.session, sessions.get(&m.session), color)
                        )?;
                        active = Some(m.session.clone());
                    }
                    let time = paint(&clock(m.timestamp), Ansi::Dim, color);
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

async fn import(client: &mut AiClient, harness: Option<HarnessKind>, style: Style) -> Result<()> {
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
                    "harness": harness_name(p.harness()),
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
                    harness_name(p.harness()),
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
async fn resolve(client: &mut AiClient, selector: &str) -> Result<HarnessSession> {
    let mut stream = client
        .list_sessions(None)
        .await?
        .map(|session| Ok::<_, eyre::Report>(Session::try_from(session?)?));
    // `latest` only needs the newest session, which the daemon streams first, so take a single
    // item instead of draining the whole stream. Any id/prefix selector needs the full list to
    // match and disambiguate.
    let sessions: Vec<Session> = if selector.eq_ignore_ascii_case("latest") {
        stream.try_next().await?.into_iter().collect()
    } else {
        stream.try_collect().await?
    };
    select_session(sessions, selector)
}

/// Pure selector logic, split out from the RPC so it can be tested directly.
fn select_session(sessions: Vec<Session>, selector: &str) -> Result<HarnessSession> {
    if selector.eq_ignore_ascii_case("latest") {
        // The daemon lists newest-first, so the first entry is the most recent.
        let latest =
            sessions.into_iter().next().ok_or_else(|| eyre!("no sessions captured yet"))?;
        return Ok(latest.handle);
    }

    // `list` prints ids truncated to 12 chars, so accept a unique id prefix as well as a full id.
    let mut matches =
        sessions.into_iter().filter(|s| s.handle.session.as_ref().starts_with(selector));
    let first = matches
        .next()
        .ok_or_else(|| eyre!("no session with id `{selector}`. Run `atuin ai session list`."))?;
    if matches.next().is_some() {
        bail!("id `{selector}` matches more than one session; use a longer or full id");
    }
    Ok(first.handle)
}

// --- human rendering ----------------------------------------------------------------------------

fn write_session_header(out: &mut dyn Write, s: &Session) -> io::Result<()> {
    writeln!(out, "session   {}", sanitize(s.handle.session.as_ref()))?;
    writeln!(out, "harness   {}", harness_name(s.handle.harness))?;
    if let Some(title) = &s.title {
        writeln!(out, "title     {}", sanitize(title))?;
    }
    if let Some(cwd) = &s.cwd {
        writeln!(out, "cwd       {}", sanitize(&cwd.to_string_lossy()))?;
    }
    if let Some(branch) = &s.git_branch {
        writeln!(out, "branch    {}", sanitize(branch))?;
    }
    if let Some(model) = &s.model {
        writeln!(out, "model     {}", sanitize(model))?;
    }
    writeln!(out, "started   {}", age(s.started_at))?;
    writeln!(out, "updated   {}", age(s.updated_at))?;
    writeln!(out, "messages  {}", s.message_count)?;
    writeln!(
        out,
        "tokens    in {} / out {} / cache {}+{}",
        s.usage.input.unwrap_or_default(),
        s.usage.output.unwrap_or_default(),
        s.usage.cache_read.unwrap_or_default(),
        s.usage.cache_write.unwrap_or_default()
    )?;
    writeln!(out)
}

fn write_message_text(out: &mut dyn Write, m: &Message) -> io::Result<()> {
    writeln!(out, "── {} · {} ──", role_name(&m.role), age(m.timestamp))?;
    for block in &m.content {
        match block {
            Content::Text(t) => writeln!(out, "{}", sanitize(t))?,
            Content::Other(json) => writeln!(out, "{}", sanitize(&json.to_string()))?,
            Content::Reasoning(t) => writeln!(out, "[thinking] {}", sanitize(t))?,
            Content::ReasoningSummary { tokens } => {
                let tokens = tokens.or(m.usage.and_then(|u| u.reasoning));
                writeln!(out, "[thinking] {}", reasoning_label(tokens))?;
            }
            Content::Summary(t) => writeln!(out, "[summary] {}", sanitize(t))?,
            Content::Error(t) => writeln!(out, "[error] {}", sanitize(t))?,
            // A null input or an absent or empty output means capture did not keep it; print the
            // tag alone.
            Content::ToolUse(tu) => {
                write!(out, "[tool-call {}]", sanitize(&tu.name))?;
                if !tu.input.is_null() {
                    write!(out, " {}", sanitize(&tu.input.to_string()))?;
                }
                writeln!(out)?;
            }
            Content::ToolResult(tr) => {
                let tag = if tr.error {
                    "tool-error"
                } else {
                    "tool-result"
                };
                write!(out, "[{tag}]")?;
                if let Some(output) = tr.output_text().filter(|output| !output.is_empty()) {
                    write!(out, " {}", sanitize(&output))?;
                }
                writeln!(out)?;
            }
        }
    }
    writeln!(out)
}

fn short_id(session_id: &str) -> &str {
    session_id.get(..12).unwrap_or(session_id)
}

fn title_of(s: &Session) -> &str {
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
    /// A failed or aborted model call.
    Error(String),
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
            Self::Error(body) => format!("{} {body}", paint("✗", Ansi::Red, color)),
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
fn message_summary(m: &Message) -> Option<Summary> {
    for block in &m.content {
        match block {
            Content::Text(t) | Content::Summary(t) => {
                let line = one_line(t, SUMMARY_WIDTH);
                if !line.is_empty() {
                    return Some(Summary::Text(line));
                }
            }
            Content::Other(json) => {
                let line = one_line(&json.to_string(), SUMMARY_WIDTH);
                if !line.is_empty() {
                    return Some(Summary::Text(line));
                }
            }
            Content::Reasoning(t) => {
                let line = one_line(t, SUMMARY_WIDTH);
                if !line.is_empty() {
                    return Some(Summary::Thinking(line));
                }
            }
            Content::ReasoningSummary { tokens } => {
                let tokens = tokens.or(m.usage.and_then(|u| u.reasoning));
                return Some(Summary::Thinking(reasoning_label(tokens)));
            }
            Content::Error(t) => {
                return Some(Summary::Error(one_line(t, SUMMARY_WIDTH)));
            }
            Content::ToolUse(tu) => {
                // Fold like the sibling arms: the tool name is captured content and must not carry
                // control chars into the `tail` view.
                return Some(Summary::ToolCall(one_line(&tu.name, SUMMARY_WIDTH)));
            }
            Content::ToolResult(tr) => {
                return Some(Summary::ToolResult {
                    is_error: tr.error,
                    body: tr
                        .output_text()
                        .map(|output| one_line(&output, SUMMARY_WIDTH))
                        .unwrap_or_default(),
                });
            }
        }
    }
    None
}

/// The role label to display: the harness's own string when the enum cannot name it (e.g. codex
/// `developer`), otherwise the standard role name.
fn message_role(m: &Message) -> String {
    // The harness's own role is free-form captured text, so strip any control chars before it
    // reaches the `tail` view; the fixed names (role_name) need no such care.
    match &m.role {
        Role::Other(label) if !label.is_empty() => sanitize(label).into_owned(),
        role => role_name(role).to_owned(),
    }
}

/// The role text and its color for a rendered `tail` line. A tool result is labelled `tool`
/// whatever the envelope role, since some harnesses model tool output as a user turn.
fn display_role(m: &Message, summary: &Summary) -> (String, Ansi) {
    if matches!(summary, Summary::ToolResult { .. }) {
        ("tool".to_owned(), role_color(&Role::Tool))
    } else {
        (message_role(m), role_color(&m.role))
    }
}

/// The session-group header line printed the first time a session appears in the pretty `tail`
/// view and whenever the active session changes.
fn tail_header(handle: &HarnessSession, session: Option<&Session>, color: bool) -> String {
    let bullet = paint("●", harness_color(handle.harness), color);
    let id = paint(short_id(handle.session.as_ref()), Ansi::Bold, color);
    let harness_label = paint(harness_name(handle.harness), Ansi::Dim, color);
    let title = session.map(title_of).map(|t| one_line(t, SUMMARY_WIDTH)).unwrap_or_default();
    if title.is_empty() {
        format!("{bullet} {id} · {harness_label}")
    } else {
        format!("{bullet} {id} · {harness_label} · {title}")
    }
}

/// Local wall-clock `HH:MM:SS`, for live `tail` lines.
fn clock(ts: OffsetDateTime) -> String {
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

fn role_color(role: &Role) -> Ansi {
    match role {
        Role::Assistant => Ansi::Cyan,
        Role::User => Ansi::Yellow,
        Role::System => Ansi::Magenta,
        Role::Tool => Ansi::Blue,
        Role::Other(_) => Ansi::Dim,
    }
}

fn harness_color(harness: HarnessKind) -> Ansi {
    match harness {
        HarnessKind::ClaudeCode => Ansi::Magenta,
        HarnessKind::Codex => Ansi::Green,
        HarnessKind::Copilot => Ansi::Blue,
        HarnessKind::Opencode => Ansi::Cyan,
        HarnessKind::Pi => Ansi::Yellow,
        HarnessKind::Unknown => Ansi::Dim,
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

/// A humanized age (e.g. "2 hours ago"), or "-" when chrono cannot represent it.
fn age(ts: OffsetDateTime) -> String {
    to_datetime(ts).map_or_else(|| "-".to_owned(), |dt| HumanTime::from(dt).to_string())
}

fn to_datetime(ts: OffsetDateTime) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(ts.unix_timestamp(), ts.nanosecond())
}

fn rfc3339(ts: OffsetDateTime) -> Option<String> {
    to_datetime(ts).map(|dt| dt.to_rfc3339())
}

fn parse_harness(value: &str) -> Result<HarnessKind, String> {
    match value {
        "claude-code" => Ok(HarnessKind::ClaudeCode),
        "codex" => Ok(HarnessKind::Codex),
        "pi" => Ok(HarnessKind::Pi),
        other => Err(format!("unknown harness `{other}` (expected claude-code, codex, or pi)")),
    }
}

/// The kebab display label for a harness. Kept exhaustive over every `HarnessKind` (including ones
/// no capture path yet produces) so a stored value always renders. Shared with the MCP
/// session-search renderer.
pub fn harness_name(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::ClaudeCode => "claude-code",
        HarnessKind::Codex => "codex",
        HarnessKind::Copilot => "copilot",
        HarnessKind::Opencode => "opencode",
        HarnessKind::Pi => "pi",
        HarnessKind::Unknown => "unknown",
    }
}

fn role_name(role: &Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Tool => "tool",
        Role::Other(_) => "unknown",
    }
}

fn stop_reason_name(stop_reason: Option<&StopReason>) -> &'static str {
    match stop_reason {
        Some(StopReason::EndTurn) => "end_turn",
        Some(StopReason::ToolUse) => "tool_use",
        Some(StopReason::MaxTokens) => "max_tokens",
        Some(StopReason::Aborted) => "aborted",
        Some(StopReason::Error) => "error",
        Some(StopReason::StopSequence) => "stop_sequence",
        Some(StopReason::Refusal) => "refusal",
        Some(StopReason::Other(_)) | None => "unknown",
    }
}

// --- JSON view structs --------------------------------------------------------------------------
//
// The domain types' serde shape is the record store's, so JSON output goes through these owned
// views with a stable, documented shape.

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
    tokens: Usage,
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
    Summary {
        text: String,
    },
    Error {
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
    id: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<Usage>,
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
    title: HighlightJson,
    preview: HighlightJson,
}

impl From<&HighlightedString> for HighlightJson {
    fn from(highlighted: &HighlightedString) -> Self {
        let plain = highlighted.to_plain();
        Self {
            text: plain.text.into_owned(),
            matches: plain.ranges.iter().map(|r| [r.start, r.end]).collect(),
        }
    }
}

impl From<&SessionMatch> for SearchMatchJson {
    fn from(m: &SessionMatch) -> Self {
        Self {
            session: session_json(&m.session),
            score: m.score,
            title: HighlightJson::from(&m.title),
            preview: HighlightJson::from(&m.preview),
        }
    }
}

fn handle_json(handle: &HarnessSession) -> HandleJson {
    HandleJson {
        harness: harness_name(handle.harness).to_owned(),
        session_id: handle.session.to_string(),
    }
}

fn session_json(s: &Session) -> SessionJson {
    SessionJson {
        harness: harness_name(s.handle.harness).to_owned(),
        session_id: s.handle.session.to_string(),
        parent: s.parent.as_ref().map(handle_json),
        cwd: s.cwd.as_ref().map(|cwd| cwd.to_string_lossy().into_owned()),
        git_branch: s.git_branch.clone(),
        model: s.model.clone(),
        started_at: rfc3339(s.started_at),
        updated_at: rfc3339(s.updated_at),
        message_count: s.message_count,
        tokens: s.usage,
        title: s.title.clone(),
        preview: s.preview.clone(),
    }
}

/// `reasoning` is the message's reasoning token count, for a summary block that lacks its own.
fn content_json(block: &Content, reasoning: Option<u64>) -> ContentJson {
    match block {
        Content::Text(t) => ContentJson::Text { text: t.clone() },
        Content::Other(json) => ContentJson::Text {
            text: json.to_string(),
        },
        Content::Reasoning(t) => ContentJson::Thinking { text: t.clone() },
        Content::ReasoningSummary { tokens } => ContentJson::Thinking {
            text: reasoning_label(tokens.or(reasoning)),
        },
        Content::Summary(t) => ContentJson::Summary { text: t.clone() },
        Content::Error(t) => ContentJson::Error { text: t.clone() },
        Content::ToolUse(tu) => ContentJson::ToolCall {
            id: tu.id.to_string(),
            name: tu.name.clone(),
            input: if tu.input.is_null() {
                String::new()
            } else {
                tu.input.to_string()
            },
        },
        Content::ToolResult(tr) => ContentJson::ToolResult {
            tool_use_id: tr.call.to_string(),
            content: tr.output_text().unwrap_or_default().into_owned(),
            is_error: tr.error,
        },
    }
}

fn message_json(m: &Message) -> MessageJson {
    let reasoning = m.usage.and_then(|u| u.reasoning);
    MessageJson {
        id: m.id.0.to_string(),
        role: message_role(m),
        timestamp: rfc3339(m.timestamp),
        model: m.model.clone(),
        cwd: m.cwd.as_ref().map(|cwd| cwd.to_string_lossy().into_owned()),
        git_branch: m.git_branch.clone(),
        content: m.content.iter().map(|block| content_json(block, reasoning)).collect(),
        tokens: m.usage,
        stop_reason: match &m.stop_reason {
            Some(StopReason::Other(label)) => label.clone(),
            reason => stop_reason_name(reason.as_ref()).to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::ai_session::{NativeSessionId, SourceId};
    use atuin_common::harnesstools::session::{ToolCallId, ToolResult, ToolUse};
    use atuin_common::string::highlighted::TextHighlighter;
    use atuin_domain::record::RecordId;
    use rstest::rstest;
    use serde_json::{Value, json};

    use super::*;

    fn handle(harness: HarnessKind, id: &str) -> HarnessSession {
        HarnessSession {
            harness,
            session: NativeSessionId::from(id.to_owned()),
        }
    }

    fn session(harness: HarnessKind, id: &str) -> Session {
        Session::builder()
            .handle(handle(harness, id))
            .started_at(OffsetDateTime::UNIX_EPOCH)
            .updated_at(OffsetDateTime::UNIX_EPOCH)
            .usage(Usage::default())
            .build()
    }

    #[rstest]
    #[case("claude-code", HarnessKind::ClaudeCode)]
    #[case("codex", HarnessKind::Codex)]
    #[case("pi", HarnessKind::Pi)]
    fn parse_harness_maps_names(#[case] input: &str, #[case] want: HarnessKind) {
        assert_eq!(parse_harness(input).unwrap(), want);
    }

    #[rstest]
    fn parse_harness_rejects_unknown_harnesses() {
        assert!(parse_harness("opencode").is_err());
    }

    #[rstest]
    fn latest_picks_the_first_listed() {
        let sessions = vec![session(HarnessKind::Codex, "newest"), session(HarnessKind::Pi, "old")];
        assert_eq!(
            select_session(sessions, "latest").unwrap(),
            handle(HarnessKind::Codex, "newest")
        );
    }

    #[rstest]
    fn latest_on_empty_is_an_error() {
        assert!(select_session(Vec::new(), "latest").is_err());
    }

    #[rstest]
    fn latest_resolves_from_a_single_session() {
        // `resolve` now hands `select_session` just the newest session for `latest`, so a
        // one-element list must still resolve.
        let selected =
            select_session(vec![session(HarnessKind::ClaudeCode, "only")], "latest").unwrap();
        assert_eq!(selected, handle(HarnessKind::ClaudeCode, "only"));
    }

    #[rstest]
    fn exact_id_resolves_the_harness_from_the_listing() {
        let sessions =
            vec![session(HarnessKind::Codex, "aaa"), session(HarnessKind::ClaudeCode, "bbb")];
        let selected = select_session(sessions, "bbb").unwrap();
        assert_eq!(selected, handle(HarnessKind::ClaudeCode, "bbb"));
    }

    #[rstest]
    fn id_prefix_resolves_a_session() {
        // `list` prints ids truncated, so a copied prefix must resolve.
        let sessions = vec![
            session(HarnessKind::Codex, "abcdef0123456789"),
            session(HarnessKind::ClaudeCode, "fedcba9876543210"),
        ];
        let selected = select_session(sessions, "abcdef012345").unwrap();
        assert_eq!(selected, handle(HarnessKind::Codex, "abcdef0123456789"));
    }

    #[rstest]
    fn ambiguous_prefix_is_an_error() {
        let sessions =
            vec![session(HarnessKind::Codex, "abc111"), session(HarnessKind::ClaudeCode, "abc222")];
        assert!(select_session(sessions, "abc").is_err());
    }

    #[rstest]
    fn unknown_id_is_an_error() {
        let sessions = vec![session(HarnessKind::Codex, "aaa")];
        assert!(select_session(sessions, "zzz").is_err());
    }

    #[rstest]
    fn ambiguous_id_across_harnesses_is_an_error() {
        let sessions =
            vec![session(HarnessKind::Codex, "dup"), session(HarnessKind::ClaudeCode, "dup")];
        assert!(select_session(sessions, "dup").is_err());
    }

    #[rstest]
    fn session_json_has_a_stable_shape() {
        let mut s = session(HarnessKind::ClaudeCode, "abcdef0123456789");
        s.message_count = 3;
        s.usage = Usage {
            input: Some(10),
            output: Some(20),
            cache_read: Some(1),
            cache_write: Some(2),
            reasoning: None,
        };
        s.title = Some("hello".to_owned());

        let v = serde_json::to_value(session_json(&s)).unwrap();
        assert_eq!(v["harness"], "claude-code");
        assert_eq!(v["session_id"], "abcdef0123456789");
        assert_eq!(v["message_count"], 3);
        assert_eq!(v["tokens"]["input"], 10);
        assert_eq!(v["tokens"]["output"], 20);
        // A count the harness did not report stays absent rather than reading as 0.
        assert_eq!(v["tokens"]["reasoning"], Value::Null);
        assert_eq!(v["title"], "hello");
        // Absent optionals are omitted rather than serialized as null.
        assert!(v.get("cwd").is_none());
    }

    #[rstest]
    #[case(HarnessKind::ClaudeCode, "claude-code")]
    #[case(HarnessKind::Pi, "pi")]
    #[case(HarnessKind::Unknown, "unknown")]
    fn harness_name_maps_known_and_unknown(#[case] harness: HarnessKind, #[case] expected: &str) {
        assert_eq!(harness_name(harness), expected);
    }

    fn msg(role: Role, content: Vec<Content>) -> Message {
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(handle(HarnessKind::ClaudeCode, "s"))
            .source_id(SourceId::from("m".to_owned()))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .role(role)
            .content(content)
            .build()
    }

    fn tool_call(name: &str) -> Content {
        Content::ToolUse(ToolUse {
            id: ToolCallId::from("c1".to_owned()),
            name: name.to_owned(),
            input: Value::Null,
        })
    }

    fn tool_result(output: Value, error: bool) -> Content {
        Content::ToolResult(ToolResult {
            call: ToolCallId::from("c1".to_owned()),
            output,
            error,
        })
    }

    #[rstest]
    fn summary_prefers_text() {
        let m = msg(Role::Assistant, vec![Content::Text("hello world".to_owned())]);
        assert_eq!(message_summary(&m).unwrap().render(false), "hello world");
    }

    #[rstest]
    fn summary_labels_a_tool_call() {
        let m = msg(Role::Assistant, vec![tool_call("Edit")]);
        assert_eq!(message_summary(&m).unwrap().render(false), "⚙ Edit");
    }

    #[rstest]
    fn tail_render_strips_control_chars_from_tool_name_and_role_label() {
        // The tail view prints the captured tool-call name and free-form role label directly; a
        // recorded session must not smuggle terminal escapes through either sink.
        let m = msg(Role::Other("dev\x1b[31mil".to_owned()), vec![tool_call(
            "run\x1b]0;pwned\x07 now",
        )]);

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
        let m = msg(Role::Tool, vec![tool_result(json!(content), is_error)]);
        assert_eq!(message_summary(&m).unwrap().render(false), expected);
    }

    #[rstest]
    fn summary_skips_content_less_messages() {
        assert!(message_summary(&msg(Role::User, vec![])).is_none());
        let blank = msg(Role::User, vec![Content::Text("   ".to_owned())]);
        assert!(message_summary(&blank).is_none());
    }

    #[rstest]
    fn summary_marks_thinking() {
        let m = msg(Role::Assistant, vec![Content::Reasoning("pondering".to_owned())]);
        assert_eq!(message_summary(&m).unwrap().render(false), "» pondering");
    }

    #[rstest]
    fn role_label_overrides_the_enum() {
        let m = msg(Role::Other("developer".to_owned()), vec![]);
        assert_eq!(message_role(&m), "developer");
        // A standard role falls back to the enum name.
        assert_eq!(message_role(&msg(Role::User, vec![])), "user");
    }

    #[rstest]
    fn tool_result_line_is_labelled_tool() {
        // Claude models a tool result as a user-turn message; the line should still say "tool".
        let m = msg(Role::User, vec![tool_result(json!("ok"), false)]);
        let summary = message_summary(&m).unwrap();
        assert_eq!(display_role(&m, &summary).0, "tool");
    }

    #[rstest]
    fn summary_falls_through_empty_text_to_the_next_block() {
        let m = msg(Role::Assistant, vec![Content::Text(String::new()), tool_call("Bash")]);
        assert_eq!(message_summary(&m).unwrap().render(false), "⚙ Bash");
    }

    #[rstest]
    fn header_includes_title_when_present() {
        let mut s = session(HarnessKind::ClaudeCode, "abcdef0123456789");
        s.title = Some("My Session".to_owned());
        assert_eq!(
            tail_header(&s.handle, Some(&s), false),
            "● abcdef012345 · claude-code · My Session"
        );
    }

    #[rstest]
    fn header_omits_title_when_absent() {
        assert_eq!(
            tail_header(&handle(HarnessKind::ClaudeCode, "abcdef0123456789"), None, false),
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
    #[case(HarnessArg::ClaudeCode, HarnessKind::ClaudeCode)]
    #[case(HarnessArg::Codex, HarnessKind::Codex)]
    #[case(HarnessArg::Opencode, HarnessKind::Opencode)]
    #[case(HarnessArg::Pi, HarnessKind::Pi)]
    fn harness_arg_maps_to_pb(#[case] arg: HarnessArg, #[case] expected: HarnessKind) {
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
        let highlighter = TextHighlighter::with_markers(['\u{E000}', '\u{E001}']).unwrap();
        let m = SessionMatch {
            session: session(HarnessKind::ClaudeCode, "abc"),
            title: highlighter.as_highlighted("the \u{E000}build\u{E001}".to_owned()),
            preview: highlighter.as_highlighted(String::new()),
            score: 2.5,
        };

        let v = serde_json::to_value(SearchMatchJson::from(&m)).unwrap();
        assert_eq!(v["session"]["session_id"], "abc");
        assert_eq!(v["score"], 2.5);
        assert_eq!(v["title"]["text"], "the build");
        assert_eq!(v["title"]["matches"], json!([[4, 9]]));
        assert!(v["preview"].get("matches").is_none(), "no matches are omitted, not empty");
    }
}
