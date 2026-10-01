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
//! Only the conversation, as text:
//!
//! - What the user said, and what the model answered.
//! - **Tool calls, flattened into notes.** The target has other tools than the source, and a
//!   model must never see calls to tools it doesn't have, so each call becomes a short line of
//!   text in its assistant turn saying what was called (see [`tool_note`]): `[ran a shell
//!   command]`, `[edited a file]`, and on what where the input is known: ``[ran `cargo
//!   test`]``, ``[edited `src/store.rs`]``. Capture keeps neither a call's input nor its output
//!   (it syncs the tool's name only; records from before it stopped may carry an input), so a
//!   note never says how a call turned out, and tool results are dropped, whatever they hold.
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
//! one turn. Timestamps keep the original's, nudged forward where needed so every row is later
//! than the one before (opencode orders messages by ids minted from their time).
//!
//! # The marker
//!
//! The session starts with a [`marker_text`] naming the source harness and session, written as
//! the least intrusive line each target has: text the harness itself adds to the user's turn (a
//! Claude Code `isMeta` line, a Codex `developer` message, an opencode `synthetic` part), which
//! the model reads with the first prompt and the user never sees. Pi has no such line: the only
//! lines it sends the model that nobody typed (`custom_message`, summaries) each become a user
//! message of their own, two user messages in a row. So there the marker is the first text block
//! of the first prompt, a short first line the user sees too. Capture reads it back as the
//! session's parent ([`continued_from_message`]).

use std::collections::HashSet;

use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use super::note::{Part, clip, render, tool_note};
use super::rehydrate::{RehydrateMessage, RehydrateSession};
use super::resume::is_plain_name;
use super::{AnyHarness, Harness as _};
use crate::harnesstools::session::{Content, Message, Role, StopReason};

/// How much of an error a note shows.
const NOTE_ERROR: usize = 200;

/// The first line of every marker ends with this.
const MARKER_TAIL: &str = " via atuin.";
const MARKER_HEAD: &str = "Continued from ";

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
    /// Tool calls turned into notes.
    pub tool_calls: usize,
    /// Tool results dropped.
    pub tool_results: usize,
    /// Messages whose reasoning was dropped.
    pub reasoning: usize,
}

impl Flattened {
    /// What was left out, for a status line: `42 tool calls flattened to notes, reasoning
    /// dropped`. Empty when nothing was.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        match self.tool_calls {
            0 => {}
            1 => parts.push("1 tool call flattened to a note".to_owned()),
            n => parts.push(format!("{n} tool calls flattened to notes")),
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
#[must_use]
pub fn label(harness: AnyHarness) -> &'static str {
    match harness {
        AnyHarness::ClaudeCode(_) => "Claude Code",
        AnyHarness::Codex(_) => "Codex",
        AnyHarness::Opencode(_) => "opencode",
        AnyHarness::Pi(_) => "Pi",
    }
}

/// The marker a continuation starts with: its first line names the session it continues
/// (`Continued from <harness> session <id> via atuin.`, the harness by its
/// [name](crate::harnesstools::Harness::name)); the rest tells the model how to read what
/// follows.
#[must_use]
pub fn marker_text(source: AnyHarness, id: &str) -> String {
    format!(
        "{MARKER_HEAD}{} session {id}{MARKER_TAIL}\nThe conversation below was recorded in {} and \
         carried over as text. Its tool calls are shown as notes in [brackets], without their \
         output, and may name tools you don't have: use your own. Check the files before relying \
         on what a note says was done.",
        source.name(),
        label(source),
    )
}

/// The harness and session `text` says it continues, when it is a [marker](marker_text).
#[must_use]
pub fn continued_from(text: &str) -> Option<(AnyHarness, &str)> {
    let first = text.lines().next()?;
    let (name, id) =
        first.strip_prefix(MARKER_HEAD)?.strip_suffix(MARKER_TAIL)?.split_once(" session ")?;
    let harness = AnyHarness::from_name(name).ok()?;
    (is_plain_name(id) && !id.contains(char::is_whitespace)).then_some((harness, id))
}

/// The harness and session a transcript line says its session continues: a [marker](marker_text)
/// the harness put in the user's turn itself, or (pi's) the first block of a prompt with more
/// after it, which typing never makes. How capture links a continuation to the session it
/// continues.
#[must_use]
pub fn continued_from_message<M: Message + ?Sized>(m: &M) -> Option<(AnyHarness, String)> {
    let content = m.content();
    let marker = match (m.role(), content.as_slice()) {
        (Role::Assistant | Role::Tool, _) => return None,
        (Role::User, [Content::Text(first), _, ..]) => first,
        (Role::User, _) => return None,
        (_, content) => content.iter().find_map(|c| match c {
            Content::Text(text) if continued_from(text).is_some() => Some(text),
            _ => None,
        })?,
    };
    continued_from(marker).map(|(harness, id)| (harness, id.to_owned()))
}

/// What continuing `original` in another harness would flatten or drop, whichever harness.
#[must_use]
pub fn flattened(original: &RehydrateSession) -> Flattened {
    turns(&original.messages).1
}

/// `original`, recorded by `source`, as a new session of `target` resuming in `original.cwd`.
#[must_use]
pub fn continue_in(
    source: AnyHarness,
    original: &RehydrateSession,
    target: AnyHarness,
) -> Continuation {
    continue_at(source, original, target, OffsetDateTime::now_utc())
}

/// [`continue_in`], its new session minted at `now`.
#[must_use]
pub fn continue_at(
    source: AnyHarness,
    original: &RehydrateSession,
    target: AnyHarness,
    now: OffsetDateTime,
) -> Continuation {
    let (turns, flattened) = turns(&original.messages);
    let mut ids = Ids::new(target, now);
    let id = ids.session();
    let start = turns.first().map_or(original.started_at, |t| t.at);

    let mut rows = Vec::with_capacity(turns.len() + 1);
    let marker = marker_text(source, &original.id);
    // pi has no line the model reads that the user didn't type but its own user message: the
    // marker is the first block of the first prompt (see the module docs).
    let pi = matches!(target, AnyHarness::Pi(_));
    if !pi {
        rows.push(row(&mut ids, None, start, Role::System, marker.clone()));
    }
    let mut last = start - Duration::milliseconds(i64::from(pi));
    for turn in turns {
        let at = turn.at.max(last + Duration::milliseconds(1));
        last = at;
        let parent = rows.last().map(|r: &RehydrateMessage| r.source_id.clone());
        let role = match turn.kind {
            Kind::User => Role::User,
            Kind::Assistant => Role::Assistant,
        };
        let mut message = row(&mut ids, parent, at, role, turn.render());
        if turn.kind == Kind::Assistant {
            message.stop_reason = Some(StopReason::EndTurn);
        }
        if pi && rows.is_empty() {
            message.content.insert(0, Content::Text(marker.clone()));
        }
        rows.push(message);
    }
    // Only Claude Code and Pi keep a tree; opencode reads a row's parent as the message it
    // answers, which it finds on its own.
    if !matches!(target, AnyHarness::ClaudeCode(_) | AnyHarness::Pi(_)) {
        for message in &mut rows {
            message.parent_source_id = None;
        }
    }

    Continuation {
        session: RehydrateSession {
            id,
            title: original.title.clone(),
            cwd: original.cwd.clone(),
            original_cwd: Some(original.cwd.clone()),
            git_branch: original.git_branch.clone(),
            model: None,
            started_at: start,
            messages: rows,
        },
        flattened,
    }
}

fn row(
    ids: &mut Ids,
    parent: Option<String>,
    at: OffsetDateTime,
    role: Role,
    text: String,
) -> RehydrateMessage {
    RehydrateMessage {
        source_id: ids.row(at),
        parent_source_id: parent,
        timestamp: at,
        role,
        content: vec![Content::Text(text)],
        model: None,
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    User,
    Assistant,
}

/// One side's turn: the rows of one role in a row, merged.
#[derive(Debug)]
struct Turn {
    kind: Kind,
    at: OffsetDateTime,
    parts: Vec<Part>,
}

impl Turn {
    fn render(&self) -> String {
        render(&self.parts)
    }
}

/// The conversation of `messages` as alternating turns, user first and assistant last, and
/// what was left out of it.
fn turns(messages: &[RehydrateMessage]) -> (Vec<Turn>, Flattened) {
    let mut flattened = Flattened::default();
    let mut turns: Vec<Turn> = Vec::new();
    for m in messages {
        let mut user = Vec::new();
        let mut assistant = Vec::new();
        let mut reasoned = false;
        for content in &m.content {
            match content {
                Content::ToolResult(_) => flattened.tool_results += 1,
                Content::Reasoning(_) | Content::ReasoningSummary { .. } => reasoned = true,
                Content::Summary(summary) if !summary.trim().is_empty() => user.push(Part::Text(
                    format!("[Summary of the earlier conversation]\n{}", summary.trim()),
                )),
                Content::Text(text) if !text.trim().is_empty() => match m.role {
                    Role::User => user.push(Part::Text(text.clone())),
                    Role::Assistant => assistant.push(Part::Text(text.clone())),
                    // Context the source harness injected, for its own tools and its own model.
                    _ => {}
                },
                Content::ToolUse(call) if m.role == Role::Assistant => {
                    flattened.tool_calls += 1;
                    assistant.push(Part::Note(tool_note(&call.name, &call.input)));
                }
                Content::Error(why) if m.role == Role::Assistant && !why.trim().is_empty() => {
                    assistant.push(Part::Note(format!("[error: {}]", clip(why, NOTE_ERROR))));
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
        for (kind, parts) in [(Kind::User, user), (Kind::Assistant, assistant)] {
            if parts.is_empty() {
                continue;
            }
            match turns.last_mut() {
                Some(turn) if turn.kind == kind => turn.parts.extend(parts),
                _ => turns.push(Turn {
                    kind,
                    at: m.timestamp,
                    parts,
                }),
            }
        }
    }
    if let Some(first) = turns.first().filter(|t| t.kind == Kind::Assistant) {
        let at = first.at;
        turns.insert(0, Turn {
            kind: Kind::User,
            at,
            parts: vec![Part::Note("[the recorded conversation opens with this reply]".into())],
        });
    }
    if let Some(last) = turns.last().filter(|t| t.kind == Kind::User) {
        let at = last.at;
        turns.push(Turn {
            kind: Kind::Assistant,
            at,
            parts: vec![Part::Note("[the original session ended before a reply to this]".into())],
        });
    }
    (turns, flattened)
}

fn is_media(kind: &str) -> bool {
    matches!(kind, "image" | "input_image" | "document" | "file")
}

/// Fresh ids in the target's own formats.
struct Ids {
    target: AnyHarness,
    now: OffsetDateTime,
    /// opencode's ids: a counter under the millisecond, as its own.
    counter: u64,
    seen: HashSet<String>,
}

impl Ids {
    fn new(target: AnyHarness, now: OffsetDateTime) -> Self {
        Self {
            target,
            now,
            counter: 0,
            seen: HashSet::new(),
        }
    }

    /// A new session's id: a UUID (Claude Code's random, Codex's and Pi's time-ordered), or an
    /// opencode `ses_` id, which sort newest first.
    fn session(&mut self) -> String {
        match self.target {
            AnyHarness::ClaudeCode(_) => Uuid::new_v4().to_string(),
            AnyHarness::Codex(_) | AnyHarness::Pi(_) => Uuid::now_v7().to_string(),
            AnyHarness::Opencode(_) => {
                let at = millis(self.now);
                self.opencode("ses", at, true)
            }
        }
    }

    /// A new row's id, as its writer takes it: a line `uuid` for Claude Code; for Codex, a
    /// content-keyed `syn-` id, which its writer leaves off the line as Codex itself does for a
    /// message it names no id; an opencode `prt_` part id, in time order; a pi entry id (8 hex
    /// digits, unique in the session).
    fn row(&mut self, at: OffsetDateTime) -> String {
        loop {
            let id = match self.target {
                AnyHarness::ClaudeCode(_) => Uuid::new_v4().to_string(),
                AnyHarness::Codex(_) => format!("syn-{}", Uuid::new_v4().as_simple()),
                AnyHarness::Opencode(_) => self.opencode("prt", millis(at), false),
                AnyHarness::Pi(_) => Uuid::new_v4().as_simple().to_string()[..8].to_owned(),
            };
            if self.seen.insert(id.clone()) {
                return id;
            }
        }
    }

    /// An opencode id (`Identifier.create`): the prefix, 6 bytes of the time in milliseconds
    /// times 4096 plus a counter (inverted for a descending id), then 14 random base-62
    /// characters.
    fn opencode(&mut self, prefix: &str, at: i64, descending: bool) -> String {
        const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        self.counter += 1;
        let time = u64::try_from(at).unwrap_or_default().wrapping_mul(0x1000) + self.counter;
        let time = if descending {
            !time
        } else {
            time
        } & 0xffff_ffff_ffff;
        let mut random = Uuid::new_v4().as_u128();
        let tail: String = (0..14)
            .map(|_| {
                let c = BASE62[usize::try_from(random % 62).unwrap_or_default()];
                random /= 62;
                char::from(c)
            })
            .collect();
        format!("{prefix}_{time:012x}{tail}")
    }
}

fn millis(at: OffsetDateTime) -> i64 {
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or_default()
}

#[cfg(test)]
mod tests;
