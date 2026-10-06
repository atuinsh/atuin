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
//! - [`SessionSync::replace`]: switch the transcript to another branch, in place (same id, same
//!   file), when the user chose to: the lines it shares with that branch are kept as they are,
//!   the lines it went on with here are dropped, and the branch's rows past where they part are
//!   appended after them as a fast-forward appends them. The transcript as it was is kept as a
//!   backup first, where no harness looks. Only once the caller knows every row of the copy is
//!   synced, so the branch it was on stays in atuin too; refused like an append while a harness
//!   here may have it open, and when it changed since it was read.
//!
//! Which rows are missing is the caller's business (the synced store knows the tree); the
//! harness side only reads and writes the native transcript.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::harnesstools::AnyHarness;
use crate::harnesstools::rehydrate::{MERGED_FIELD, RehydrateMessage};
use crate::harnesstools::session::synthetic::{SYNTHETIC, content_hash, synthetic_id};
use crate::harnesstools::session::{Message, SessionId};

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
    /// Why [`SessionSync::replace`] would refuse to switch this copy to any branch at all, as far
    /// as reading it tells (a Codex thread kept in several rollouts, a pi session file from
    /// before ids, opencode's database), so a switch is never offered for it; `None` when it may
    /// be switched.
    pub unswitchable: Option<&'static str>,
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

/// What [`SessionSync::replace`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceOutcome {
    /// The transcript written to.
    pub native_path: PathBuf,
    /// The source ids of the branch's rows written after the lines kept, in order, as
    /// [`AppendOutcome::appended`] has them.
    pub appended: Vec<String>,
    /// The row the harness now resumes from: the branch's head, or the line it was merged into.
    pub tip_source_id: Option<String>,
    /// Where the transcript as it was before the switch is kept, whole: a new file in the
    /// backups directory the caller named, which no harness lists or loads.
    pub backup: PathBuf,
    /// What went wrong once the transcript was replaced: the switch happened (the transcript
    /// holds the branch, and the backup is kept), but a crash before the disk caught up could
    /// still bring back the copy as it was (its directory could not be synced).
    pub warning: Option<String>,
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

    /// Switch session `id`'s transcript, read as `base`, to the branch `branch` (its rows, root
    /// to head, along the tree: each the parent of the next; with the rows beside the tree that
    /// go with it, as sync places them for a restore: titles, pi's prompts from before ids), in
    /// place: the same native id and file, which the harness then resumes from the branch's
    /// head (or the last row written after it).
    ///
    /// The transcript keeps every line of the history it shares with the branch as it is, with
    /// whatever only it holds (tool input and output, images), and the lines that name the
    /// session (its header, its titles); only the lines that went on from where the branch
    /// leaves it (the last of the branch's first rows it holds) are dropped. The branch's rows
    /// past that are written after them as [`Self::append`] writes rows, from the synced rows
    /// (so with what capture keeps of them, as a restore). The caller must know sync holds every
    /// row of the transcript first, so the lines dropped stay in atuin; their native content is
    /// kept too: the transcript as it was is written whole to a new file in `backups` before
    /// anything replaces it ([`ReplaceOutcome::backup`]).
    ///
    /// Written whole to a temporary file beside the transcript and renamed over it
    /// ([`crate::fs::replace`]). Refuses as [`Self::append`] does while a harness here may be
    /// writing the session, and when the transcript is no longer what `base` read (checked
    /// again just before the rename); ([`SyncError::Unsupported`]) when the harness's store
    /// can't be switched so (opencode's database, a Codex thread kept in several rollouts:
    /// [`LocalTip::unswitchable`] says so up front), the transcript shares no row with the
    /// branch, or would not resume from its head. Once the new file is in place, nothing fails
    /// the switch: what went wrong after (its directory not synced) is
    /// [`ReplaceOutcome::warning`], and the backup is kept.
    #[allow(async_fn_in_trait)]
    async fn replace(
        &self,
        id: &str,
        base: &LocalTip,
        branch: &[RehydrateMessage],
        options: &AppendOptions<'_>,
        backups: &Path,
    ) -> Result<ReplaceOutcome, SyncError>;
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

    async fn replace(
        &self,
        id: &str,
        base: &LocalTip,
        branch: &[RehydrateMessage],
        options: &AppendOptions<'_>,
        backups: &Path,
    ) -> Result<ReplaceOutcome, SyncError> {
        match self {
            Self::ClaudeCode(h) => h.replace(id, base, branch, options, backups).await,
            Self::Codex(h) => h.replace(id, base, branch, options, backups).await,
            Self::Opencode(h) => h.replace(id, base, branch, options, backups).await,
            Self::Pi(h) => h.replace(id, base, branch, options, backups).await,
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

/// Replace the JSONL transcript at `path`, holding `current` as `stamp` read it, with `bytes`, if
/// it is still what `stamp` read (see [`crate::fs::replace`]). `current` is first written to a new
/// file in `backups` ([`backup_path`]), whose path is returned; a replace that doesn't happen
/// leaves none. One that did happen keeps it whatever fails after, and says what did (a warning).
fn replace_jsonl(
    path: &Path,
    stamp: Stamp,
    current: &[u8],
    bytes: &[u8],
    backups: &Path,
) -> Result<(PathBuf, Option<String>), SyncError> {
    replace_jsonl_with(path, stamp, current, bytes, backups, |path, bytes, still| {
        crate::fs::replace(path, bytes, still)
    })
}

/// [`replace_jsonl`], replacing the transcript with `replace` ([`crate::fs::replace`]).
fn replace_jsonl_with(
    path: &Path,
    stamp: Stamp,
    current: &[u8],
    bytes: &[u8],
    backups: &Path,
    replace: impl FnOnce(&Path, &[u8], &dyn Fn(&[u8]) -> bool) -> Result<(), crate::fs::ReplaceError>,
) -> Result<(PathBuf, Option<String>), SyncError> {
    if Stamp::of(current) != stamp {
        return Err(SyncError::Changed);
    }
    let backup = keep_backup(path, current, backups, time::OffsetDateTime::now_utc())?;
    match replace(path, bytes, &|now| Stamp::of(now) == stamp) {
        Ok(()) => Ok((backup, None)),
        // The transcript holds the branch now: the backup is all that is left of the copy as it
        // was, and is kept.
        Err(e @ crate::fs::ReplaceError::Unsynced(_)) => {
            tracing::warn!(path = %path.display(), backup = %backup.display(), %e,
                "switched a transcript, but couldn't sync its directory");
            Ok((backup, Some(format!("{e}: a crash now could bring back the copy as it was"))))
        }
        // The transcript is as it was: the copy of it backs nothing up.
        Err(crate::fs::ReplaceError::NotReplaced(e)) => {
            let _ = std::fs::remove_file(&backup);
            Err(if e.kind() == std::io::ErrorKind::Interrupted {
                SyncError::Changed
            } else {
                e.into()
            })
        }
    }
}

/// Write `bytes`, the transcript at `path` as it was read at `at`, to a new file in `backups`
/// ([`backup_path`]), never over another; the path written.
fn keep_backup(
    path: &Path,
    bytes: &[u8],
    backups: &Path,
    at: time::OffsetDateTime,
) -> Result<PathBuf, SyncError> {
    std::fs::create_dir_all(backups)?;
    let mut n = 0;
    loop {
        let backup = backup_path(path, backups, at, n)
            .ok_or_else(|| SyncError::Other(format!("{} is not a file path", path.display())))?;
        match crate::fs::write_new(&backup, bytes) {
            Ok(()) => return Ok(backup),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e.into()),
        }
    }
}

/// Where the transcript at `path`, switched at `at`, is backed up in `backups`:
/// `<its file stem>-<at, to the millisecond, UTC>[-<n>].<its extension>`, `n` counting from 1 the
/// names already taken. In a directory of atuin's own, so no harness lists it with its sessions,
/// whatever its extension.
fn backup_path(path: &Path, backups: &Path, at: time::OffsetDateTime, n: u32) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_string_lossy();
    let ext = path.extension().map_or_else(|| "jsonl".into(), |e| e.to_string_lossy());
    let when = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}.{:03}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        at.millisecond()
    );
    let again = if n == 0 {
        String::new()
    } else {
        format!("-{n}")
    };
    Some(backups.join(format!("{stem}-{when}{again}.{ext}")))
}

/// Whether row `m` of a branch is no node of the session's tree: a row keyed on its content
/// (`syn-`, a line with no id: titles, pi's prompts from before ids) of a harness whose rows link
/// to their parents, which sync places beside the tree (`Analysis::rows_for`), not on it. Codex
/// links its content-keyed rows like any other.
fn off_tree(m: &RehydrateMessage, links: Links) -> bool {
    links == Links::Parents && m.source_id.starts_with(SYNTHETIC)
}

/// Where branch `branch` (its rows root to head, along the tree, with the rows beside it that go
/// with it: [`off_tree`]) leaves the transcript `base` read: how many of its first rows go with
/// the history the transcript shares with it (through the last row of the tree it holds), and the
/// line the last of those is (or was merged into), which the lines the transcript went on with
/// here descend from. Rows beside the tree say nothing of where it leaves: the transcript keeps
/// its own (its titles), and those it hasn't got past there are written after it ([`past`]).
///
/// Refused ([`SyncError::Unsupported`]) when it holds no row of the tree of it, or holds rows of
/// the tree past where it leaves it (which would be written twice).
fn branch_point(
    base: &LocalTip,
    branch: &[RehydrateMessage],
    links: Links,
) -> Result<(usize, String), SyncError> {
    let held = |m: &RehydrateMessage| base.known_source_ids.contains(&m.source_id);
    let tree = |m: &&RehydrateMessage| !off_tree(m, links);
    let leaves = branch.iter().position(|m| tree(&m) && !held(m)).unwrap_or(branch.len());
    let Some(last) = branch[..leaves].iter().rposition(|m| tree(&m)) else {
        return Err(SyncError::Unsupported("the transcript here shares nothing with that branch"));
    };
    let shared = last + 1;
    if branch[shared..].iter().filter(tree).any(held) {
        return Err(SyncError::Unsupported(
            "the transcript here holds rows of that branch past where it leaves it",
        ));
    }
    let last = &branch[last].source_id;
    let line = base.merged.get(last).unwrap_or(last);
    Ok((shared, line.clone()))
}

/// The rows of `branch` past where it leaves a transcript (its first `shared`; see
/// [`branch_point`]) that a switch writes after the lines it keeps, `kept`: all but those the
/// lines kept hold already (titles kept wherever they are). A row beside the tree that names no
/// parent ([`off_tree`]) follows the row of the branch before it, as sync places it there: it is
/// hung from that row (or from the line written last, when that row isn't written), not written
/// as a root of a tree of its own, which a harness resuming from it would read alone.
fn past(
    branch: &[RehydrateMessage],
    shared: usize,
    kept: &LocalTip,
    links: Links,
) -> Vec<RehydrateMessage> {
    let mut rest = Vec::new();
    for (n, m) in branch.iter().enumerate().skip(shared) {
        if kept.known_source_ids.contains(&m.source_id) {
            continue;
        }
        let mut m = m.clone();
        if off_tree(&m, links) && m.parent_source_id.is_none() {
            m.parent_source_id = n.checked_sub(1).map(|before| branch[before].source_id.clone());
        }
        rest.push(m);
    }
    rest
}

/// The row a transcript switched to a branch must resume from: the last row written after the
/// lines kept (`written`), else the line `from` the branch leaves it at.
fn switched_to<'a>(written: &'a [RehydrateMessage], from: &'a str) -> &'a str {
    written.last().map_or(from, |m| m.source_id.as_str())
}

/// `kept` (what the lines a switch keeps of a transcript hold) continued from `line`, where the
/// branch leaves it ([`branch_point`]), which it must still hold as a line of its own.
fn continuing_from(kept: LocalTip, line: &str) -> Result<LocalTip, SyncError> {
    if !kept.known_source_ids.contains(line) || kept.merged.contains_key(line) {
        return Err(SyncError::Other(format!(
            "the line {line} the branch leaves the transcript at was not kept"
        )));
    }
    Ok(LocalTip {
        tip_source_id: Some(line.to_owned()),
        ..kept
    })
}

/// The lines of JSONL transcript `bytes` a switch keeps ([`SessionSync::replace`]): each line
/// that `keep` says to, or `None` (it can't tell: a line it can't read, or one tied to no other)
/// when no line before it was dropped. A line of the history shared with the branch is kept;
/// one written after the transcript went its own way is not, unless `keep` says so (a title).
/// Each kept line is as it was, newline-terminated.
fn kept_lines(bytes: &[u8], mut keep: impl FnMut(&[u8]) -> Option<bool>) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut dropped = false;
    for line in jsonl_lines(bytes) {
        let kept = keep(line).unwrap_or(!dropped);
        dropped |= !kept;
        if kept {
            out.extend_from_slice(line);
            out.push(b'\n');
        }
    }
    out
}

/// Whether `id` descends from `from` (strictly) along `parents` (each line's parent), memoised in
/// `memo`; a line whose ancestry ends, or loops, short of `from` does not.
fn descends<'a>(
    parents: &HashMap<&'a str, &'a str>,
    memo: &mut HashMap<&'a str, bool>,
    id: &'a str,
    from: &str,
) -> bool {
    let mut path = Vec::new();
    let mut at = parents.get(id).copied();
    let found = loop {
        let Some(step) = at else {
            break false;
        };
        if step == from {
            break true;
        }
        if let Some(known) = memo.get(step) {
            break *known;
        }
        if path.contains(&step) {
            break false;
        }
        path.push(step);
        at = parents.get(step).copied();
    };
    for step in path {
        memo.insert(step, found);
    }
    memo.insert(id, found);
    found
}

/// Check that a transcript written out along a branch resumes from its head, `head`: the row the
/// harness continues from (`tip`) is it, or the line it was merged into.
fn resumes_from(
    tip: Option<&str>,
    merged: &HashMap<String, String>,
    head: &str,
) -> Result<(), SyncError> {
    let tip = tip.ok_or(SyncError::Unsupported("the transcript written out would be empty"))?;
    if tip == head || merged.get(head).is_some_and(|into| into == tip) {
        Ok(())
    } else {
        Err(SyncError::Unsupported("the agent would not resume the branch from its head"))
    }
}

/// The source id capture gives the row of line `m` of `session`, counting the id-less lines
/// alike in `occurrences` (from the transcript's first line on); `None` for a line capture keeps
/// no row of. As the daemon's `MessageEnricher` keys them: a line's own id, else its content
/// ([`synthetic_id`]).
fn capture_key<M: Message + ?Sized>(
    session: &SessionId,
    m: &M,
    occurrences: &mut HashMap<u64, u32>,
) -> Option<String> {
    if let Some(id) = m.id() {
        return Some(String::from(id));
    }
    let nothing = m.content().is_empty()
        && m.usage().is_none()
        && m.stop_reason().is_none()
        && m.model().is_none()
        && m.cwd().is_none()
        && m.git_branch().is_none()
        && m.title().is_none();
    if nothing {
        return None;
    }
    let hash = content_hash(session, m);
    let n = occurrences.entry(hash).or_insert(0);
    *n += 1;
    Some(synthetic_id(hash, *n - 1))
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
            // A row beside the tree follows the row of the branch before it ([`past`]), which
            // the transcript may hold beside its lines (a title) rather than as one.
            _ if off_tree(row, links) => true,
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
