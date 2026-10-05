//! Fast-forwarding a harness's own copy of a session to the synced one.
//!
//! A session keeps one id on every machine, and each machine's native transcript is a line of
//! it. When this machine's copy is behind on the same line (the session went on elsewhere from
//! where this copy ends), catching it up appends the synced rows it lacks, in place: the
//! transcript keeps its native id, and the harness resumes from the last row appended. Anything
//! else (a copy that went on here, or that is on another line) is never rewritten: the caller
//! offers to fork instead. This module is the harness side of that:
//!
//! - [`SessionSync::local_tip`]: what this machine's transcript holds, keyed as capture keys it,
//!   and which line the harness would continue from.
//! - [`SessionSync::is_live`]: whether a harness process on this machine may be writing it.
//! - [`SessionSync::append`]: append rows that continue the transcript from its tip, refusing
//!   ([`SyncError::Unsupported`]) whatever is not a clean fast-forward.
//!
//! Which rows are missing is the caller's business (the synced store knows the tree); the
//! harness side only reads and writes the native transcript.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::harnesstools::AnyHarness;
use crate::harnesstools::rehydrate::{MERGED_FIELD, RehydrateMessage};

mod ccode;
mod codex;
pub mod liveness;
mod opencode;
mod pi;

pub use liveness::{Dirs, Liveness, ProcessInfo, Processes, RECENT, Seen, TableEntry};

/// What this machine's native copy of a session holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTip {
    /// The transcript, or for opencode, the database holding the session.
    pub native_path: PathBuf,
    /// The source id capture gives each row of the transcript, from the same readers capture
    /// uses, so they compare with the synced rows' ids; and the synced rows a writer merged into
    /// its lines ([`merged`](Self::merged)).
    pub known_source_ids: HashSet<String>,
    /// The synced rows the transcript holds without a line of their own, each with the line it
    /// went into, as the writer that wrote the transcript recorded them ([`MERGED_FIELD`]).
    pub merged: HashMap<String, String>,
    /// The row the harness continues from when the session is resumed, by the harness's own rule
    /// (see each harness's module); `None` for a transcript it would start empty.
    pub tip_source_id: Option<String>,
    /// The transcript as it was read, for [`SessionSync::append`] to tell it has not changed.
    pub stamp: Stamp,
    /// When the transcript last changed, as it was read (the file's modification time; for
    /// opencode, the latest update opencode recorded of the session's rows); `None` when that
    /// can't be read. While it is recent, any process of the harness may be writing the session,
    /// wherever it works ([`liveness::changed_lately`]).
    pub modified: Option<SystemTime>,
    /// The directory the transcript records the session working in (Claude Code: its last
    /// line's; pi: its header's; Codex: its newest segment's `session_meta`; opencode: the
    /// session's), where a harness here resumes it; `None` when it records none. An append checks
    /// for a harness here as well as where its caller last saw the session, never instead of it
    /// ([`AppendOptions::cwd`]).
    pub cwd: Option<PathBuf>,
}

impl LocalTip {
    /// Whether the transcript holds synced row `id` as the tip, or merged into the tip's line.
    #[must_use]
    pub fn is_tip(&self, id: &str) -> bool {
        let Some(tip) = self.tip_source_id.as_deref() else {
            return false;
        };
        id == tip || self.merged.get(id).is_some_and(|into| into == tip)
    }

    /// The session as [`liveness::agent_running`] takes it, for an append whose caller last saw
    /// it working in `cwd`, and named on a command line by `names`: changing while this
    /// transcript changed lately.
    ///
    /// It may be working where this transcript records ([`cwd`](Self::cwd)) or in `cwd`, so a
    /// process of the harness in either counts; the append's check is never narrower than its
    /// caller's. When either is not known (`None`, or the recorded directory is no longer
    /// there), it may be working anywhere ([`Dirs::Any`]).
    fn seen(&self, cwd: Option<&Path>, names: Vec<String>) -> Seen {
        let recorded = self.cwd.as_deref().filter(|dir| dir.is_dir());
        let dirs = match (recorded, cwd) {
            (Some(recorded), Some(cwd)) if recorded == cwd => Dirs::Within(vec![cwd.to_path_buf()]),
            (Some(recorded), Some(cwd)) => {
                Dirs::Within(vec![recorded.to_path_buf(), cwd.to_path_buf()])
            }
            _ => Dirs::Any,
        };
        Seen {
            dirs,
            names,
            changing: liveness::changed_lately(self.modified),
        }
    }
}

/// When the file at `path` was last modified; `None` when that can't be read.
fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
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
    /// Ids that must not be written, besides those the transcript already holds: the caller's
    /// other rows of the session (pi's ids are only 8 hex digits, and two machines can mint the
    /// same one).
    pub taken_ids: Option<&'a HashSet<String>>,
    /// The directory the session was last seen working in, as [`SessionSync::is_live`] takes it:
    /// the append is refused while a harness process here may be writing the session. A process
    /// in the directory the transcript records ([`LocalTip::cwd`]) counts too; with either not
    /// known, a process anywhere does.
    pub cwd: Option<&'a Path>,
}

/// What [`SessionSync::append`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendOutcome {
    /// The transcript written to.
    pub native_path: PathBuf,
    /// The source ids of the rows written as lines of their own, in order. Rows with nothing the
    /// format can carry are not written, and neither are rows merged into another (see
    /// [`flatten_uncaptured_calls`](crate::harnesstools::rehydrate::flatten_uncaptured_calls)).
    pub appended: Vec<String>,
    /// The row the harness now resumes from: the last one appended.
    pub tip_source_id: Option<String>,
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
    #[error("{0} is already taken")]
    IdTaken(String),
    /// Not a clean fast-forward: the caller forks instead.
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

/// The harness side of fast-forwarding a session (see the module docs).
pub trait SessionSync {
    /// What this machine's native copy of session `id` holds; `None` when it has none.
    #[allow(async_fn_in_trait)]
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError>;

    /// Whether a harness process on this machine may be writing session `id`, last seen working
    /// in `cwd`. [`Liveness::Unknown`] is to be taken as live.
    #[allow(async_fn_in_trait)]
    async fn is_live(&self, id: &str, cwd: Option<&Path>) -> Liveness;

    /// Append `lines` (root to head, the first hanging from the tip `base` read, each after it
    /// from the tip or a row before it) to session `id`'s transcript, so the harness resumes from
    /// the last of them.
    ///
    /// Refuses ([`SyncError::Live`], [`SyncError::MaybeLive`]) while a harness here may be
    /// writing the session, ([`SyncError::Changed`]) when the transcript is no longer what `base`
    /// read, and ([`SyncError::Unsupported`]) when the rows are no clean fast-forward of it.
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
const fn require_idle(liveness: Liveness) -> Result<(), SyncError> {
    match liveness {
        Liveness::NotLive => Ok(()),
        Liveness::Live { pid } => Err(SyncError::Live(pid)),
        Liveness::Unknown => Err(SyncError::MaybeLive),
    }
}

/// `f`, a liveness read on the blocking pool; [`Liveness::Unknown`] when it can't be run.
async fn blocking_liveness(f: impl FnOnce() -> Liveness + Send + 'static) -> Liveness {
    tokio::task::spawn_blocking(f).await.unwrap_or(Liveness::Unknown)
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

/// The rows the lines of JSONL transcript `bytes` record as merged into them ([`MERGED_FIELD`]),
/// but for any that is a line of its own among `lines`.
fn merged_in(bytes: &[u8], lines: &HashSet<String>) -> HashMap<String, String> {
    let marker = MERGED_FIELD.as_bytes();
    jsonl_lines(bytes)
        .filter(|line| memchr::memmem::find(line, marker).is_some())
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .flat_map(|line| {
            let merged = line.get(MERGED_FIELD).and_then(serde_json::Value::as_object).cloned();
            merged.into_iter().flatten()
        })
        .filter_map(|(row, into)| Some((row, into.as_str()?.to_owned())))
        .filter(|(row, _)| !lines.contains(row))
        .collect()
}

/// `known`, with the rows merged into its lines: a [`LocalTip`]'s ids and merged rows.
fn with_merged(
    mut known: HashSet<String>,
    bytes: &[u8],
) -> (HashSet<String>, HashMap<String, String>) {
    let merged = merged_in(bytes, &known);
    known.extend(merged.keys().cloned());
    (known, merged)
}

/// Append `lines` (one JSON value each) to the JSONL transcript at `path`, if it is still what
/// `stamp` read.
///
/// Appended in place, through `O_APPEND`, in one write, then synced: the harnesses append to
/// their transcripts the same way, and everything that follows a transcript (capture's readers,
/// which resume at a byte offset; the harnesses' own watchers) expects it to only ever grow. A
/// crash can leave at most a torn last line, which the harnesses' loaders skip, and so does
/// capture. The check that the file is unchanged reads it through the handle that then writes.
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

/// How a harness's rows say what they follow on from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Links {
    /// Each names its parent row (Claude Code, pi).
    Parents,
    /// Each names its parent row, or none and follows the one before it (Codex, whose rows
    /// captured before capture linked them name none).
    Chained,
    /// None names a row: they follow on in order (opencode).
    Linear,
}

/// Check that `lines` fast-forward the transcript `base` read: none takes an id already in use,
/// and each hangs from the tip (or a row merged into its line) or from a row before it, as far
/// as its `links` say.
fn check_segment(
    lines: &[RehydrateMessage],
    base: &LocalTip,
    taken: Option<&HashSet<String>>,
    links: Links,
) -> Result<(), SyncError> {
    if base.tip_source_id.is_none() && links != Links::Linear {
        return Err(SyncError::Unsupported("the transcript here has no line to continue from"));
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for row in lines {
        let id = row.source_id.as_str();
        let follows = match (links, row.parent_source_id.as_deref()) {
            (Links::Linear, _) | (Links::Chained, None) => true,
            (_, Some(parent)) => seen.contains(parent) || base.is_tip(parent),
            (Links::Parents, None) => false,
        };
        if !follows {
            return Err(SyncError::Unsupported(
                "the rows don't continue the transcript from where it ends here",
            ));
        }
        if base.known_source_ids.contains(id)
            || taken.is_some_and(|taken| taken.contains(id))
            || !seen.insert(id)
        {
            return Err(SyncError::IdTaken(row.source_id.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
