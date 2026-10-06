//! Continuing a session in a different harness from the one that recorded it: the captured
//! conversation, turned into a new session the target harness can write out
//! ([`Harness::rehydrate`](crate::harnesstools::Harness::rehydrate)) and resume.
//!
//! A continuation is a **new** session in the target, under a fresh id in the target's own
//! format, never a copy of the original: the target captures it as a session of its own, linked
//! to the original by its first line, the [marker](marker_text).
//!
//! # What is carried over
//!
//! The conversation, and the work done in it:
//!
//! - What the user said, and what the model answered.
//! - **Tool calls, as calls.** A call captured with its input and its result (`ai.capture_tools`)
//!   is carried over as a call of the target, with its result: as the target's own tool where it
//!   has one that does the same (a shell command, a file read, written or edited, a search: see
//!   `tools`), else as the tool it was. A model reads a call to a tool it doesn't have like any
//!   other; it just can't make it again.
//! - **Other calls, as notes**: a short line of text in the assistant's turn saying what was
//!   called (see [`tool_note`]): `[ran a shell command]`, ``[edited `src/store.rs`]``. That is a
//!   call captured without its input (by default capture keeps the tool's name only), one with no
//!   result (interrupted, or a Codex web search), and one the model's provider ran itself (a
//!   Claude Code web search), whose result only that provider reads.
//! - Summaries (compactions, abandoned branches), as text of the user's turn.
//! - Failed model calls, as a short note of the error in the assistant's turn.
//!
//! Dropped: reasoning (only the model that wrote it can read it back), text the source harness
//! injected itself (its system context, reminders, environment), usage (it belongs to the
//! original's model calls), and the model: the target resumes on its own default.
//!
//! The target's history then has a valid shape for any of the APIs behind it: after the marker,
//! user and assistant turns alternate, starting with the user and ending with the assistant.
//! Rows of one role in a row (two prompts whose reply was only tool calls, say) are merged into
//! one turn. An assistant turn's calls are written in the target's own rows, each answered by
//! its result before the turn goes on. Timestamps keep the original's, nudged forward where
//! needed so every row is later than the one before (opencode orders messages by ids minted from
//! their time).
//!
//! # The marker
//!
//! The session starts with a [`marker_text`] naming the source harness and session (and the
//! session's atuin id, for whoever reads it: the link capture stores is the harness's), written as
//! the least intrusive line each target has: text the harness itself adds to the user's turn (a
//! Claude Code `isMeta` line, a Codex `developer` message, an opencode `synthetic` part), which
//! the model reads with the first prompt and the user never sees. Pi has no such line: the only
//! lines it sends the model that nobody typed (`custom_message`, summaries) each become a user
//! message of their own, two user messages in a row. So there the marker is the first text block
//! of the first prompt, a short first line the user sees too. Capture reads it back as the
//! session's parent ([`continued_from_message`]).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::note::{Part, clip, render, tool_note};
use super::rehydrate::{RehydrateMessage, RehydrateSession};
use super::resume::is_plain_name;
use super::{AnyHarness, Harness as _, opencode};
use crate::harnesstools::session::{
    Content, Message, ParentKind, Role, StopReason, ToolCallId, ToolResult, ToolUse,
};

mod tools;

/// The tools Anthropic's API runs itself (Claude Code's `server_tool_use`), answered in the
/// assistant's own message.
const PROVIDER_TOOLS: &[&str] = &[
    "web_search",
    "web_fetch",
    "code_execution",
    "bash_code_execution",
    "text_editor_code_execution",
    "tool_search_tool_regex",
    "tool_search_tool_bm25",
];

/// How much of an error a note shows.
const NOTE_ERROR: usize = 200;

/// The first line of every marker ends with this.
const MARKER_TAIL: &str = " via atuin.";
const MARKER_HEAD: &str = "Continued from ";
/// A [fork marker](fork_marker_text) starts with this instead.
const FORK_HEAD: &str = "Forked from ";
/// Before the tail, when the marker names the session's atuin id.
const MARKER_ATUIN_ID: &str = " (atuin id ";

/// Why a session can't be continued: it holds nothing of the conversation to carry over. Such a
/// continuation would be its marker alone, and in pi not even that: there the marker rides on the
/// first prompt, and without one the new session wouldn't be linked to the original.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("nothing to continue: the session has no messages")]
pub struct NothingToContinue;

/// A session to continue in another harness, and what was left out of it.
#[derive(Clone, Debug)]
pub struct Continuation {
    /// The new session, ready for the target's [`Harness::rehydrate`].
    ///
    /// [`Harness::rehydrate`]: crate::harnesstools::Harness::rehydrate
    pub session: RehydrateSession,
    pub flattened: Flattened,
}

/// What a continuation left out, or carried over only as text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flattened {
    /// Tool calls turned into notes (the rest are carried over as calls).
    pub tool_calls: usize,
    /// Tool results dropped: those of calls turned into notes, and any answering no call.
    pub tool_results: usize,
    /// Messages whose reasoning was dropped.
    pub reasoning: usize,
}

impl Flattened {
    /// What was left out, for a status line: `42 tool calls become notes, reasoning
    /// dropped`. Empty when nothing was.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        match self.tool_calls {
            0 => {}
            1 => parts.push("1 tool call becomes a note".to_owned()),
            n => parts.push(format!("{n} tool calls become notes")),
        }
        if self.tool_calls == 0 && self.tool_results > 0 {
            parts.push("tool output dropped".to_owned());
        }
        if self.reasoning > 0 {
            parts.push("reasoning dropped".to_owned());
        }
        parts.join(", ")
    }
}

/// The harness's name as people know it.
fn label(harness: AnyHarness) -> &'static str {
    match harness {
        AnyHarness::ClaudeCode(_) => "Claude Code",
        AnyHarness::Codex(_) => "Codex",
        AnyHarness::Opencode(_) => "opencode",
        AnyHarness::Pi(_) => "Pi",
    }
}

/// The marker a continuation starts with: its first line names the session it continues
/// (`Continued from <harness> session <id> (atuin id <atuin id>) via atuin.`, the harness by its
/// [name](crate::harnesstools::Harness::name), without the atuin id when it isn't known); the
/// rest tells the model how to read what follows.
#[must_use]
pub fn marker_text(source: AnyHarness, id: &str, atuin_id: Option<&str>) -> String {
    let atuin_id = atuin_id.map(|a| format!("{MARKER_ATUIN_ID}{a})")).unwrap_or_default();
    format!(
        "{MARKER_HEAD}{} session {id}{atuin_id}{MARKER_TAIL}\nThe conversation below was recorded \
         in {} and carried over. Its tool calls may name tools you don't have: use your own. \
         Calls it couldn't carry are shown as notes in [brackets]. The files may have changed \
         since: check them before relying on what was done.",
        source.name(),
        label(source),
    )
}

/// The one line an opencode [fork](crate::harnesstools::fork) starts with, naming the session
/// it was forked from: `Forked from <harness> session <id> (atuin id <atuin id>) via atuin.`,
/// as [`marker_text`]'s first line does. opencode keeps it from the model (an `ignored` part).
#[must_use]
pub fn fork_marker_text(source: AnyHarness, id: &str, atuin_id: Option<&str>) -> String {
    let atuin_id = atuin_id.map(|a| format!("{MARKER_ATUIN_ID}{a})")).unwrap_or_default();
    format!("{FORK_HEAD}{} session {id}{atuin_id}{MARKER_TAIL}", source.name())
}

/// How `text` links its session to another, when it is a [marker](marker_text) or a
/// [fork marker](fork_marker_text), read from its first line: the kind of link, and the harness,
/// session and atuin id (if named) it names.
fn marker_parts(text: &str) -> Option<(ParentKind, AnyHarness, &str, Option<&str>)> {
    let first = text.lines().next()?;
    let (kind, rest) = match first.strip_prefix(MARKER_HEAD) {
        Some(rest) => (ParentKind::Continuation, rest),
        None => (ParentKind::Fork, first.strip_prefix(FORK_HEAD)?),
    };
    let (name, rest) = rest.strip_suffix(MARKER_TAIL)?.split_once(" session ")?;
    let (id, atuin_id) = match rest.split_once(MARKER_ATUIN_ID) {
        Some((id, atuin_id)) => (id, Some(atuin_id.strip_suffix(')')?)),
        None => (rest, None),
    };
    let harness = AnyHarness::from_name(name).ok()?;
    (is_plain_name(id) && !id.contains(char::is_whitespace))
        .then_some((kind, harness, id, atuin_id))
}

/// The harness and session `text` says it continues, when it is a [marker](marker_text), with
/// or without the atuin id (which capture doesn't need: the harness's id is the link).
#[must_use]
pub fn continued_from(text: &str) -> Option<(AnyHarness, &str)> {
    marker_parts(text)
        .filter(|(kind, ..)| *kind == ParentKind::Continuation)
        .map(|(_, harness, id, _)| (harness, id))
}

/// Whether `text` is a [fork marker](fork_marker_text).
#[must_use]
pub fn is_fork_marker(text: &str) -> bool {
    marker_parts(text).is_some_and(|(kind, ..)| kind == ParentKind::Fork)
}

/// The harness and session a transcript line says its session continues: a [marker](marker_text)
/// the harness put in the user's turn itself, or (pi's) the first block of a prompt with more
/// after it. That block must be the whole marker exactly as [`marker_text`] writes it (so its
/// wording must not change), not just its first line: text pasted with an image is a prompt's
/// first block too. How capture links a continuation to the session it continues.
#[must_use]
pub fn continued_from_message<M: Message + ?Sized>(m: &M) -> Option<(AnyHarness, String)> {
    linked_from_message(m)
        .filter(|(_, _, kind)| *kind == ParentKind::Continuation)
        .map(|(harness, id, _)| (harness, id))
}

/// The harness and session a transcript line links its session to, and how: a continuation's
/// [marker](marker_text) as [`continued_from_message`] reads it, and an opencode fork's
/// [marker](fork_marker_text) as a [fork](ParentKind::Fork). How capture links a continuation, or
/// an opencode fork, to the session it came from.
#[must_use]
pub fn linked_from_message<M: Message + ?Sized>(m: &M) -> Option<(AnyHarness, String, ParentKind)> {
    let content = m.content();
    let marker = match (m.role(), content.as_slice()) {
        (Role::Assistant | Role::Tool, _) => return None,
        (Role::User, [Content::Text(first), _, ..]) => {
            let (kind, harness, id, atuin_id) = marker_parts(first)?;
            let exact = match kind {
                ParentKind::Fork => fork_marker_text(harness, id, atuin_id),
                _ => marker_text(harness, id, atuin_id),
            };
            return (*first == exact).then(|| (harness, id.to_owned(), kind));
        }
        (Role::User, _) => return None,
        (_, content) => content.iter().find_map(|c| match c {
            Content::Text(text) if marker_parts(text).is_some() => Some(text),
            _ => None,
        })?,
    };
    marker_parts(marker).map(|(kind, harness, id, _)| (harness, id.to_owned(), kind))
}

/// That `messages` hold something of the conversation, so a continuation or fork of them isn't
/// empty.
pub fn conversational(messages: &[RehydrateMessage]) -> Result<(), NothingToContinue> {
    if turns(messages).0.is_empty() {
        return Err(NothingToContinue);
    }
    Ok(())
}

/// What continuing `original` in another harness would flatten or drop, whichever harness, or
/// that there's nothing to continue.
pub fn flattened(original: &RehydrateSession) -> Result<Flattened, NothingToContinue> {
    let (turns, flattened) = turns(&original.messages);
    if turns.is_empty() {
        return Err(NothingToContinue);
    }
    Ok(flattened)
}

/// `original`, recorded by `source`, as a new session of `target` resuming in `original.cwd`.
/// Its marker names `original`'s atuin id too, when given. Refused, whatever the target, when
/// `original` has no conversation to carry over.
pub fn continue_in(
    source: AnyHarness,
    original: &RehydrateSession,
    atuin_id: Option<&str>,
    target: AnyHarness,
) -> Result<Continuation, NothingToContinue> {
    continue_at(source, original, atuin_id, target, OffsetDateTime::now_utc())
}

/// [`continue_in`], its new session minted at `now`.
fn continue_at(
    source: AnyHarness,
    original: &RehydrateSession,
    atuin_id: Option<&str>,
    target: AnyHarness,
    now: OffsetDateTime,
) -> Result<Continuation, NothingToContinue> {
    let (turns, flattened) = turns(&original.messages);
    let Some(start) = turns.first().map(|t| t.at) else {
        return Err(NothingToContinue);
    };
    let ids = Ids::new(target, now);
    let id = ids.session();

    let mut rows = Rows {
        source,
        target,
        ids,
        calls: tools::CallIds::default(),
        rows: Vec::with_capacity(turns.len() + 1),
        last: start,
        steps: 0,
        step: None,
    };
    let marker = marker_text(source, &original.id, atuin_id);
    // pi has no line the model reads that the user didn't type but its own user message: the
    // marker is the first block of the first prompt (see the module docs).
    let pi = matches!(target, AnyHarness::Pi(_));
    if pi {
        rows.last -= Duration::milliseconds(1);
    } else {
        rows.push(start, Role::System, vec![Content::Text(marker.clone())], None);
    }
    for turn in turns {
        match turn.kind {
            Kind::User => {
                let mut content = vec![Content::Text(turn.render())];
                if pi && rows.rows.is_empty() {
                    content.insert(0, Content::Text(marker.clone()));
                }
                rows.push(turn.at, Role::User, content, None);
            }
            Kind::Assistant => {
                let at = turn.at;
                // The calls ran where the session did, which a continuation on another machine
                // resumes elsewhere.
                let ran_in = original.original_cwd.as_deref().unwrap_or(&original.cwd);
                for step in turn.steps() {
                    rows.step(at, &step, ran_in, &original.cwd);
                }
            }
        }
    }
    // A session that stopped with its calls answered (interrupted, or out of tokens) still ends
    // on the assistant's turn.
    let last = rows.rows.last().expect("a continuation has turns");
    if last.content.iter().any(|c| matches!(c, Content::ToolResult(_))) {
        let note = Content::Text("[the original session stopped here]".to_owned());
        let at = rows.last;
        rows.open_step();
        rows.push(at, Role::Assistant, vec![note], Some(StopReason::EndTurn));
    }
    let mut rows = rows.rows;
    // Only Claude Code and Pi keep a tree; opencode reads a row's parent as the message it
    // answers, which it finds on its own.
    if !matches!(target, AnyHarness::ClaudeCode(_) | AnyHarness::Pi(_)) {
        for message in &mut rows {
            message.parent_source_id = None;
        }
    }

    Ok(Continuation {
        session: RehydrateSession {
            id,
            title: original.title.clone(),
            cwd: original.cwd.clone(),
            original_cwd: Some(original.cwd.clone()),
            git_branch: original.git_branch.clone(),
            model: None,
            started_at: start,
            messages: rows,
            fork_of: None,
        },
        flattened,
    })
}

/// A continuation's rows as they are written, each later than the one before and hanging from
/// it.
struct Rows {
    source: AnyHarness,
    target: AnyHarness,
    ids: Ids,
    calls: tools::CallIds,
    rows: Vec<RehydrateMessage>,
    last: OffsetDateTime,
    /// The steps written so far, and the key of the one being written, for opencode: its rows
    /// are its parts, which it folds into one message for as long as their turn is the same (see
    /// `opencode::rehydrate::Draft::holds`), so each step's rows name one of their own. Else
    /// every call of a turn, and the text after their results, would be one model call's.
    steps: usize,
    step: Option<String>,
}

impl Rows {
    fn push(
        &mut self,
        at: OffsetDateTime,
        role: Role,
        content: Vec<Content>,
        stop: Option<StopReason>,
    ) {
        let at = at.max(self.last + Duration::milliseconds(1));
        self.last = at;
        self.rows.push(RehydrateMessage {
            source_id: self.ids.row(at),
            parent_source_id: self.rows.last().map(|r| r.source_id.clone()),
            timestamp: at,
            role,
            content,
            model: None,
            usage: None,
            stop_reason: stop,
            turn_id: self.step.clone(),
            cwd: None,
            git_branch: None,
        });
    }

    /// A step of an assistant's turn, as the target writes one: its text, then its calls, each
    /// answered by its result. Claude Code and pi send a message's text and calls together,
    /// then the results (Claude Code all in one message, pi one each); Codex writes every item
    /// as a line of its own, and opencode every part, a call and its result being one.
    fn step(&mut self, at: OffsetDateTime, step: &Step<'_>, ran_in: &Path, cwd: &Path) {
        self.open_step();
        self.write_step(at, step, ran_in, cwd);
        self.step = None;
    }

    /// Start a step of the assistant's: for opencode, the rows pushed until it ends name it.
    fn open_step(&mut self) {
        if matches!(self.target, AnyHarness::Opencode(_)) {
            self.steps += 1;
            self.step = Some(format!("step-{}", self.steps));
        }
    }

    fn write_step(&mut self, at: OffsetDateTime, step: &Step<'_>, ran_in: &Path, cwd: &Path) {
        let text = (!step.parts.is_empty()).then(|| Content::Text(render(&step.parts)));
        let (calls, results): (Vec<_>, Vec<_>) = step
            .calls
            .iter()
            .map(|(call, result)| {
                tools::carry(self.source, self.target, call, result, ran_in, cwd, &mut self.calls)
            })
            .unzip();
        if calls.is_empty() {
            self.push(at, Role::Assistant, text.into_iter().collect(), Some(StopReason::EndTurn));
            return;
        }
        let calling = Some(StopReason::ToolUse);
        match self.target {
            AnyHarness::ClaudeCode(_) | AnyHarness::Pi(_) => {
                let content = text.into_iter().chain(calls.into_iter().map(Content::ToolUse));
                self.push(at, Role::Assistant, content.collect(), calling);
                if matches!(self.target, AnyHarness::ClaudeCode(_)) {
                    let results = results.into_iter().map(Content::ToolResult).collect();
                    self.push(at, Role::Tool, results, None);
                } else {
                    for result in results {
                        self.push(at, Role::Tool, vec![Content::ToolResult(result)], None);
                    }
                }
            }
            AnyHarness::Codex(_) | AnyHarness::Opencode(_) => {
                if let Some(text) = text {
                    self.push(at, Role::Assistant, vec![text], Some(StopReason::EndTurn));
                }
                let codex = matches!(self.target, AnyHarness::Codex(_));
                for (call, result) in calls.into_iter().zip(results) {
                    let (call, result) = (Content::ToolUse(call), Content::ToolResult(result));
                    if codex {
                        self.push(at, Role::Assistant, vec![call], calling.clone());
                        self.push(at, Role::Tool, vec![result], None);
                    } else {
                        self.push(at, Role::Assistant, vec![call, result], calling.clone());
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    User,
    Assistant,
}

/// A piece of a turn: text or a note, a call carried over with its result, or where the model
/// was called again once results came back.
#[derive(Debug)]
enum Item<'m> {
    Part(Part),
    Call(&'m ToolUse, &'m ToolResult),
    Answered,
}

/// One side's turn: the rows of one role in a row, merged.
#[derive(Debug)]
struct Turn<'m> {
    kind: Kind,
    at: OffsetDateTime,
    items: Vec<Item<'m>>,
}

/// A stretch of an assistant's turn: what it said, then the calls it made.
#[derive(Debug, Default)]
struct Step<'m> {
    parts: Vec<Part>,
    calls: Vec<(&'m ToolUse, &'m ToolResult)>,
}

impl<'m> Turn<'m> {
    fn parts(kind: Kind, at: OffsetDateTime, parts: Vec<Part>) -> Self {
        Self {
            kind,
            at,
            items: parts.into_iter().map(Item::Part).collect(),
        }
    }

    /// The turn's text and notes, as one text.
    fn render(&self) -> String {
        let parts: Vec<Part> = self
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Part(part) => Some(part.clone()),
                Item::Call(..) | Item::Answered => None,
            })
            .collect();
        render(&parts)
    }

    /// The turn in [steps](Step): a new one starts at text after calls, and where the model was
    /// called again.
    fn steps(self) -> Vec<Step<'m>> {
        let mut steps: Vec<Step<'m>> = Vec::new();
        for item in self.items {
            match (item, steps.last_mut()) {
                (Item::Answered, Some(step)) if !step.calls.is_empty() => {
                    steps.push(Step::default());
                }
                (Item::Answered, _) => {}
                (Item::Part(part), Some(step)) if step.calls.is_empty() => step.parts.push(part),
                (Item::Part(part), _) => steps.push(Step {
                    parts: vec![part],
                    calls: Vec::new(),
                }),
                (Item::Call(call, result), Some(step)) => step.calls.push((call, result)),
                (Item::Call(call, result), None) => steps.push(Step {
                    parts: Vec::new(),
                    calls: vec![(call, result)],
                }),
            }
        }
        steps
    }
}

/// The conversation of `messages` as alternating turns, user first and assistant last, and
/// what was left out of it.
fn turns(messages: &[RehydrateMessage]) -> (Vec<Turn<'_>>, Flattened) {
    // A call is carried over with its result. One of a tool the model's provider ran itself,
    // answered in the assistant's own row (opencode keeps every result there), is only for that
    // provider to read: its results are opaque, encrypted.
    let mut results: HashMap<&ToolCallId, &ToolResult> = HashMap::new();
    let mut in_reply: HashSet<&ToolCallId> = HashSet::new();
    for m in messages {
        for content in &m.content {
            if let Content::ToolResult(result) = content {
                results.entry(&result.call).or_insert(result);
                if m.role == Role::Assistant {
                    in_reply.insert(&result.call);
                }
            }
        }
    }
    let hosted = |call: &ToolUse| {
        in_reply.contains(&call.id) && PROVIDER_TOOLS.contains(&call.name.as_str())
    };
    let mut flattened = Flattened::default();
    let mut carried: HashSet<&ToolCallId> = HashSet::new();
    let mut turns: Vec<Turn> = Vec::new();
    // Results came back since the model's last row, or its row is of another model call (its
    // turn id up to a step's usage key: Claude Code's message id, opencode's call): its next is
    // another call of the model. (opencode answers calls in the assistant's own rows.)
    let mut answered = false;
    let mut call_of: Option<&str> = None;
    for m in messages {
        let mut user = Vec::new();
        let mut assistant = Vec::new();
        let mut reasoned = false;
        for content in &m.content {
            match content {
                Content::Reasoning(_) | Content::ReasoningSummary { .. } => reasoned = true,
                Content::Summary(summary) if !summary.trim().is_empty() => user.push(Part::Text(
                    format!("[Summary of the earlier conversation]\n{}", summary.trim()),
                )),
                Content::Text(text) if !text.trim().is_empty() => match m.role {
                    Role::User => user.push(Part::Text(text.clone())),
                    Role::Assistant => assistant.push(Item::Part(Part::Text(text.clone()))),
                    // Context the source harness injected, for its own tools and its own model.
                    _ => {}
                },
                Content::ToolUse(call) if m.role == Role::Assistant => {
                    match results.get(&call.id).filter(|_| !hosted(call)) {
                        Some(result) if !call.input.is_null() => {
                            carried.insert(&call.id);
                            assistant.push(Item::Call(call, result));
                        }
                        _ => {
                            flattened.tool_calls += 1;
                            let note = tool_note(&call.name, &call.input);
                            assistant.push(Item::Part(Part::Note(note)));
                        }
                    }
                }
                Content::Error(why) if m.role == Role::Assistant && !why.trim().is_empty() => {
                    let note = format!("[error: {}]", clip(why, NOTE_ERROR));
                    assistant.push(Item::Part(Part::Note(note)));
                }
                Content::Other(raw) if m.role == Role::User => {
                    if let Some(kind) = raw["type"].as_str().filter(|k| is_media(k)) {
                        user.push(Part::Note(format!("[{kind} not carried over]")));
                    }
                }
                _ => {}
            }
        }
        flattened.reasoning += usize::from(reasoned);
        if m.role != Role::Assistant
            && m.content.iter().any(|c| matches!(c, Content::ToolResult(_)))
        {
            answered = true;
        } else if m.role == Role::Assistant {
            let call = m.turn_id.as_deref().and_then(|t| t.split('#').next());
            answered |= call.is_some() && call_of.is_some() && call != call_of;
            call_of = call.or(call_of);
            if answered && !assistant.is_empty() {
                assistant.insert(0, Item::Answered);
                answered = false;
            }
        }
        let user = user.into_iter().map(Item::Part).collect();
        for (kind, items) in [(Kind::User, user), (Kind::Assistant, assistant)] {
            if items.is_empty() {
                continue;
            }
            match turns.last_mut() {
                Some(turn) if turn.kind == kind => turn.items.extend(items),
                _ => turns.push(Turn {
                    kind,
                    at: m.timestamp,
                    items,
                }),
            }
        }
    }
    flattened.tool_results = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|c| matches!(c, Content::ToolResult(r) if !carried.contains(&r.call)))
        .count();
    if let Some(first) = turns.first().filter(|t| t.kind == Kind::Assistant) {
        let at = first.at;
        let note = Part::Note("[the recorded conversation opens with this reply]".into());
        turns.insert(0, Turn::parts(Kind::User, at, vec![note]));
    }
    if let Some(last) = turns.last().filter(|t| t.kind == Kind::User) {
        let at = last.at;
        let note = Part::Note("[the original session ended before a reply to this]".into());
        turns.push(Turn::parts(Kind::Assistant, at, vec![note]));
    }
    (turns, flattened)
}

fn is_media(kind: &str) -> bool {
    matches!(kind, "image" | "input_image" | "document" | "file" | "input_file")
}

/// A new session's id for `harness`, in its own format, minted at `now`.
pub(crate) fn new_session_id(harness: AnyHarness, now: OffsetDateTime) -> String {
    Ids::new(harness, now).session()
}

/// Fresh ids in the target's own formats.
struct Ids {
    target: AnyHarness,
    now: OffsetDateTime,
    seen: HashSet<String>,
}

impl Ids {
    fn new(target: AnyHarness, now: OffsetDateTime) -> Self {
        Self {
            target,
            now,
            seen: HashSet::new(),
        }
    }

    /// A new session's id: a UUID (Claude Code's random, Codex's and Pi's time-ordered), or an
    /// opencode `ses_` id, which sort newest first.
    fn session(&self) -> String {
        match self.target {
            AnyHarness::ClaudeCode(_) => Uuid::new_v4().to_string(),
            AnyHarness::Codex(_) | AnyHarness::Pi(_) => Uuid::now_v7().to_string(),
            AnyHarness::Opencode(_) => {
                opencode::rehydrate::mint_session(self.now, &Uuid::new_v4().to_string())
            }
        }
    }

    /// A new row's id, as its writer takes it: a line `uuid` for Claude Code; for Codex, a
    /// content-keyed `syn-` id, which its writer leaves off the line as Codex itself does for a
    /// message it names no id; an opencode `prt_` part id, in time order (rows of one message,
    /// the marker and the first prompt, are read in id order); a pi entry id (8 hex digits,
    /// unique in the session).
    fn row(&mut self, at: OffsetDateTime) -> String {
        loop {
            let id = match self.target {
                AnyHarness::ClaudeCode(_) => Uuid::new_v4().to_string(),
                AnyHarness::Codex(_) => format!("syn-{}", Uuid::new_v4().as_simple()),
                AnyHarness::Opencode(_) => {
                    opencode::rehydrate::mint_part(at, &Uuid::new_v4().to_string())
                }
                AnyHarness::Pi(_) => Uuid::new_v4().as_simple().to_string()[..8].to_owned(),
            };
            if self.seen.insert(id.clone()) {
                return id;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
