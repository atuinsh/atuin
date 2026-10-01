//! Codex: the rollouts `$CODEX_HOME/sessions/<yyyy>/<mm>/<dd>/rollout-<timestamp>-<thread>[_<rollout>].jsonl`.
//!
//! **Format.** A rollout in Codex's paginated history mode (`session_meta.history_mode:
//! "paginated"`, what Codex writes since 0.156, and what rehydrate writes) numbers every line
//! (`ordinal`: 0 for its `session_meta`, then one more per line). Codex continues a rollout from
//! its last line's number (codex-rs `rollout/src/ordinal.rs`), and builds the model's history by
//! reading back from the end (`thread-store/src/local/model_context.rs`). A thread may be kept in
//! several files, each a *segment*: `thread/revert` starts a new one (`revert_thread.rs`),
//! `rollout-<timestamp>-<thread>_<rollout>.jsonl`, whose `session_meta` (the thread's own, taking
//! the next number itself) names where it continues from: `history_base: {thread_id: <the
//! rollout id of the file it continues>, end_ordinal_exclusive: <the first number not
//! included>, end_byte_offset: <the byte just past the last line included>}`. The thread's history
//! is then that file's lines up to the offset (and so on down), followed by the segment's own.
//! Codex resumes the thread from its newest segment: the one its thread index names
//! (`state_<n>.sqlite` `threads.rollout_path`, see `codex::state_db`),
//! else the newest by name. Capture takes every segment for the thread (see
//! `codex::session::session_id_of`).
//!
//! **Tip.** Codex continues from the last line of the newest segment; the tip is the last line
//! capture keeps a row of by its own id, within the history that segment continues.
//!
//! **Append.** Codex's rows carry no parent pointer; they are placed by number (`seq`), which the
//! rows keep from the lines they were captured from. Rows the history already holds (by id; a
//! content-keyed `syn-` row by its number and content) are skipped.
//!
//! - *Fast-forward*: the first row is numbered past every line of the newest segment (or carries
//!   no number), and hangs from nothing or from the tip. Its lines are appended to that segment,
//!   each row at its number (see `rehydrate::number`), so
//!   Codex continues from the last of them.
//! - *Branch*: the first row takes a number the newest segment already has (another host
//!   continued the thread from an earlier line), or hangs from a line before the tip. The rows are
//!   written as a new segment of the same thread, the way `thread/revert` writes one: its
//!   `session_meta` is the newest segment's own, taking the number just before the first row's
//!   (where the line that started that host's turn was: a revert takes that one too), and its
//!   `history_base` points just past the line it hangs from: the first row's
//!   `parent_source_id` when the caller names one (Codex rows have none of their own), else the
//!   last line numbered below it. Nothing is copied: the history before is read from the older
//!   segment, and the rows keep their ids, so re-capture finds every one of them already synced.
//!   The thread index is pointed at the new segment.
//! - *Switching to a branch already here* ([`AppendOptions::head`], nothing to append): a new
//!   segment of nothing but its `session_meta`, continuing just past that line, wherever among
//!   the thread's segments it is.
//!
//! A rollout in Codex's legacy history mode (no numbers) can only be caught up: Codex cannot
//! continue a legacy rollout from a paginated one.
//!
//! **Liveness.** A Codex process holds an exclusive `flock` on
//! `$CODEX_HOME/thread-writer-locks/<thread>.lock` for as long as it has the thread loaded, and
//! removes the file when it lets go (codex-rs `rollout/src/writer_lock.rs`; seen held by `codex
//! app-server` 0.157 with a thread loaded, and gone after it exited). A lock file that is there
//! says it all. Without one, a Codex old enough to keep no locks may still have the thread
//! loaded, whether or not a newer Codex (or another tool) made the directory, so its processes
//! are looked for: a process that may have the thread (one resuming it, one in its directory
//! when that is known or in any directory when not, a server) keeps it from being written.
//! Appending takes that lock itself, as Codex's own writers take it, so no Codex that keeps locks
//! loads the thread meanwhile; but never makes the directory, only a Codex does.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use time::OffsetDateTime;

use super::liveness::{Claim, ProcessInfo, flock_holder, scan};
use super::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, Processes, SessionSync, Stamp, SyncError,
    append_jsonl, blocking, merged_in, require_idle,
};
use crate::harnesstools::codex::Codex;
use crate::harnesstools::codex::rehydrate::{self, HistoryMode, create_new, number};
use crate::harnesstools::codex::session::{
    ReadLine, RolloutName, read_lines, resume_id, rollouts_of,
};
use crate::harnesstools::codex::state_db::{self, StateDbError};
use crate::harnesstools::rehydrate::{RehydrateError, RehydrateMessage, RehydrateSession};
use crate::harnesstools::session::{Message, SessionId};
use crate::utils::{env_nonempty, home_dir};

impl SessionSync for Codex {
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError> {
        let (home, thread) = (codex_home(), resume_id(id).to_owned());
        blocking(move || local_tip_in(&home, &thread)).await
    }

    async fn is_live(&self, id: &str, cwd: Option<&Path>) -> Liveness {
        let (home, id, cwd) = (codex_home(), id.to_owned(), cwd.map(Path::to_path_buf));
        tokio::task::spawn_blocking(move || {
            liveness(&home, &id, cwd.as_deref(), &Processes::here())
        })
        .await
        .unwrap_or(Liveness::Unknown)
    }

    async fn append(
        &self,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        require_idle(self.is_live(id, None).await)?;
        append_in(&codex_home(), resume_id(id), base, lines, options).await
    }
}

/// `$CODEX_HOME`, else `~/.codex`.
fn codex_home() -> PathBuf {
    env_nonempty("CODEX_HOME").map_or_else(|| home_dir().join(".codex"), PathBuf::from)
}

/// Whether a Codex process has session `id` (a thread, or a segment of one) loaded.
pub(super) fn liveness(home: &Path, id: &str, cwd: Option<&Path>, procs: &Processes) -> Liveness {
    let thread = resume_id(id);
    if !crate::harnesstools::resume::is_plain_name(thread) {
        return Liveness::NotLive;
    }
    let lock = home.join(LOCKS).join(format!("{thread}.lock"));
    if lock.is_file() {
        match flock_holder(&lock, procs) {
            // Let go (and removed) just now: no lock file to go by after all.
            Liveness::NotLive if !lock.exists() => {}
            held => return held,
        }
    }
    // No lock file: no Codex that keeps locks has the thread, but one that keeps none might.
    match procs.list() {
        Ok(processes) => scan(&processes, cwd, |p| claim(p, thread)),
        Err(_) => Liveness::Unknown,
    }
}

/// What a Codex process (the native binary, or the node launcher in front of it) claims of
/// thread `thread`.
fn claim(process: &ProcessInfo, thread: &str) -> Claim {
    let name = |arg: &str| {
        let name = arg.rsplit(['/', '\\']).next().unwrap_or(arg);
        matches!(name, "codex" | "codex.exe" | "codex.js")
    };
    let args = match process.argv.as_slice() {
        [program, rest @ ..] if name(program) => rest,
        [_, script, rest @ ..] if name(script) => rest,
        _ => return Claim::None,
    };
    let words: Vec<&str> =
        args.iter().map(String::as_str).filter(|a| !a.starts_with('-')).collect();
    match words.as_slice() {
        ["resume", session, ..] | ["exec", "resume", session, ..] => {
            Claim::Session(*session == thread)
        }
        ["fork" | "login" | "logout" | "completion" | "help" | "apply" | "debug", ..] => {
            Claim::None
        }
        ["app-server" | "mcp-server" | "proto" | "cloud", ..] => Claim::Any,
        _ => Claim::Directory(None),
    }
}

/// Where Codex keeps its writer locks (codex-rs `WRITER_LOCK_DIR`).
const LOCKS: &str = "thread-writer-locks";

/// A Codex thread's writer lock, held as Codex's own writers hold it (codex-rs
/// `WriterLockCoordinator`): the lock file is opened (and made) and locked under the home's
/// coordination lock, and closed and removed under it again when let go.
///
/// Only taken in a home whose lock directory a Codex made. Making it here would tell
/// [`liveness`] that this home's Codex keeps locks, and an older one that keeps none would pass
/// for idle as long as it runs.
struct WriterLock {
    dir: PathBuf,
    path: PathBuf,
    file: Option<File>,
}

impl WriterLock {
    /// `None` when the home has no lock directory: no Codex there keeps locks to respect.
    fn acquire(home: &Path, thread: &str) -> Result<Option<Self>, SyncError> {
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
/// `COORDINATION_LOCK_FILE`), held until dropped. The directory `dir` must be there already.
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

/// Where a segment continues the one before it (codex-rs `HistoryPosition`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Base {
    rollout: String,
    end_ordinal_exclusive: u64,
    end_byte_offset: u64,
}

/// One rollout file of a thread, read as capture reads it.
struct Rollout {
    name: RolloutName,
    bytes: Vec<u8>,
    lines: Vec<ReadLine>,
    /// Its own `session_meta` payload.
    meta: Option<Value>,
}

impl Rollout {
    fn read(path: &Path) -> Result<Self, SyncError> {
        let name = RolloutName::of(path)
            .ok_or_else(|| SyncError::Other(format!("{} is no rollout", path.display())))?;
        let bytes = std::fs::read(path)?;
        let lines = read_lines(&bytes, &SessionId::from(name.thread.clone()));
        let meta = lines.first().and_then(|l| l.message.own_meta()).cloned();
        Ok(Self {
            name,
            bytes,
            lines,
            meta,
        })
    }

    fn paginated(&self) -> bool {
        self.meta.as_ref().is_some_and(|meta| meta["history_mode"] == "paginated")
    }

    /// Where it continues another segment, if it does.
    fn base(&self) -> Option<Base> {
        let base = &self.meta.as_ref()?["history_base"];
        Some(Base {
            rollout: base["thread_id"].as_str()?.to_owned(),
            end_ordinal_exclusive: base["end_ordinal_exclusive"].as_u64()?,
            end_byte_offset: base["end_byte_offset"].as_u64()?,
        })
    }

    /// The greatest number among its lines.
    fn last_ordinal(&self) -> Option<u64> {
        self.lines.iter().filter_map(|l| l.message.seq()).max()
    }
}

/// A thread's history as Codex reads it from its newest segment: each segment, oldest first,
/// with the lines of it the history takes (up to where the next continues it; all of the newest).
struct Lineage {
    parts: Vec<(Rollout, usize)>,
}

impl Lineage {
    /// The history the segment `head` of thread `head.name.thread` continues, as far as it
    /// stays in the thread and its segments are here.
    fn of(home: &Path, head: Rollout) -> Self {
        let mut parts = vec![];
        let mut seen = HashSet::new();
        let mut at = head;
        let mut end = None;
        loop {
            let base = at.base();
            let kept = end.map_or(at.lines.len(), |offset| {
                at.lines.iter().take_while(|l| l.end <= offset).count()
            });
            seen.insert(at.name.rollout.clone());
            let thread = at.name.thread.clone();
            parts.push((at, kept));
            let Some(base) = base else {
                break;
            };
            if seen.contains(&base.rollout) {
                break;
            }
            let Some(next) = find_rollout(home, &thread, &base.rollout) else {
                break;
            };
            match Rollout::read(&next) {
                Ok(next) => {
                    end = Some(base.end_byte_offset);
                    at = next;
                }
                Err(_) => break,
            }
        }
        parts.reverse();
        Self { parts }
    }

    fn head(&self) -> &Rollout {
        &self.parts.last().expect("a lineage has its head").0
    }

    /// Every line of the history, with the segment it is in.
    fn lines(&self) -> impl Iterator<Item = (usize, &ReadLine)> {
        self.parts
            .iter()
            .enumerate()
            .flat_map(|(n, (rollout, kept))| rollout.lines[..*kept].iter().map(move |l| (n, l)))
    }

    /// The ids capture keys the history's lines on, where a line has one of its own, and those
    /// of the rows its lines record as merged into them (see [`LocalTip::merged`]), with the line
    /// each went into.
    fn held(&self) -> (HashSet<String>, HashMap<String, String>) {
        let mut known: HashSet<String> =
            self.lines().filter_map(|(_, l)| l.message.id()).map(String::from).collect();
        let mut merged = HashMap::new();
        for (rollout, kept) in &self.parts {
            let Some(end) = kept.checked_sub(1).map(|k| rollout.lines[k].end) else {
                continue;
            };
            let end = usize::try_from(end).unwrap_or(usize::MAX).min(rollout.bytes.len());
            merged.extend(merged_in(&rollout.bytes[..end], &known));
        }
        known.extend(merged.keys().cloned());
        (known, merged)
    }

    /// The last line of the history capture keeps a row of by its own id, but for a segment's
    /// `session_meta` (the thread's own, again), which continues nothing.
    fn tip(&self) -> Option<(usize, &ReadLine)> {
        self.lines()
            .filter(|(_, l)| l.message.id().is_some() && l.message.own_meta().is_none())
            .last()
    }

    /// The line of the history with id `id`.
    fn find(&self, id: &str) -> Option<(usize, &ReadLine)> {
        self.lines().filter(|(_, l)| l.message.id().is_some_and(|m| m.as_ref() == id)).last()
    }

    /// The line of the history holding row `id`: its own, or the one it was merged into.
    fn find_held(&self, id: &str, merged: &HashMap<String, String>) -> Option<(usize, &ReadLine)> {
        self.find(id).or_else(|| self.find(merged.get(id)?))
    }

    /// Whether the history holds `row`: a line with its id (or one it was merged into), or for a
    /// row capture keyed on the line's content, an id-less line with that content at its number.
    fn holds(&self, row: &RehydrateMessage, known: &HashSet<String>) -> bool {
        if known.contains(&row.source_id) || !row.source_id.starts_with("syn-") {
            return known.contains(&row.source_id);
        }
        let millis = |at: OffsetDateTime| at.unix_timestamp_nanos() / 1_000_000;
        self.lines().any(|(_, l)| {
            let m = &l.message;
            m.id().is_none()
                && (row.seq.is_none() || m.seq().is_none() || m.seq() == row.seq)
                && m.timestamp().is_none_or(|at| millis(at) == millis(row.timestamp))
                && m.role() == row.role
                && m.content() == row.content
                && m.model() == row.model
                && m.cwd() == row.cwd
                && m.usage() == row.usage
                && m.stop_reason() == row.stop_reason
        })
    }

    /// Where a segment continuing the history just past `line` (of part `part`) starts.
    fn base_after(&self, part: usize, line: &ReadLine) -> Result<Base, SyncError> {
        let ordinal = line.message.seq().ok_or(SyncError::Unsupported(
            "the Codex line to branch from is unnumbered (a legacy rollout)",
        ))?;
        Ok(Base {
            rollout: self.parts[part].0.name.rollout.clone(),
            end_ordinal_exclusive: ordinal + 1,
            end_byte_offset: line.end,
        })
    }

    /// Where a segment continuing the history with lines from number `first` on starts: past
    /// the last line of the history numbered below `first - 1`, the number its `session_meta`
    /// takes.
    fn base_before(&self, first: u64) -> Result<Base, SyncError> {
        let exclusive = first.checked_sub(1).filter(|e| *e > 0).ok_or(SyncError::Unsupported(
            "a Codex branch can't start before the rollout's first line",
        ))?;
        let (part, line) = self
            .lines()
            .filter(|(_, l)| l.message.seq().is_some_and(|seq| seq < exclusive))
            .last()
            .ok_or(SyncError::Unsupported(
                "nothing of the Codex rollout is numbered below the branch",
            ))?;
        Ok(Base {
            rollout: self.parts[part].0.name.rollout.clone(),
            end_ordinal_exclusive: exclusive,
            end_byte_offset: line.end,
        })
    }

    /// Where a branch whose first row written is numbered `first` (if it is), hanging from
    /// `hangs_from` (if it names a line), continues the history: just past the last turn that
    /// ended in between, where a revert of the next turn would continue it (the lines after it,
    /// up to the branch's own, are what started this host's next turn: a `task_started`,
    /// `thread_settings_applied`, which the branch's host wrote its own of); else just past the
    /// line it hangs from, else just before the number its `session_meta` takes, the one before
    /// its first row.
    fn branch_base(
        &self,
        hangs_from: Option<(usize, &ReadLine)>,
        first: Option<u64>,
    ) -> Result<Base, SyncError> {
        let from = hangs_from.and_then(|(_, line)| line.message.seq());
        let turn_end = self
            .lines()
            .filter(|(_, l)| {
                l.message.ends_turn()
                    && l.message.seq().is_some_and(|seq| {
                        from.is_none_or(|from| seq >= from)
                            && first.is_none_or(|first| seq + 1 < first)
                    })
            })
            .last();
        match (turn_end, hangs_from, first) {
            (Some((part, line)), ..) | (None, Some((part, line)), _) => self.base_after(part, line),
            (None, None, Some(first)) => self.base_before(first),
            (None, None, None) => Err(SyncError::Other("a branch placed nowhere".to_owned())),
        }
    }
}

/// The rollout of thread `thread` whose own id is `rollout`, under `sessions/` or
/// `archived_sessions/`.
fn find_rollout(home: &Path, thread: &str, rollout: &str) -> Option<PathBuf> {
    ["sessions", "archived_sessions"].into_iter().find_map(|dir| {
        rollouts_of(&home.join(dir), thread)
            .into_iter()
            .find(|path| RolloutName::of(path).is_some_and(|name| name.rollout == rollout))
    })
}

/// [`SessionSync::local_tip`] of thread `thread` under Codex home `home`.
pub(super) fn local_tip_in(home: &Path, thread: &str) -> Result<Option<LocalTip>, SyncError> {
    let Some(path) = rollouts_of(&home.join("sessions"), thread).pop() else {
        return Ok(None);
    };
    let lineage = Lineage::of(home, Rollout::read(&path)?);
    let head = lineage.head();
    let (known, merged) = lineage.held();
    Ok(Some(LocalTip {
        native_path: path,
        known_source_ids: known,
        merged,
        tip_source_id: lineage.tip().and_then(|(_, l)| l.message.id()).map(String::from),
        stamp: Stamp::of(&head.bytes),
    }))
}

/// What the blocking half of an append did, for the half that updates Codex's thread index.
struct Written {
    outcome: AppendOutcome,
    /// A new segment, and the one it took over from.
    segment: Option<(PathBuf, PathBuf)>,
    /// Held until the index names the new segment (when the home keeps locks).
    lock: Option<WriterLock>,
}

/// [`SessionSync::append`] to thread `thread` under Codex home `home`, once it is known to be
/// idle.
pub(super) async fn append_in(
    home: &Path,
    thread: &str,
    base: &LocalTip,
    lines: &[RehydrateMessage],
    options: &AppendOptions<'_>,
) -> Result<AppendOutcome, SyncError> {
    let (h, t, b, l) = (home.to_path_buf(), thread.to_owned(), base.clone(), lines.to_vec());
    let head = options.head.map(str::to_owned);
    let taken = options.taken_ids.cloned();
    let written = blocking(move || write(&h, &t, &b, &l, head.as_deref(), taken.as_ref())).await?;
    if let Some((segment, replacing)) = &written.segment {
        let pointed = state_db::point_at(home, thread, Some(replacing), segment).await;
        if let Err(err) = pointed {
            // As `thread/revert` does when the index moved on meanwhile.
            let _ = std::fs::remove_file(segment);
            return Err(match err {
                StateDbError::Elsewhere(_) => SyncError::Changed,
                StateDbError::Db(err) => SyncError::Other(err.to_string()),
            });
        }
    }
    drop(written.lock);
    Ok(written.outcome)
}

fn write(
    home: &Path,
    thread: &str,
    base: &LocalTip,
    lines: &[RehydrateMessage],
    make_head: Option<&str>,
    taken: Option<&HashSet<String>>,
) -> Result<Written, SyncError> {
    let lock = WriterLock::acquire(home, thread)?;
    let path = rollouts_of(&home.join("sessions"), thread).pop().ok_or(SyncError::NotFound)?;
    if path != base.native_path {
        return Err(SyncError::Changed);
    }
    let head = Rollout::read(&path)?;
    if Stamp::of(&head.bytes) != base.stamp {
        return Err(SyncError::Changed);
    }
    let lineage = Lineage::of(home, head);
    let (known, merged) = lineage.held();
    let mut rows: Vec<RehydrateMessage> = Vec::with_capacity(lines.len());
    // Where the rows held already leave off: what the first row written hangs from, when it
    // names nothing itself.
    let mut placed = lines.first().and_then(|row| row.parent_source_id.clone());
    for row in lines {
        if !lineage.holds(row, &known) {
            rows.push(row.clone());
        } else if rows.is_empty() && lineage.find_held(&row.source_id, &merged).is_some() {
            placed = Some(row.source_id.clone());
        }
    }
    if let Some(first) = rows.first_mut()
        && first.parent_source_id.is_none()
    {
        first.parent_source_id = placed;
    }
    let mut ids = HashSet::new();
    for row in rows.iter().filter(|row| !row.source_id.starts_with("syn-")) {
        let id = row.source_id.as_str();
        if taken.is_some_and(|taken| taken.contains(id)) || !ids.insert(id) {
            return Err(SyncError::IdTaken(row.source_id.clone()));
        }
    }
    let tip = lineage.tip().and_then(|(_, l)| l.message.id()).map(String::from);
    let head = lineage.head();
    let session = RehydrateSession {
        id: thread.to_owned(),
        title: None,
        cwd: head.meta.as_ref().and_then(|m| m["cwd"].as_str()).unwrap_or_default().into(),
        original_cwd: None,
        git_branch: None,
        model: None,
        started_at: OffsetDateTime::now_utc(),
        messages: Vec::new(),
    };
    let nothing = |tip: Option<String>, lock| Written {
        outcome: AppendOutcome {
            native_path: path.clone(),
            appended: Vec::new(),
            tip_source_id: tip,
            marked_tip: false,
        },
        segment: None,
        lock,
    };

    let Some(first) = rows.first() else {
        let Some(wanted) = make_head else {
            return Ok(nothing(tip, lock));
        };
        if tip.as_deref() == Some(wanted)
            || merged.get(wanted).is_some_and(|into| tip.as_ref() == Some(into))
        {
            return Ok(nothing(tip, lock));
        }
        let at = match lineage.find_held(wanted, &merged) {
            Some((part, line)) => lineage.base_after(part, line)?,
            None => elsewhere(home, thread, wanted)?,
        };
        let segment = new_segment(home, &lineage, &at, &[])?;
        return Ok(Written {
            outcome: AppendOutcome {
                native_path: segment.clone(),
                appended: Vec::new(),
                tip_source_id: Some(wanted.to_owned()),
                marked_tip: true,
            },
            segment: Some((segment, path)),
            lock,
        });
    };

    let hangs_from = match first.parent_source_id.as_deref() {
        Some(parent) => Some(
            lineage
                .find_held(parent, &merged)
                .ok_or_else(|| SyncError::Disconnected(first.source_id.clone()))?,
        ),
        None => None,
    };
    let from_tip = hangs_from
        .is_none_or(|(_, line)| tip.is_some() && line.message.id().map(String::from) == tip);
    let mode = if head.paginated() {
        HistoryMode::Paginated
    } else {
        HistoryMode::Legacy
    };
    let rows = rehydrate::rows(&session, &rows, mode);
    // Placed by the first row written: rows the rollout cannot carry (reasoning) take no line.
    let Some(written) = rows.iter().find(|row| !row.lines.is_empty()) else {
        return Ok(nothing(tip, lock));
    };
    let last = head.last_ordinal();
    let past_end = match (written.seq, last) {
        (Some(seq), Some(last)) => seq > last,
        _ => true,
    };

    if from_tip && (past_end || !head.paginated()) {
        let next = head.paginated().then(|| last.map_or(0, |last| last + 1));
        let numbered = number(&rows, next);
        if !numbered.lines.is_empty() {
            append_jsonl(&path, base.stamp, &numbered.lines)?;
        }
        return Ok(Written {
            outcome: AppendOutcome {
                native_path: path.clone(),
                tip_source_id: numbered.written.last().cloned().or(tip),
                appended: numbered.written,
                marked_tip: false,
            },
            segment: None,
            lock,
        });
    }
    if !head.paginated() {
        return Err(SyncError::Unsupported(
            "this Codex rollout is in the legacy history mode, which can't branch",
        ));
    }
    // Not from the tip, or numbered within the rollout: a branch.
    let at = lineage.branch_base(hangs_from, written.seq)?;
    let segment = new_segment(home, &lineage, &at, &rows)?;
    let numbered = number(&rows, Some(at.end_ordinal_exclusive + 1));
    Ok(Written {
        outcome: AppendOutcome {
            native_path: segment.clone(),
            tip_source_id: numbered.written.last().cloned(),
            appended: numbered.written,
            marked_tip: false,
        },
        segment: Some((segment, path)),
        lock,
    })
}

/// Where a segment making line `id` the tip starts, for a line of the thread outside the history
/// its newest segment continues (a branch an older segment holds).
fn elsewhere(home: &Path, thread: &str, id: &str) -> Result<Base, SyncError> {
    for path in rollouts_of(&home.join("sessions"), thread).iter().rev() {
        let rollout = Rollout::read(path)?;
        let Some(line) =
            rollout.lines.iter().rfind(|l| l.message.id().is_some_and(|m| m.as_ref() == id))
        else {
            continue;
        };
        if !rollout.paginated() {
            break;
        }
        let ordinal = line
            .message
            .seq()
            .ok_or(SyncError::Unsupported("the Codex line to branch from is unnumbered"))?;
        return Ok(Base {
            rollout: rollout.name.rollout.clone(),
            end_ordinal_exclusive: ordinal + 1,
            end_byte_offset: line.end,
        });
    }
    Err(SyncError::Disconnected(id.to_owned()))
}

/// Write a new segment of the thread `lineage` is the history of, continuing it at `at` with
/// `rows`, and hand back its path.
fn new_segment(
    home: &Path,
    lineage: &Lineage,
    at: &Base,
    rows: &[rehydrate::Row],
) -> Result<PathBuf, SyncError> {
    let head = lineage.head();
    let thread = head.name.thread.as_str();
    // Named after every segment of the thread, as Codex tells the newest by name.
    let newest = lineage
        .parts
        .iter()
        .map(|(rollout, _)| rollout.name.stamp.clone())
        .chain(
            rollouts_of(&home.join("sessions"), thread)
                .iter()
                .filter_map(|p| Some(RolloutName::of(p)?.stamp)),
        )
        .max()
        .unwrap_or_default();
    let mut created = OffsetDateTime::now_utc();
    if RolloutName::stamp_at(created) <= newest {
        let format =
            time::macros::format_description!("[year]-[month]-[day]T[hour]-[minute]-[second]");
        let named = time::PrimitiveDateTime::parse(&newest, &format)
            .map_err(|_| SyncError::Other(format!("a Codex rollout is named for {newest}")))?;
        created = named.assume_utc() + time::Duration::SECOND;
    }
    let rollout = crate::utils::uuid_v7().to_string();
    let name = RolloutName::file_name(created, thread, &rollout);
    let dir = home
        .join("sessions")
        .join(format!("{:04}", created.year()))
        .join(format!("{:02}", u8::from(created.month())))
        .join(format!("{:02}", created.day()));

    // The newest segment's own meta, continuing the history at `at` (codex-rs
    // `revert_thread.rs` `create_replacement_recorder`).
    let mut meta = head.meta.clone().unwrap_or_else(|| json!({"id": thread}));
    let stamped = rehydrate::stamp(created);
    meta["timestamp"] = json!(stamped);
    meta["history_mode"] = json!("paginated");
    meta["history_base"] = json!({
        "thread_id": at.rollout,
        "end_ordinal_exclusive": at.end_ordinal_exclusive,
        "end_byte_offset": at.end_byte_offset,
    });
    if let Some(fork) = meta["forked_from_ordinal_exclusive"].as_u64() {
        meta["forked_from_ordinal_exclusive"] = json!(fork.min(at.end_ordinal_exclusive));
    }
    let mut header = rehydrate::line(&stamped, "session_meta", meta);
    header["ordinal"] = json!(at.end_ordinal_exclusive);
    let mut text = header.to_string();
    text.push('\n');
    for line in number(rows, Some(at.end_ordinal_exclusive + 1)).lines {
        text.push_str(&line.to_string());
        text.push('\n');
    }
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    create_new(&path, text.as_bytes()).map_err(|err| match err {
        RehydrateError::Io(err) => SyncError::Io(err),
        other => SyncError::Other(other.to_string()),
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests;
