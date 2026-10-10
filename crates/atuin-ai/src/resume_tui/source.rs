//! The picker's data seam: where session rows, previews and children come from.
//!
//! The picker only talks to a [`SessionSource`]. The real one reads the sidecar database directly
//! (read-only, never through the daemon, so the first frame doesn't wait on it); tests use
//! `super::fake::FakeSource`.

use std::ops::Range;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use atuin_client::ai_session::{
    Analysis, AtuinSessionId, HarnessKind, HarnessSession, SessionFilter as DbFilter,
};
use atuin_common::harnesstools::rehydrate::RehydrateSession;
use atuin_common::harnesstools::session::Usage;
use time::OffsetDateTime;

/// What to list. The picker resolves its filter mode and query tokens into the database's filter,
/// so a source never needs to know about modes or the current directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFilter {
    /// Full-text query. Empty lists sessions newest first; otherwise the last term matches as a
    /// prefix (search-as-you-type).
    pub text: String,
    /// Which sessions: the host (the host filter mode, with this host's unrecorded sessions),
    /// workspace or directory (those modes), branch (`b:`, or the branch mode), agent (`agent:`)
    /// and model (`m:`). With [`DbFilter::roots_only`], forks and subagents are grouped under
    /// their roots (counted in [`SessionRow::children`]; a child's match is its root's).
    pub db: DbFilter,
    /// Maximum rows; 0 for unbounded.
    pub limit: usize,
}

/// How a session relates to the root row it is grouped under (see
/// [`atuin_client::ai_session::Session::inferred_parent_kind`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Relation {
    #[default]
    Root,
    /// Spawned by its parent to do part of its work. Also a child of a kind this build does not
    /// know (a newer build's).
    Subagent,
    /// Branches off its parent's conversation, or copies it.
    Fork,
    /// Carries its parent's conversation on under a new session (maybe in another harness).
    Continuation,
}

impl Relation {
    /// Whether the picker shows sessions of this kind: roots, forks and continuations (which
    /// resume like any session). Subagents never resume, so they are left out everywhere.
    pub fn is_listed(self) -> bool {
        matches!(self, Self::Root | Self::Fork | Self::Continuation)
    }
}

/// Text with highlighted byte ranges (the query's matches).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snippet {
    pub text: String,
    /// Byte ranges into `text`, sorted and non-overlapping.
    pub highlights: Vec<Range<usize>>,
}

impl Snippet {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            highlights: Vec::new(),
        }
    }
}

/// One session, as a row in the picker (a root, with its children folded in) or as a child in the
/// Inspect tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRow {
    pub handle: HarnessSession,
    pub atuin_id: AtuinSessionId,
    /// The session this one was forked or spawned from.
    pub parent: Option<HarnessSession>,
    pub relation: Relation,
    /// The session title, falling back to the first prompt. Highlights mark query matches.
    pub title: Snippet,
    pub cwd: Option<PathBuf>,
    /// The git repository root the session ran in, if any.
    pub git_root: Option<PathBuf>,
    pub branch: Option<String>,
    pub model: Option<String>,
    /// The host the session was recorded on, by id (a UUID's simple form; see
    /// [`super::simple_host_id`]).
    pub host_id: String,
    pub started_at: OffsetDateTime,
    /// The newest message in the session or any of its grouped children.
    pub updated_at: OffsetDateTime,
    /// The newest message in the session itself, its grouped children not counted: whether it is
    /// the one still running.
    pub active_at: OffsetDateTime,
    /// How many messages the session holds: every row stored of it
    /// ([`atuin_client::ai_session::Session::message_count`]).
    pub messages: u64,
    /// Token usage attributed to this session (each model call counted once).
    pub usage: Usage,
    /// Sessions grouped under this row (with [`DbFilter::roots_only`]), subagents included:
    /// whether there are forks to ask [`SessionSource::children`] for. Never shown as it is.
    pub children: u32,
    /// The best-matching message text, when there is a query. The match may be in a child.
    pub matched: Option<Snippet>,
    /// Where that message is in the session's own messages, in transcript order, when it is the
    /// session's own (not a child's): what the reader opens at.
    pub matched_at: Option<u64>,
}

/// The title a session shows: its own, or `untitled` when it has none (and no prompt to take one
/// from).
pub fn shown_title(row: &SessionRow) -> &str {
    if is_untitled(row) {
        "untitled"
    } else {
        row.title.text.trim()
    }
}

/// Whether `row` has no title to show (see [`shown_title`]).
pub fn is_untitled(row: &SessionRow) -> bool {
    row.title.text.trim().is_empty()
}

/// The preview for one session: its opening prompt and where it left off. Tool calls and
/// reasoning never appear here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionPreview {
    pub first_prompt: Option<String>,
    pub last_assistant: Option<String>,
}

/// One piece of a session's conversation, as the reader shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptEntry {
    /// What the user asked.
    Prompt(String),
    /// What the agent answered.
    Reply(String),
    /// A model-written summary standing in for earlier conversation.
    Summary(String),
    /// The tools the agent called between two pieces of text: each name, and how many times in a
    /// row.
    Tools(Vec<(String, usize)>),
}

/// A session's conversation, for the reader: its entries, and the message each came from (its
/// place in the session's messages, in the order a search's match is counted).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Transcript {
    pub entries: Vec<TranscriptEntry>,
    pub messages: Vec<u64>,
    /// How many messages the session had when it was read, as [`SessionRow::messages`] counts
    /// them: a session found with more is read again.
    pub message_count: u64,
}

impl Transcript {
    /// The entry message `index` is in: the last to start at it or before (a run of tool calls
    /// takes in the calls of the messages after its first).
    pub fn entry_at(&self, index: u64) -> Option<usize> {
        self.messages.iter().rposition(|m| *m <= index)
    }
}

/// Builds a session's [`Transcript`] from its messages in order (each started with [`Self::at`]),
/// folding each run of tool calls into one entry.
#[derive(Default)]
pub struct TranscriptBuilder {
    transcript: Transcript,
    at: u64,
}

impl TranscriptBuilder {
    /// What follows comes from the message at `index`.
    pub fn at(&mut self, index: u64) {
        self.at = index;
    }

    /// The session had `n` messages when read (see [`Transcript::message_count`]).
    pub fn counted(&mut self, n: u64) {
        self.transcript.message_count = n;
    }

    fn push(&mut self, entry: TranscriptEntry) {
        self.transcript.entries.push(entry);
        self.transcript.messages.push(self.at);
    }

    /// `text` as an entry of the kind `entry` makes, unless it's blank.
    fn text(&mut self, text: &str, entry: fn(String) -> TranscriptEntry) {
        if !text.trim().is_empty() {
            self.push(entry(text.to_owned()));
        }
    }

    pub fn prompt(&mut self, text: &str) {
        self.text(text, TranscriptEntry::Prompt);
    }

    pub fn reply(&mut self, text: &str) {
        self.text(text, TranscriptEntry::Reply);
    }

    pub fn summary(&mut self, text: &str) {
        self.text(text, TranscriptEntry::Summary);
    }

    pub fn tool(&mut self, name: &str) {
        if let Some(TranscriptEntry::Tools(tools)) = self.transcript.entries.last_mut() {
            match tools.last_mut() {
                Some((last, n)) if last == name => *n += 1,
                _ => tools.push((name.to_owned(), 1)),
            }
        } else {
            self.push(TranscriptEntry::Tools(vec![(name.to_owned(), 1)]));
        }
    }

    pub fn finish(self) -> Transcript {
        self.transcript
    }
}

/// Where the picker's sessions come from.
#[async_trait]
pub trait SessionSource: Send + Sync {
    /// Root rows matching `filter`: best match first when there's text, newest first otherwise.
    /// Only [listed](Relation::is_listed) sessions; grouped, a match in a subagent is its
    /// root's.
    async fn search(&self, filter: &SessionFilter) -> eyre::Result<Vec<SessionRow>>;

    /// Every session whose id is `id` or starts with it, across harnesses (for `atuin ai resume
    /// <id>`).
    async fn find_by_id(&self, id: &str) -> eyre::Result<Vec<SessionRow>>;

    /// The preview text for one session.
    async fn preview(&self, session: &HarnessSession) -> eyre::Result<SessionPreview>;

    /// The forks grouped under a root, newest first: the [listed](Relation::is_listed) sessions
    /// among its children, never its subagents.
    async fn children(&self, session: &HarnessSession) -> eyre::Result<Vec<SessionRow>>;

    /// The session's whole conversation, oldest first, for the reader. Reasoning and tool
    /// results never appear; tool calls are folded into [`TranscriptEntry::Tools`].
    async fn transcript(&self, _session: &HarnessSession) -> eyre::Result<Transcript> {
        Ok(Transcript::default())
    }

    /// Session `session` with every message it holds, as its harness can write it back out to be
    /// resumed in `cwd` (see [`super::resumer::Resumer::restore`]).
    async fn rehydrate(
        &self,
        _session: &HarnessSession,
        _cwd: &Path,
    ) -> eyre::Result<RehydrateSession> {
        eyre::bail!("this source can't restore sessions")
    }

    /// The branches of `session` as its synced rows hold them now: its heads, and the rows on
    /// each (see [`super::catchup`]). `None` when the source doesn't know them.
    async fn analyse(&self, _session: &HarnessSession) -> eyre::Result<Option<Analysis>> {
        Ok(None)
    }
}

/// How a session's host shows: `this machine` when it is `here`, and another by a short form of
/// its id, `@3f9a12bc` (sessions record only the host's id).
pub fn host_label(host_id: &str, here: &str) -> String {
    if host_id == here {
        "this machine".to_owned()
    } else {
        format!("@{}", short_host_id(host_id))
    }
}

/// A host id's last eight characters: a UUIDv7's random part, where its first are a timestamp
/// that hosts set up around the same time share.
pub fn short_host_id(host_id: &str) -> &str {
    let start = host_id.char_indices().rev().nth(7).map_or(0, |(i, _)| i);
    &host_id[start..]
}

/// The two-letter harness badge shown in each row.
pub fn harness_badge(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::ClaudeCode => "CC",
        HarnessKind::Codex => "CX",
        HarnessKind::Opencode => "OC",
        HarnessKind::Pi => "PI",
        HarnessKind::Copilot => "CP",
        HarnessKind::Unknown => "??",
    }
}

/// The harness as `atuin ai resume --in` names it; `None` for one that can't be continued in.
pub fn harness_arg(harness: HarnessKind) -> Option<&'static str> {
    match harness {
        HarnessKind::ClaudeCode => Some("claude"),
        HarnessKind::Codex => Some("codex"),
        HarnessKind::Opencode => Some("opencode"),
        HarnessKind::Pi => Some("pi"),
        HarnessKind::Copilot | HarnessKind::Unknown => None,
    }
}

/// The harness's display name.
pub fn harness_label(harness: HarnessKind) -> &'static str {
    match harness {
        HarnessKind::ClaudeCode => "Claude Code",
        HarnessKind::Codex => "Codex",
        HarnessKind::Opencode => "opencode",
        HarnessKind::Pi => "Pi",
        HarnessKind::Copilot => "Copilot",
        HarnessKind::Unknown => "unknown",
    }
}
