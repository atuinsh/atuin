//! The picker's data seam: where session rows, previews and children come from.
//!
//! The picker only talks to a [`SessionSource`]. The real one reads the sidecar database directly
//! (read-only, never through the daemon, so the first frame doesn't wait on it); tests and
//! `--demo` use [`super::fake::FakeSource`].

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use async_trait::async_trait;
use atuin_client::ai_session::{HarnessKind, HarnessSession};
use atuin_common::harnesstools::session::Usage;
use time::OffsetDateTime;

/// What to list. The picker resolves its filter mode and query tokens into these constraints, so
/// a source never needs to know about modes or the current directory.
///
/// Mirrors `atuin_client::ai_session::SessionFilter` field for field, plus the query text, the
/// `@host` name and the limit. An absent field is not a filter; every present one must hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFilter {
    /// Full-text query. Empty lists sessions newest first; otherwise the last term matches as a
    /// prefix (search-as-you-type).
    pub text: String,
    /// Recorded on the host with this id (the host filter mode).
    pub host: Option<String>,
    /// Recorded on a host whose [`SessionRow::hostname`] starts with this (`@host`). Sessions
    /// with no recorded host are this host's.
    pub host_name: Option<String>,
    /// Working directory at or under this path (workspace mode).
    pub workspace: Option<PathBuf>,
    /// Working directory exactly this path (directory mode).
    pub directory: Option<PathBuf>,
    /// On this git branch (`b:`, or the branch filter mode).
    pub branch: Option<String>,
    /// From this harness (`h:`).
    pub harness: Option<HarnessKind>,
    /// Model containing this, ignoring case (`m:`).
    pub model: Option<String>,
    /// Only root rows, with forks and subagents grouped under them (counted in
    /// [`SessionRow::children`]; a child's match is its root's).
    pub roots_only: bool,
    /// Maximum rows; 0 for unbounded.
    pub limit: usize,
}

/// How a session relates to the root row it is grouped under.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Relation {
    #[default]
    Root,
    /// Spawned by its parent to do part of its work.
    Subagent,
    /// Continues or branches off its parent's conversation.
    Fork,
    /// Has a parent, but the harness doesn't say which kind of child it is.
    Child,
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
    pub host_id: String,
    /// The host's name, or a short form of its id until (unless) [`SessionSource::host_names`]
    /// knows it.
    pub hostname: String,
    pub started_at: OffsetDateTime,
    /// The newest message in the session or any of its grouped children.
    pub updated_at: OffsetDateTime,
    pub message_count: u64,
    /// Token usage attributed to this session (each model call counted once).
    pub usage: Usage,
    /// Sessions grouped under this row (with [`SessionFilter::roots_only`]).
    pub children: u32,
    /// The best-matching message text, when there is a query. The match may be in a child.
    pub matched: Option<Snippet>,
}

/// The preview for one session: its opening prompt and where it left off. Tool calls and
/// reasoning never appear here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionPreview {
    pub first_prompt: Option<String>,
    pub last_assistant: Option<String>,
    /// When each user and assistant message was sent, oldest first: the session's cadence.
    pub activity: Vec<OffsetDateTime>,
}

/// Where the picker's sessions come from.
#[async_trait]
pub trait SessionSource: Send + Sync {
    /// Root rows matching `filter`: best match first when there's text, newest first otherwise.
    async fn search(&self, filter: &SessionFilter) -> eyre::Result<Vec<SessionRow>>;

    /// Every session whose id is `id` or starts with it, across harnesses (for `atuin ai resume
    /// <id>`).
    async fn find_by_id(&self, id: &str) -> eyre::Result<Vec<SessionRow>>;

    /// The preview text for one session.
    async fn preview(&self, session: &HarnessSession) -> eyre::Result<SessionPreview>;

    /// The sessions grouped under a root, newest first; subagents only with `include_subagents`.
    async fn children(
        &self,
        session: &HarnessSession,
        include_subagents: bool,
    ) -> eyre::Result<Vec<SessionRow>>;

    /// Other hosts' names, by [`SessionRow::host_id`], for rows that came before they were known.
    /// May be slow (it is read once, then cached). Empty when there's nothing to add.
    async fn host_names(&self) -> eyre::Result<HashMap<String, String>> {
        Ok(HashMap::new())
    }
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
