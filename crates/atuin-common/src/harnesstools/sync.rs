//! Keeping a harness's own copy of a session in step with the synced one, the git way.
//!
//! A session keeps one id on every machine. Each machine's native transcript is a branch of it,
//! and the synced rows are the remote: a session that went on elsewhere is caught up here by
//! appending the lines this copy lacks to the transcript, in place, never by merging or by
//! writing the history out again under a new id. This module is the harness side of that:
//!
//! - [`SessionSync::local_tip`]: what this machine's transcript holds, keyed as capture keys it,
//!   and which line the harness would continue from.
//! - [`SessionSync::is_live`]: whether a harness process on this machine may be writing the
//!   session right now.
//! - [`SessionSync::append`]: append synced rows to the transcript so they chain from the lines
//!   they were written after, and make the last of them the line the harness resumes from.
//!
//! Which rows are missing, and on which branch, is the caller's business (the synced store knows
//! the tree); the harness side only reads and writes the native transcript.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::harnesstools::AnyHarness;
use crate::harnesstools::rehydrate::RehydrateMessage;

mod ccode;
mod codex;
pub mod liveness;
mod opencode;
mod pi;

pub use liveness::{Liveness, ProcessInfo, Processes};

/// What this machine's native copy of a session holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTip {
    /// The transcript, or for opencode, the database holding the session.
    pub native_path: PathBuf,
    /// The source id capture gives each line the transcript holds, from the same readers live
    /// capture uses, so they compare with the synced rows' ids. Lines capture keys on their
    /// content (`syn-` ids: a harness's bookkeeping lines without an id of their own) are not
    /// among them: no line hangs from one.
    pub known_source_ids: HashSet<String>,
    /// The line the harness continues from when the session is resumed, by the harness's own
    /// rule (see each harness's [`SessionSync::local_tip`]); `None` for a transcript it would
    /// start empty.
    pub tip_source_id: Option<String>,
    /// The transcript as it was read, for [`SessionSync::append`] to tell that it has not changed
    /// since.
    pub stamp: Stamp,
}

/// A digest of a transcript's content, taken when its tip was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Stamp {
    len: u64,
    digest: u64,
}

impl Stamp {
    /// The stamp of a transcript whose content is `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self {
            len: bytes.len() as u64,
            digest: xxhash_rust::xxh3::xxh3_64(bytes),
        }
    }
}

/// How to append.
#[derive(Debug, Clone, Default)]
pub struct AppendOptions<'a> {
    /// Make the last appended line the one the harness resumes from even where it already
    /// would be; with nothing to append, make [`head`](Self::head) the tip.
    pub make_tip: bool,
    /// With no rows to append: the line already in the transcript to make the tip (a branch
    /// this machine already holds).
    pub head: Option<&'a str>,
    /// Ids that must not be written, besides those the transcript already holds: the caller's
    /// other rows of the session (pi's ids are only 8 hex digits, and two machines can mint the
    /// same one).
    pub taken_ids: Option<&'a HashSet<String>>,
}

/// What [`SessionSync::append`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendOutcome {
    pub native_path: PathBuf,
    /// The source ids of the lines written, in order. Rows with nothing the format can carry
    /// are not written, and neither are rows merged into another (see
    /// [`flatten_uncaptured_calls`](crate::harnesstools::rehydrate::flatten_uncaptured_calls)).
    pub appended: Vec<String>,
    /// The line the harness now resumes from.
    pub tip_source_id: Option<String>,
    /// Whether a marker was written to make that line the tip (Claude Code's `last-prompt`,
    /// pi's `label`).
    pub marked_tip: bool,
}

/// Why sync could not read or write a session.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("the session is open in a running harness on this machine{}", pid_note(*.0))]
    Live(Option<u32>),
    #[error("the session may be open in a running harness on this machine")]
    MaybeLive,
    #[error("the session's transcript changed since it was read")]
    Changed,
    #[error("the session has no transcript on this machine")]
    NotFound,
    #[error("{0} does not hang from anything the transcript holds")]
    Disconnected(String),
    #[error("{0} is already taken")]
    IdTaken(String),
    #[error("{0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    Other(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn pid_note(pid: Option<u32>) -> String {
    pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default()
}

/// The harness side of keeping a session in step across machines (see the module docs).
pub trait SessionSync {
    /// What this machine's native copy of session `id` holds; `None` when it has none.
    #[allow(async_fn_in_trait)]
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError>;

    /// Whether a harness process on this machine may be writing session `id`, last seen working
    /// in `cwd`. Cheap enough for a selection, not for every row of a list.
    #[allow(async_fn_in_trait)]
    async fn is_live(&self, id: &str, cwd: Option<&Path>) -> Liveness;

    /// Append `lines` (a segment of the synced tree, root to head, each hanging from a line the
    /// transcript holds or from one before it) to session `id`'s transcript, read as `base`,
    /// and make the last of them the line the harness resumes from.
    ///
    /// Refuses ([`SyncError::Live`], [`SyncError::MaybeLive`]) while a harness on this machine
    /// may be writing the session, and ([`SyncError::Changed`]) when the transcript is no longer
    /// what `base` read. Re-capturing the transcript afterwards finds only ids among `base`'s and
    /// `lines`', so it pushes nothing new, except a tip marker that has to be an entry of its own
    /// (pi, when a branch already here is made the tip).
    #[allow(async_fn_in_trait)]
    async fn append(
        &self,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError>;
}

impl SessionSync for AnyHarness {
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError> {
        match self {
            Self::ClaudeCode(h) => h.local_tip(id).await,
            Self::Codex(h) => h.local_tip(id).await,
            Self::Opencode(h) => h.local_tip(id).await,
            Self::Pi(h) => h.local_tip(id).await,
        }
    }

    async fn is_live(&self, id: &str, cwd: Option<&Path>) -> Liveness {
        match self {
            Self::ClaudeCode(h) => h.is_live(id, cwd).await,
            Self::Codex(h) => h.is_live(id, cwd).await,
            Self::Opencode(h) => h.is_live(id, cwd).await,
            Self::Pi(h) => h.is_live(id, cwd).await,
        }
    }

    async fn append(
        &self,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        match self {
            Self::ClaudeCode(h) => h.append(id, base, lines, options).await,
            Self::Codex(h) => h.append(id, base, lines, options).await,
            Self::Opencode(h) => h.append(id, base, lines, options).await,
            Self::Pi(h) => h.append(id, base, lines, options).await,
        }
    }
}

/// Refuse to write under a harness that is, or may be, writing the session itself.
fn require_idle(liveness: Liveness) -> Result<(), SyncError> {
    match liveness {
        Liveness::NotLive => Ok(()),
        Liveness::Live { pid } => Err(SyncError::Live(pid)),
        Liveness::Unknown => Err(SyncError::MaybeLive),
    }
}

/// Run `f` on the blocking pool.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, SyncError> + Send + 'static,
) -> Result<T, SyncError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| SyncError::Other(e.to_string()))?
}

/// The non-empty lines of a JSONL transcript, as its readers split it.
fn jsonl_lines(bytes: &[u8]) -> impl DoubleEndedIterator<Item = &[u8]> {
    bytes.split(|b| *b == b'\n').filter(|line| !line.iter().all(u8::is_ascii_whitespace))
}

/// Append `lines` (one JSON value each) to the JSONL transcript at `path`, if it is still what
/// `stamp` read.
///
/// Appended in place, through `O_APPEND`, in one write, then synced: the harnesses append to
/// their transcripts the same way (Claude Code and pi both `appendFile`), and everything that
/// follows a transcript (capture's readers, which hold the file open and resume at a byte
/// offset; the harnesses' own watchers) expects it to only ever grow. A copy renamed over it would
/// swap the inode under them. A crash can leave at most a torn last line after the old content,
/// which both harnesses' loaders skip, and so does capture.
///
/// The check that the file is unchanged reads it through the handle that then writes, so the
/// only window left is between that read and the write; a harness writing the session in it is
/// what [`SessionSync::is_live`] is checked for first.
fn append_jsonl(path: &Path, stamp: Stamp, lines: &[serde_json::Value]) -> Result<(), SyncError> {
    let mut file = std::fs::OpenOptions::new().read(true).append(true).open(path)?;
    let mut current = Vec::new();
    file.read_to_end(&mut current)?;
    if Stamp::of(&current) != stamp {
        return Err(SyncError::Changed);
    }
    let mut out = Vec::new();
    if !current.is_empty() && !current.ends_with(b"\n") {
        // A torn last line stays torn, rather than swallowing the first line written after it.
        out.push(b'\n');
    }
    for line in lines {
        serde_json::to_writer(&mut out, line).map_err(|e| SyncError::Other(e.to_string()))?;
        out.push(b'\n');
    }
    file.write_all(&out)?;
    file.sync_data()?;
    Ok(())
}

/// Check that each row of `lines` hangs from the transcript (`present`) or from a row before it,
/// and that none takes an id already in use. With `root_ok`, the first row may be a root (the
/// transcript holds no line to hang from).
fn check_segment(
    lines: &[RehydrateMessage],
    present: &HashSet<String>,
    taken: Option<&HashSet<String>>,
    root_ok: bool,
) -> Result<(), SyncError> {
    let mut seen: HashSet<&str> = HashSet::new();
    for (n, row) in lines.iter().enumerate() {
        let id = row.source_id.as_str();
        if present.contains(id) || taken.is_some_and(|taken| taken.contains(id)) || !seen.insert(id)
        {
            return Err(SyncError::IdTaken(row.source_id.clone()));
        }
        match row.parent_source_id.as_deref() {
            Some(parent) if present.contains(parent) || seen.contains(parent) => {}
            None if n == 0 && root_ok => {}
            _ => return Err(SyncError::Disconnected(row.source_id.clone())),
        }
    }
    Ok(())
}
