//! The picker's data seam: where session rows, previews and children come from.
//!
//! The picker only talks to a [`SessionSource`]. The real one reads the sidecar database directly
//! (read-only, never through the daemon, so the first frame doesn't wait on it); tests and
//! `--demo` use [`super::fake::FakeSource`].

use std::ops::Range;
use std::path::PathBuf;

use async_trait::async_trait;
use atuin_client::ai_session::{HarnessKind, HarnessSession};
use time::OffsetDateTime;

/// What to list. The picker resolves its filter mode and query tokens into these constraints, so
/// a source never needs to know about modes or the current directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFilter {
    /// Full-text query. Empty lists every session (newest first); otherwise the last term matches
    /// as a prefix (search-as-you-type).
    pub text: String,
    /// Only these harnesses (`h:`). Empty means all.
    pub harnesses: Vec<HarnessKind>,
    /// Only sessions whose model contains this, case-insensitively (`m:`).
    pub model: Option<String>,
    /// Only sessions on this git branch (`b:`, or the branch filter mode).
    pub branch: Option<String>,
    /// Only sessions recorded on the host with this id (the host filter mode).
    pub host_id: Option<String>,
    /// Only sessions recorded on a host whose name starts with this (`@host`).
    pub hostname: Option<String>,
    /// Only sessions whose working directory is this directory or below it (workspace mode).
    pub cwd_prefix: Option<PathBuf>,
    /// Only sessions whose working directory is exactly this (directory mode).
    pub cwd: Option<PathBuf>,
    /// Fold forks (including Claude Code `--resume` forks) into their root session's row.
    pub group_forks: bool,
    /// Count subagent sessions among a root's children. Subagents never get their own row.
    pub include_subagents: bool,
    /// Maximum rows; 0 for unbounded.
    pub limit: usize,
}

/// How a session relates to the root row it is grouped under.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Relation {
    #[default]
    Root,
    Fork,
    Subagent,
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
    pub relation: Relation,
    /// The session title, falling back to the first prompt. Highlights mark query matches.
    pub title: Snippet,
    pub cwd: Option<PathBuf>,
    /// The git repository root the session ran in, if any.
    pub git_root: Option<PathBuf>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub host_id: String,
    pub hostname: String,
    pub started_at: OffsetDateTime,
    /// The newest message in the session or any of its grouped children.
    pub updated_at: OffsetDateTime,
    pub message_count: u64,
    /// Forks and (with `include_subagents`) subagents grouped under this row.
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

    /// The forks and subagents grouped under a root session, newest first.
    async fn children(
        &self,
        session: &HarnessSession,
        include_subagents: bool,
    ) -> eyre::Result<Vec<SessionRow>>;
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
