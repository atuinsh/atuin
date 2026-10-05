//! Codex: the rollouts `$CODEX_HOME/sessions/<yyyy>/<mm>/<dd>/rollout-<timestamp>-<thread>[_<rollout>].jsonl`.
//!
//! **Format.** A rollout in Codex's paginated history mode (`session_meta.history_mode:
//! "paginated"`, what Codex writes since 0.156, and what rehydrate writes) numbers every line
//! (`ordinal`: 0 for its `session_meta`, then one more per line), and Codex continues a rollout
//! numbering on from its last line's number (codex-rs `rollout/src/ordinal.rs`). A thread may be
//! kept in several files, each a *segment*: `thread/revert` starts a new one
//! (`rollout-<timestamp>-<thread>_<rollout>.jsonl`), whose `session_meta` names where it continues
//! the one before (`history_base: {thread_id: <the rollout it continues>, end_byte_offset: <the
//! byte past the last line kept>}`). Codex resumes the thread from its newest segment, reading
//! the history it continues from the older ones ([`Lineage`]).
//!
//! **Tip.** Codex continues from the last line of the newest segment; the tip is the last row
//! capture keeps of the history it holds. Rows are keyed as capture keys them: a line's own id,
//! else its content (`syn-`; see [`synthetic`](crate::harnesstools::session::synthetic)).
//!
//! **Append.** Fast-forward appends to the newest segment only. Codex's rows name no parent of
//! their own; capture links each to the row it captured before it, so rows continuing the tip
//! hang from it (or name nothing, captured before capture linked them). The rehydrate writer
//! builds their lines, which are numbered on from the segment's last line: Codex keeps numbering
//! from there, and nothing else reads the numbers of synced rows. A rollout in Codex's legacy
//! history mode (no numbers) is refused: Codex can't continue it with numbered lines. Nothing
//! else is touched: the thread index already names the segment.
//!
//! **Liveness.** A Codex process holds an exclusive `flock` on
//! `$CODEX_HOME/thread-writer-locks/<thread>.lock` for as long as it has the thread loaded
//! (codex-rs `rollout/src/writer_lock.rs`). Appending takes that lock as Codex's own writers
//! take it, around the check that the rollout is unchanged and the write, so no Codex loads the
//! thread meanwhile; but never makes the directory, which says that Codex here keeps locks.
//! Without it, any Codex process running here in the session's directory (or below it, or where
//! that can't be told) may have the thread loaded; so may one anywhere whose command line names
//! the thread (`codex resume <thread>`), and while the newest rollout was modified in the last
//! [`RECENT`](super::RECENT), any Codex at all. The lock is a `flock`
//! on any Unix (only Linux's `/proc/locks` also says which process holds it); on Windows a lock
//! nothing holds is not taken to mean that no Codex has the thread loaded.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use futures::StreamExt;
use serde_json::{Value, json};

use super::liveness::{agent_running, changed_lately, flock_holder};
use super::{
    AppendOptions, AppendOutcome, Dirs, Links, Liveness, LocalTip, Processes, Seen, SessionSync,
    Stamp, SyncError, append_jsonl, blocking, blocking_liveness, check_segment, jsonl_lines,
    merged_in, modified, require_idle,
};
use crate::harnesstools::codex::session::{CodexSession, default_root, resume_id, session_id_of};
use crate::harnesstools::codex::{Codex, rehydrate};
use crate::harnesstools::rehydrate::{RehydrateMessage, RehydrateSession};
use crate::harnesstools::resume::is_plain_name;
use crate::harnesstools::session::synthetic::{content_hash, synthetic_id};
use crate::harnesstools::session::{Message, Session, SessionId};
use crate::sync::BlockingPool;

impl SessionSync for Codex {
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError> {
        local_tip_in(&codex_home(), id).await
    }

    async fn is_live(&self, id: &str, cwd: Option<&Path>) -> Liveness {
        let (home, id, cwd) = (codex_home(), id.to_owned(), cwd.map(Path::to_path_buf));
        blocking_liveness(move || {
            let thread = resume_id(&id);
            let newest = is_plain_name(thread)
                .then(|| rollouts_of(&home.join("sessions"), thread).pop())
                .flatten();
            let seen = Seen {
                dirs: Dirs::of(cwd),
                names: vec![thread.to_owned()],
                changing: newest.is_some_and(|path| changed_lately(modified(&path))),
            };
            liveness(&home, &id, &Processes::here(), &seen)
        })
        .await
    }

    async fn append(
        &self,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        let (home, thread) = (codex_home(), resume_id(id).to_owned());
        let seen = appending(base, &thread, options.cwd);
        let (at, of) = (home.clone(), id.to_owned());
        require_idle(
            blocking_liveness(move || liveness(&at, &of, &Processes::here(), &seen)).await,
        )?;
        let (base, lines, taken) = (base.clone(), lines.to_vec(), options.taken_ids.cloned());
        blocking(move || append_in(&home, &thread, &base, &lines, taken.as_ref())).await
    }
}

/// `$CODEX_HOME`, else `~/.codex`: where `sessions/` is.
fn codex_home() -> PathBuf {
    let root = default_root();
    root.parent().map_or_else(|| root.clone(), Path::to_path_buf)
}

/// Where Codex keeps its writer locks (codex-rs `WRITER_LOCK_DIR`).
const LOCKS: &str = "thread-writer-locks";

/// Thread `thread`, as [`SessionSync::append`] to the rollout `base` read takes it, last seen
/// working in `cwd`: a Codex names the thread on its command line (`codex resume <thread>`).
fn appending(base: &LocalTip, thread: &str, cwd: Option<&Path>) -> Seen {
    base.seen(cwd, vec![thread.to_owned()])
}

/// Whether a Codex process has session `id` (a thread, or a segment of one, as `seen`) loaded.
pub(super) fn liveness(home: &Path, id: &str, procs: &Processes, seen: &Seen) -> Liveness {
    let thread = resume_id(id);
    if !is_plain_name(thread) {
        return Liveness::NotLive;
    }
    let locks = home.join(LOCKS);
    if !locks.is_dir() {
        return agent_running(procs, "codex", &[], seen);
    }
    match flock_holder(&locks.join(format!("{thread}.lock")), procs) {
        // Whether Codex on Windows holds the lock as it does elsewhere is unconfirmed: a lock
        // nothing holds there says no more than the processes do.
        Liveness::NotLive if cfg!(windows) => agent_running(procs, "codex", &[], seen),
        liveness => liveness,
    }
}

/// A Codex thread's writer lock, held as Codex's own writers hold it (codex-rs
/// `WriterLockCoordinator`): the lock file is opened (and made) and locked under the home's
/// coordination lock, and closed and removed under it again when let go.
pub(super) struct WriterLock {
    dir: PathBuf,
    path: PathBuf,
    file: Option<File>,
}

impl WriterLock {
    /// `None` when the home has no lock directory: no Codex there keeps locks to respect.
    pub(super) fn acquire(home: &Path, thread: &str) -> Result<Option<Self>, SyncError> {
        let dir = home.join(LOCKS);
        if !dir.is_dir() {
            return Ok(None);
        }
        let _coordination = coordinate(&dir)?;
        let path = dir.join(format!("{thread}.lock"));
        let file =
            OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self {
                dir,
                path,
                file: Some(file),
            })),
            Err(std::fs::TryLockError::WouldBlock) => Err(SyncError::Live(None)),
            Err(std::fs::TryLockError::Error(err)) => Err(err.into()),
        }
    }
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        let coordination = coordinate(&self.dir);
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
        drop(coordination);
    }
}

/// The coordination lock over the writer locks of a Codex home (codex-rs
/// `COORDINATION_LOCK_FILE`), held until dropped.
fn coordinate(dir: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(".coordination.lock"))?;
    file.lock()?;
    Ok(file)
}

/// The rollouts of thread `thread` under `root` (`<yyyy>/<mm>/<dd>/`, or flat in the archive),
/// oldest first: named for when each was started.
fn rollouts_of(root: &Path, thread: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().filter_map(Result::ok) {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push(path);
            } else if thread_of(&path).is_some_and(|(t, _)| t == thread) {
                found.push(path);
            }
        }
    }
    found.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    found
}

/// The thread a rollout file is of, and its own rollout id (the thread's, for its first).
fn thread_of(path: &Path) -> Option<(String, String)> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    let session = session_id_of(&format!("rollout-{stem}"));
    let (thread, rollout) = session.as_ref().split_once('_').unwrap_or((session.as_ref(), ""));
    let rollout = if rollout.is_empty() {
        thread
    } else {
        rollout
    };
    Some((thread.to_owned(), rollout.to_owned()))
}

/// One rollout file, read as capture reads it.
struct Rollout {
    bytes: Vec<u8>,
    /// The rows capture keeps of it, in order: where the line each is read from ends, its id.
    rows: Vec<(u64, String)>,
    /// Its `session_meta` payload.
    meta: Value,
}

impl Rollout {
    async fn read(path: &Path, pool: &BlockingPool) -> Result<Self, SyncError> {
        let bytes = std::fs::read(path)?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let session = session_id_of(stem);
        let reader = CodexSession::open(session.clone(), path.to_path_buf(), pool.clone());
        let mut lines = std::pin::pin!(reader.messages_from(None));
        let mut occurrences = HashMap::new();
        let mut rows = Vec::new();
        while let Some(line) = lines.next().await {
            // A line capture can't read makes no row (a torn last one).
            let Ok((at, message)) = line else {
                continue;
            };
            if at.at > bytes.len() as u64 {
                // Written since the bytes were read.
                break;
            }
            if let Some(id) = capture_key(&session, &message, &mut occurrences) {
                rows.push((at.at, id));
            }
        }
        let meta = jsonl_lines(&bytes)
            .next()
            .and_then(|l| serde_json::from_slice::<Value>(l).ok())
            .map(|l| l["payload"].clone())
            .unwrap_or_default();
        Ok(Self { bytes, rows, meta })
    }

    /// The rollout it continues, and up to which byte of it, if it does.
    fn base(&self) -> Option<(&str, u64)> {
        let base = &self.meta["history_base"];
        Some((base["thread_id"].as_str()?, base["end_byte_offset"].as_u64()?))
    }
}

/// The source id capture gives the row of line `m` of `session`, counting the id-less lines
/// alike in `occurrences`; `None` for a line capture keeps no row of. As the daemon's
/// `MessageEnricher` keys them.
fn capture_key<M: Message>(
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

/// A thread's history as Codex reads it from its newest segment: the segments it continues,
/// oldest first, each with how many of its rows the history keeps (all of the newest).
struct Lineage {
    parts: Vec<(Rollout, usize)>,
}

impl Lineage {
    /// The history segment `head` of thread `thread` continues, as far as its segments are here.
    async fn of(home: &Path, thread: &str, head: Rollout, pool: &BlockingPool) -> Self {
        let mut all = rollouts_of(&home.join("sessions"), thread);
        all.extend(rollouts_of(&home.join("archived_sessions"), thread));
        let mut seen = HashSet::new();
        let kept = head.rows.len();
        let mut parts = vec![(head, kept)];
        loop {
            let at = &parts.last().expect("a lineage has its head").0;
            let Some((base, end)) = at.base().map(|(b, e)| (b.to_owned(), e)) else {
                break;
            };
            let found = all.iter().find(|p| thread_of(p).is_some_and(|(_, r)| r == base));
            let Some(path) = found.filter(|p| seen.insert((*p).clone())) else {
                break;
            };
            let Ok(next) = Rollout::read(path, pool).await else {
                break;
            };
            let kept = next.rows.iter().take_while(|(at, _)| *at <= end).count();
            parts.push((next, kept));
        }
        parts.reverse();
        Self { parts }
    }

    /// The ids of the rows the history holds, and of the rows merged into its lines.
    fn held(&self) -> (HashSet<String>, HashMap<String, String>) {
        let known: HashSet<String> = self
            .parts
            .iter()
            .flat_map(|(rollout, kept)| rollout.rows[..*kept].iter().map(|(_, id)| id.clone()))
            .collect();
        let mut merged = HashMap::new();
        for (rollout, kept) in &self.parts {
            let end = kept.checked_sub(1).map_or(0, |k| rollout.rows[k].0);
            let end = usize::try_from(end).unwrap_or(usize::MAX).min(rollout.bytes.len());
            merged.extend(merged_in(&rollout.bytes[..end], &known));
        }
        let mut known = known;
        known.extend(merged.keys().cloned());
        (known, merged)
    }

    /// The last row of the history.
    fn tip(&self) -> Option<&str> {
        self.parts
            .iter()
            .rev()
            .find_map(|(rollout, kept)| kept.checked_sub(1).map(|k| rollout.rows[k].1.as_str()))
    }
}

/// [`SessionSync::local_tip`] of session `id` (a thread, or a segment of one) under Codex home
/// `home`.
pub(super) async fn local_tip_in(home: &Path, id: &str) -> Result<Option<LocalTip>, SyncError> {
    let thread = resume_id(id);
    if !is_plain_name(thread) {
        return Ok(None);
    }
    let Some(path) = rollouts_of(&home.join("sessions"), thread).pop() else {
        return Ok(None);
    };
    let pool = BlockingPool::new(std::num::NonZeroUsize::MIN);
    let modified = modified(&path);
    let head = Rollout::read(&path, &pool).await?;
    let stamp = Stamp::of(&head.bytes);
    let cwd = head.meta["cwd"].as_str().map(PathBuf::from);
    let lineage = Lineage::of(home, thread, head, &pool).await;
    let (known, merged) = lineage.held();
    Ok(Some(LocalTip {
        native_path: path,
        known_source_ids: known,
        merged,
        tip_source_id: lineage.tip().map(str::to_owned),
        stamp,
        modified,
        cwd,
    }))
}

/// [`SessionSync::append`] to thread `thread` under Codex home `home`, once it is known to be
/// idle: under the thread's writer lock, when Codex keeps them here.
pub(super) fn append_in(
    home: &Path,
    thread: &str,
    base: &LocalTip,
    lines: &[RehydrateMessage],
    taken: Option<&HashSet<String>>,
) -> Result<AppendOutcome, SyncError> {
    let lock = WriterLock::acquire(home, thread)?;
    let path = &base.native_path;
    if rollouts_of(&home.join("sessions"), thread).last() != Some(path) {
        // Another segment is the newest now.
        return Err(SyncError::Changed);
    }
    let bytes = std::fs::read(path)?;
    if Stamp::of(&bytes) != base.stamp {
        return Err(SyncError::Changed);
    }
    check_segment(lines, base, taken, Links::Chained)?;
    let parsed = || jsonl_lines(&bytes).filter_map(|l| serde_json::from_slice::<Value>(l).ok());
    let meta = parsed().next().map(|l| l["payload"].clone()).unwrap_or_default();
    let last = parsed().next_back().and_then(|l| l["ordinal"].as_u64());
    let (true, Some(last)) = (meta["history_mode"] == "paginated", last) else {
        return Err(SyncError::Unsupported(
            "this Codex rollout is in the legacy history mode, which can't be continued with \
             numbered lines",
        ));
    };

    let session = RehydrateSession {
        id: thread.to_owned(),
        title: None,
        cwd: meta["cwd"].as_str().unwrap_or_default().into(),
        original_cwd: None,
        git_branch: None,
        model: None,
        started_at: time::OffsetDateTime::now_utc(),
        messages: Vec::new(),
        fork_of: None,
    };
    let mut out = Vec::new();
    let mut appended = Vec::new();
    for row in rehydrate::rows(&session, lines) {
        if !row.lines.is_empty() {
            appended.push(row.source_id);
        }
        out.extend(row.before.into_iter().chain(row.lines));
    }
    for (n, line) in (last + 1..).zip(&mut out) {
        line["ordinal"] = json!(n);
    }
    if !out.is_empty() {
        append_jsonl(path, base.stamp, &out)?;
    }
    drop(lock);
    Ok(AppendOutcome {
        native_path: path.clone(),
        tip_source_id: appended.last().cloned().or_else(|| base.tip_source_id.clone()),
        appended,
    })
}

#[cfg(test)]
mod tests;
