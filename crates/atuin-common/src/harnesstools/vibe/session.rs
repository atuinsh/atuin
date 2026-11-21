//! Mistral Vibe sessions, read from its session store (as of Vibe 3.1.3, store format
//! `mistral.vibe.unified-session-store/v1`; the format is not documented).
//!
//! Every surface Vibe has writes one store, `$VIBE_HOME/logs/session/unified/<session>/`, with a
//! directory per session (a subagent's too, named `child-<hex>`):
//! - `CURRENT` names the session's newest generation, `generations/<generation>/`. Vibe writes a
//!   generation whenever the session goes idle, and keeps the two newest.
//! - A generation's `manifest.json` lists the content-addressed `chunks/<sha256>.json` its parts
//!   are stored in, and the journal segment that goes on from it.
//! - `projection-state.json` holds the session's public state (`snapshot.session`: its title,
//!   parent and times), and its chunks hold the public transcript: the entries every Vibe client
//!   shows, in order.
//! - `runtime-state.json` holds where the session works (`session_metadata.cwd`), its model, what
//!   kind of session it is (`identity`), and where an imported session came from
//!   (`import_provenance`).
//! - `journal/<sequence>.jsonl` records what happened since the generation was written. Its
//!   `projection_delta` records change the transcript (append, replace or remove an entry, or set
//!   them all) and the public state (`set_envelope`).
//!
//! A read takes the newest generation's transcript and folds its journal over it, so a turn's rows
//! are read as it runs. Text, reasoning and the output of an interrupted turn are written once,
//! settled. A tool call (an `effect` entry) is written when it starts and replaced when it
//! settles, so a read stops at the first call that hasn't settled yet, and goes on from there once
//! it has. A checkpoint is an entry's position in the transcript and a digest of its id. A read
//! whose checkpoint no longer names the entry it was taken after (a rewind that truncated the
//! history) starts the session over.
//!
//! **Rows.**
//! - `message`: what the user typed, steers included, and the model's answer, as text. An answer
//!   Vibe discarded (an interrupted turn) stops as [`StopReason::Aborted`].
//! - `reasoning`: the model's thinking.
//! - `effect`: a tool call, as a call row and a result row. The call is named as the model sees it
//!   (`bash` for Vibe's `file_system.bash`), with its arguments. The result is the call's
//!   structured output, an error when the call failed, was denied or a command exited non-zero.
//!   An edit or a file write carries its changes as a [`Patch`].
//! - `callback` (approvals), `notice` (hook runs, goals, loops) and `checkpoint` (a compaction,
//!   whose summary only the model's context holds) are not conversation, and are skipped.
//!
//! **Context.** Every row carries the session's directory, and the model's rows its model.
//! A session's `meta.json` names its git branch. A subagent names the session that spawned it as
//! a [`ParentKind::Subagent`] parent, and a fork the session it came from as a
//! [`ParentKind::Fork`] parent.
//!
//! **What only the journal holds.** A model call's token usage, and the arguments of the tool
//! calls it made, are recorded in the journal (`action_result`) and not in the transcript, which
//! leaves some out (the query of a `search_tool_functions` call). A read takes them from the
//! journal segments Vibe still keeps: the first row of a model call carries its usage, and a call
//! is given the arguments its entry leaves out. Vibe removes a segment once two newer generations
//! are written, so a session read long after it ran has neither for its older turns.
//!
//! **Titles.** The public state's title, generated after the first turn or set by the user. The
//! reader yields it as a row of its own whenever it changes, as Hermes's reader does.
//!
//! **Sessions atuin wrote.** Atuin writes a session out as one of Vibe's legacy (2.x) session
//! folders (see `rehydrate`), which Vibe imports into a new session of its store when it is
//! resumed. The imported session records the legacy session's id as its source, and the legacy
//! folder holds the rows atuin wrote (`atuin.jsonl`). Such a session is read under the id atuin
//! wrote it as: the rows atuin wrote first, as they were (a restored session's under the ids
//! capture already holds them by, a continuation's with the marker naming the session it carries
//! on), then what was added in Vibe. Its imported entries (`imported-<n>`) are skipped, as the
//! rows atuin wrote stand for them, tool calls and all, which Vibe's import leaves out. Vibe's import leaves the folder's title out, so the session has the
//! title atuin wrote until Vibe gives it one, and a fork names the session it was forked from as
//! the folder records it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use futures::stream::{BoxStream, SelectAll};
use futures::{Stream, StreamExt};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;
use typed_builder::TypedBuilder;
use xxhash_rust::xxh3::xxh3_64;

use super::{Vibe, session_root};
use crate::fs::tree_watcher::{FileStat, TreeWatcher};
use crate::harnesstools::resume::is_plain_name;
use crate::harnesstools::session::model::{
    Content, MessageId, ParentKind, Role, StopReason, TitleChange, TitleSource, ToolCallId,
    ToolResult, ToolUse, Usage,
};
use crate::harnesstools::session::patch::{Change, FilePatch, Hunk, Patch};
use crate::harnesstools::session::synthetic::SYNTHETIC;
use crate::harnesstools::session::{
    Checkpoint, Listener, Message, MessageError, Observable, RuntimeError, Session, SessionId,
    Sessions, WatchError,
};
use crate::path::PathBufExt as _;
use crate::sync::BlockingPool;

/// The store's directory under the session root.
pub(crate) const UNIFIED_DIR: &str = "unified";

/// The file naming a session's newest generation.
const CURRENT_FILE: &str = "CURRENT";

/// The directory of a session's journal segments.
const JOURNAL_DIR: &str = "journal";

/// The prefix of the entries Vibe makes of an imported session's history.
pub(crate) const IMPORTED_PREFIX: &str = "imported-";

/// The key under which atuin's legacy session folders list the rows atuin wrote.
pub(crate) const ATUIN_KEY: &str = "atuin";

/// The file in a legacy folder atuin wrote that holds the rows it wrote, one a line.
pub(crate) const WRITTEN_FILE: &str = "atuin.jsonl";

/// The file in a legacy folder atuin wrote that says how many of the rows written each version of
/// the folder holds, by Vibe's revision of it (see `rehydrate`).
pub(crate) const REVISIONS_FILE: &str = "atuin-revisions.jsonl";

/// A row atuin wrote to a legacy folder, as capture keys and holds it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WrittenRow {
    pub(crate) id: String,
    pub(crate) role: Role,
    pub(crate) time: Timestamp,
    pub(crate) content: Vec<Content>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stop: Option<StopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) git_branch: Option<String>,
}

/// The rows atuin wrote to the legacy folder at `dir`, in order. A line that doesn't parse is
/// skipped.
pub(crate) fn read_written(dir: &Path) -> std::io::Result<Vec<WrittenRow>> {
    let bytes = std::fs::read(dir.join(WRITTEN_FILE))?;
    Ok(bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect())
}

/// The role of the reader's own title rows, which no Vibe entry has.
const TITLE_ROW: &str = "atuin-title";

/// How many times a read looks again for a generation Vibe removed while it was read.
const GENERATION_RETRIES: usize = 3;

#[derive(Debug, Clone, TypedBuilder)]
pub struct VibeSessions {
    /// The session root ([`session_root`] when not given).
    #[builder(default, setter(strip_option, into))]
    root: Option<PathBuf>,
    pool: BlockingPool,
}

impl VibeSessions {
    fn resolve_root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(session_root)
    }
}

/// Where session `id` is under the session root `root`: its directory in the store, or the legacy
/// folder atuin wrote it as before Vibe imported it.
pub(crate) fn locate(root: &Path, id: &str) -> Option<PathBuf> {
    store_dir(root, id).or_else(|| legacy_folder(root, id))
}

/// The store directory holding session `id` under the session root `root`: its own, or for a
/// session atuin wrote, the one Vibe imported it into (the earliest, which Vibe resumes).
pub(crate) fn store_dir(root: &Path, id: &str) -> Option<PathBuf> {
    if !is_plain_name(id) {
        return None;
    }
    let unified = root.join(UNIFIED_DIR);
    let dir = unified.join(id);
    if dir.join(CURRENT_FILE).is_file() {
        return Some(dir);
    }
    legacy_folder(root, id)?;
    imports(&unified)?.get(id).cloned()
}

/// The sessions Vibe imported from legacy folders into the store `unified`, by the id of the
/// session imported: the earliest import of each, which Vibe resumes. Kept while the store's
/// directory is unchanged and every session in it was read whole, as a store holds thousands.
fn imports(unified: &Path) -> Option<Arc<HashMap<String, PathBuf>>> {
    type Imports = (PathBuf, SystemTime, Arc<HashMap<String, PathBuf>>);
    static CACHE: parking_lot::Mutex<Option<Imports>> = parking_lot::Mutex::new(None);

    let modified = std::fs::metadata(unified).and_then(|m| m.modified()).ok();
    if let Some((dir, at, found)) = CACHE.lock().as_ref()
        && dir == unified
        && Some(*at) == modified
    {
        return Some(Arc::clone(found));
    }
    let mut earliest: HashMap<String, (String, PathBuf)> = HashMap::new();
    let mut whole = true;
    for dir in std::fs::read_dir(unified).ok()?.filter_map(Result::ok).map(|e| e.path()) {
        let Some(runtime) = read_generation(&dir)
            .and_then(|generation| read_json::<Value>(&generation.dir.join("runtime-state.json")))
            .ok()
        else {
            // One being written, as an import is: read the store again next time.
            whole = false;
            continue;
        };
        let provenance = &runtime["import_provenance"];
        let Some(source) = provenance["source"]["session_id"].as_str() else {
            continue;
        };
        let at = provenance["imported_at"].as_str().unwrap_or_default().to_owned();
        let import = earliest.entry(source.to_owned()).or_insert_with(|| (at.clone(), dir.clone()));
        if (&at, &dir) < (&import.0, &import.1) {
            *import = (at, dir);
        }
    }
    let found = Arc::new(earliest.into_iter().map(|(id, (_, dir))| (id, dir)).collect());
    *CACHE.lock() =
        modified.filter(|_| whole).map(|at| (unified.to_owned(), at, Arc::clone(&found)));
    Some(found)
}

/// The legacy session folder of session `id` under the session root `root`: Vibe names one
/// `session_<date>_<time>_<first 8 characters of its id>`, and its `meta.json` holds the id.
pub(crate) fn legacy_folder(root: &Path, id: &str) -> Option<PathBuf> {
    let short = id.get(..8)?;
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("session_") && name.ends_with(short))
        })
        .find(|path| {
            read_json(&path.join("meta.json"))
                .is_ok_and(|meta: Value| meta["session_id"].as_str() == Some(id))
        })
}

/// Whether the legacy folder at `dir` is one atuin wrote.
fn written_by_atuin(dir: &Path) -> bool {
    read_json(&dir.join("meta.json")).is_ok_and(|meta: Value| meta.get(ATUIN_KEY).is_some())
}

/// The rows atuin wrote to the legacy folder at `dir`, by source id, in order, and the directory
/// it names; `None` for a folder atuin didn't write.
pub(crate) fn written_rows(dir: &Path) -> Option<(Vec<String>, Option<PathBuf>)> {
    let meta: Value = read_json(&dir.join("meta.json")).ok()?;
    meta.get(ATUIN_KEY)?;
    let rows = held_rows(dir).ok()?.into_iter().map(|row| row.id).collect();
    let cwd = meta["environment"]["working_directory"].as_str().map(PathBuf::from);
    Some((rows, cwd))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> std::io::Result<T> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(std::io::Error::other)
}

impl Sessions for VibeSessions {
    type Listener = VibeListener;

    fn last_active(&self) -> Option<SystemTime> {
        crate::fs::newest_modified(self.resolve_root().joined(UNIFIED_DIR))
    }

    fn listener(&self) -> Result<VibeListener, RuntimeError> {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(VibeListener {
            root,
            pool: self.pool.clone(),
        })
    }

    fn existing(
        &self,
    ) -> Result<impl Stream<Item = Result<VibeSession, RuntimeError>> + Send + 'static, RuntimeError>
    {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        let pool = self.pool.clone();
        Ok(async_stream::stream! {
            let scan_root = root.clone();
            let found = pool
                .run(move || {
                    let unified = scan_root.join(UNIFIED_DIR);
                    let entries = match std::fs::read_dir(&unified) {
                        Ok(entries) => entries,
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                        Err(err) => return Err(err),
                    };
                    let mut found = Vec::new();
                    for entry in entries {
                        let dir = entry?.path();
                        if let Some(identity) = Identity::of(&scan_root, &dir) {
                            found.push((dir, identity));
                        }
                    }
                    Ok(found)
                })
                .await;
            match found {
                Ok(Ok(found)) => {
                    for (dir, identity) in found {
                        yield Ok(VibeSession::new(dir, identity, pool.clone(), None));
                    }
                }
                Ok(Err(err)) => yield Err(RuntimeError::Io(err)),
                Err(cancelled) => yield Err(RuntimeError::Io(std::io::Error::other(cancelled))),
            }
        })
    }
}

impl Observable for Vibe {
    type Sessions = VibeSessions;

    fn sessions(&self, pool: BlockingPool) -> VibeSessions {
        VibeSessions::builder().pool(pool).build()
    }
}

#[derive(Debug, Clone)]
pub struct VibeListener {
    root: PathBuf,
    pool: BlockingPool,
}

/// The session directory `path` (a session's `CURRENT`, or one of its journal segments) belongs
/// to, under the store `unified`.
fn session_dir_of(unified: &Path, path: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(unified).ok()?;
    let parts: Vec<_> = relative.components().collect();
    let is_journal = |dir: &std::path::Component<'_>, file: &std::path::Component<'_>| {
        dir.as_os_str() == JOURNAL_DIR
            && Path::new(file.as_os_str()).extension().is_some_and(|ext| ext == "jsonl")
    };
    match parts[..] {
        [session, file] if file.as_os_str() == CURRENT_FILE => Some(unified.join(session)),
        [session, dir, file] if is_journal(&dir, &file) => Some(unified.join(session)),
        _ => None,
    }
}

/// A session directory's changes, as the listener has reported them so far.
struct Known {
    wake: watch::Sender<u64>,
    /// Whether the session was reported to the consumer.
    reported: bool,
}

/// One unit for each change to a file `stat` follows (once as it appears), tagged with `dir`.
fn changes_of(dir: PathBuf, mut stat: watch::Receiver<FileStat>) -> BoxStream<'static, PathBuf> {
    async_stream::stream! {
        yield dir.clone();
        while stat.changed().await.is_ok() {
            yield dir.clone();
        }
    }
    .boxed()
}

impl Listener for VibeListener {
    type Session = VibeSession;

    /// Every session of the store as the watcher first reports one of its files, each woken when
    /// its `CURRENT` or a journal segment changes. A session whose newest generation can't be
    /// read yet is reported once a change makes it readable.
    fn watch(self) -> impl Stream<Item = Result<VibeSession, WatchError>> + Send + 'static {
        let root = self.root;
        let pool = self.pool;
        async_stream::stream! {
            let unified = root.join(UNIFIED_DIR);
            if let Err(err) = std::fs::create_dir_all(&unified) {
                yield Err(WatchError::from(err));
                return;
            }
            // The watcher reports paths under the canonical root.
            let canonical = crate::path::AbsPathBuf::canonicalize(&unified).map_or_else(|_| unified.clone(), PathBuf::from);
            let files = {
                let canonical = canonical.clone();
                TreeWatcher::builder()
                    .filter(move |path| session_dir_of(&canonical, path).is_some())
                    .watch(&unified)
            };
            let mut files = match files {
                Ok(files) => files,
                Err(err) => {
                    yield Err(WatchError::from(err));
                    return;
                }
            };
            let mut changes: SelectAll<BoxStream<'static, PathBuf>> = SelectAll::new();
            // ponytail: entries are kept for every session ever seen; prune on end if the map
            // ever matters.
            let mut known: HashMap<PathBuf, Known> = HashMap::new();
            loop {
                tokio::select! {
                    file = files.next() => {
                        let Some(file) = file else { break };
                        let (path, stat) = file.into_parts();
                        let Some(dir) = session_dir_of(&canonical, &path) else { continue };
                        changes.push(changes_of(dir, stat));
                    }
                    Some(dir) = changes.next(), if !changes.is_empty() => {
                        let known = known.entry(dir.clone()).or_insert_with(|| Known {
                            wake: watch::channel(0).0,
                            reported: false,
                        });
                        if known.reported {
                            known.wake.send_modify(|n| *n = n.wrapping_add(1));
                            continue;
                        }
                        let (session_root, session_dir) = (root.clone(), dir.clone());
                        let identity = pool
                            .run(move || Identity::of(&session_root, &session_dir))
                            .await
                            .ok()
                            .flatten();
                        if let Some(identity) = identity {
                            known.reported = true;
                            let woken = known.wake.subscribe();
                            yield Ok(VibeSession::new(dir, identity, pool.clone(), Some(woken)));
                        }
                    }
                }
            }
        }
    }
}

/// Who a session is: the id it is read under, and the session it names as its parent.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    id: SessionId,
    parent: Option<(SessionId, ParentKind)>,
    /// Whether the session is one Vibe imported from a legacy folder atuin wrote, whose imported
    /// entries are rows atuin already holds.
    rehydrated: bool,
    /// The title atuin wrote the session with, which Vibe's import leaves out: the session's
    /// title until Vibe gives it one.
    written_title: Option<String>,
    /// The legacy folder atuin wrote the session to, holding the rows it wrote.
    written: Option<PathBuf>,
    /// How many of the rows written Vibe imported, when the folder says.
    imported: Option<usize>,
}

impl Identity {
    /// The identity of the session whose store directory is `dir`, under the session root
    /// `root`; `None` when the directory holds no readable session.
    fn of(root: &Path, dir: &Path) -> Option<Self> {
        let native = dir.file_name()?.to_str()?.to_owned();
        let generation = read_generation(dir).ok()?;
        let runtime: Value = read_json(&generation.dir.join("runtime-state.json")).ok()?;
        let imported_from = runtime["import_provenance"]["source"]
            .as_object()
            .filter(|source| source.get("backend").and_then(Value::as_str) == Some("legacy"))
            .and_then(|source| source.get("session_id")?.as_str())
            .filter(|source| legacy_folder(root, source).is_some_and(|f| written_by_atuin(&f)))
            // Only the import Vibe resumes is the session; another is a copy of its own.
            .filter(|source| {
                store_dir(root, source).is_some_and(|found| found.file_name() == dir.file_name())
            });
        if let Some(source) = imported_from {
            let written = legacy_folder(root, source);
            let meta = written
                .as_ref()
                .and_then(|folder| read_json::<Value>(&folder.join("meta.json")).ok());
            let meta = meta.unwrap_or(Value::Null);
            let written_title = meta["title"].as_str().map(str::to_owned);
            let fork_of = meta[ATUIN_KEY]["fork_of"].as_str();
            let revision = runtime["import_provenance"]["source"]["store_revision"].as_str();
            let imported = written.as_deref().zip(revision).and_then(|(f, r)| rows_at(f, r));
            return Some(Self {
                id: SessionId::from(source.to_owned()),
                parent: fork_of
                    .map(|parent| (SessionId::from(parent.to_owned()), ParentKind::Fork)),
                rehydrated: true,
                written_title,
                written,
                imported,
            });
        }
        let identity = &runtime["identity"];
        let parent_id = identity["parent_session_id"]
            .as_str()
            .or_else(|| runtime["session_metadata"]["parent_session_id"].as_str());
        let parent = parent_id.map(|parent| {
            let kind = match identity["kind"].as_str() {
                Some("subagent") => ParentKind::Subagent,
                _ => ParentKind::Fork,
            };
            // A session Vibe imported from a folder atuin wrote is captured under that folder's id.
            let imported = Some(parent)
                .filter(|parent| is_plain_name(parent) && *parent != native)
                .and_then(|parent| Self::of(root, &root.join(UNIFIED_DIR).joined(parent)))
                .filter(|parent| parent.rehydrated);
            (imported.map_or_else(|| SessionId::from(parent.to_owned()), |p| p.id), kind)
        });
        Some(Self {
            id: SessionId::from(native),
            parent,
            rehydrated: false,
            written_title: None,
            written: None,
            imported: None,
        })
    }

    /// The rows atuin wrote that Vibe imported, in order.
    fn written_rows(&self) -> Vec<WrittenRow> {
        let Some(folder) = &self.written else {
            return Vec::new();
        };
        let mut rows = read_written(folder).unwrap_or_default();
        rows.truncate(self.imported.unwrap_or(usize::MAX));
        rows
    }
}

/// The rows atuin wrote to the legacy folder at `dir` that its transcript holds, in order: an
/// append records its rows before it writes the transcript.
pub(crate) fn held_rows(dir: &Path) -> std::io::Result<Vec<WrittenRow>> {
    let mut rows = read_written(dir)?;
    let meta = std::fs::read(dir.join("meta.json"))?;
    let transcript = std::fs::read(dir.join("messages.jsonl"))?;
    if let Some(held) = rows_at(dir, &revision(&meta, &transcript)) {
        rows.truncate(held);
    }
    Ok(rows)
}

/// Vibe's revision of a legacy folder whose `meta.json` is `meta` and transcript `transcript`, as
/// its import records it.
pub(crate) fn revision(meta: &[u8], transcript: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};

    let mut digest = Sha256::new();
    for (label, bytes) in [(&b"metadata"[..], meta), (b"messages", transcript)] {
        digest.update(label);
        digest.update(b"\0");
        digest.update(bytes);
        digest.update(b"\0");
    }
    format!("{:x}", digest.finalize())
}

/// How many of the rows atuin wrote to the legacy folder `dir` its version `revision` holds;
/// `None` when the folder doesn't say.
fn rows_at(dir: &Path, revision: &str) -> Option<usize> {
    std::fs::read(dir.join(REVISIONS_FILE))
        .ok()?
        // Rows that change nothing the model sees leave the revision as it was: the latest count.
        .rsplit(|b| *b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .find(|line| line["revision"].as_str() == Some(revision))
        .and_then(|line| usize::try_from(line["rows"].as_u64()?).ok())
}

/// What every row of a session carries.
#[derive(Debug, Clone, Default)]
struct Context {
    cwd: Option<PathBuf>,
    model: Option<String>,
    git_branch: Option<String>,
    parent: Option<(SessionId, ParentKind)>,
}

#[derive(Debug, Deserialize)]
struct Current {
    generation: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    projection_state: Stored,
    recovery_journal_segment: Option<Segment>,
}

#[derive(Debug, Deserialize)]
struct Stored {
    #[serde(default)]
    chunks: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Segment {
    path: String,
}

/// A session's newest generation, as `CURRENT` names it.
struct Generation {
    name: String,
    dir: PathBuf,
}

fn read_generation(session: &Path) -> std::io::Result<Generation> {
    let current: Current = read_json(&session.join(CURRENT_FILE))?;
    if !is_plain_name(&current.generation) {
        return Err(std::io::Error::other("CURRENT names no generation"));
    }
    let dir = session.join("generations").joined(&current.generation);
    Ok(Generation {
        name: current.generation,
        dir,
    })
}

/// A generation as written: its transcript and public state, before its journal.
#[derive(Debug, Clone)]
struct Base {
    generation: String,
    entries: Vec<Value>,
    envelope: Value,
    context: Arc<Context>,
    /// The journal segment that goes on from it.
    journal: Option<PathBuf>,
    /// What the segments before that one recorded, which Vibe keeps a generation longer: the
    /// last model call of a turn is in the segment before the generation written as it ended.
    recorded: Arc<Recorded>,
}

/// A session as one read of the store saw it.
#[derive(Debug, Clone)]
struct Snapshot {
    entries: Vec<Value>,
    envelope: Value,
    context: Arc<Context>,
    recorded: Arc<Recorded>,
}

/// What the journal recorded of the model's calls.
#[derive(Debug, Clone, Default)]
struct Recorded {
    /// Each model call's usage, by its action id.
    usage: HashMap<String, Usage>,
    /// Each tool call the model made, by its id.
    calls: HashMap<String, Call>,
    /// The usage of each model call, by the first entry of the call's.
    shown: HashMap<String, Usage>,
}

#[derive(Debug, Clone)]
struct Call {
    /// The model call that made it.
    action: String,
    name: String,
    input: Option<serde_json::Map<String, Value>>,
}

impl Recorded {
    /// Take in an `action_result` record's payload: a model call's usage and tool calls.
    fn add(&mut self, payload: &Value) {
        let result = &payload["result"];
        let Some(action) = payload["action_id"].as_str() else {
            return;
        };
        if result["type"].as_str() != Some("completion_succeeded") {
            return;
        }
        let reply = &result["result"];
        for part in reply["parts"].as_array().into_iter().flatten() {
            if part["type"].as_str() == Some("tool_call")
                && let Some(id) = part["id"].as_str()
            {
                let input =
                    part["arguments_json"].as_str().and_then(|a| serde_json::from_str(a).ok());
                let call = Call {
                    action: action.to_owned(),
                    name: part["name"].as_str().unwrap_or_default().to_owned(),
                    input,
                };
                self.calls.insert(id.to_owned(), call);
            }
        }
        if let Some(usage) = usage_of(&reply["usage"]) {
            self.usage.insert(action.to_owned(), usage);
        }
    }

    /// The model call an entry shows part of.
    fn action_of<'a>(&'a self, entry: &'a Value) -> Option<&'a str> {
        let id = entry["id"].as_str()?;
        if let Some(rest) = id.strip_prefix("reasoning-").or_else(|| id.strip_prefix("assistant-"))
        {
            return rest.get(..36);
        }
        let call = call_id(entry)?;
        // A call code mode made names the model's call it ran in: `typescript:<n>:<call>:…`.
        let call = call
            .strip_prefix("typescript:")
            .and_then(|rest| rest.split(':').nth(1))
            .unwrap_or(call);
        self.calls.get(call).map(|c| c.action.as_str())
    }

    /// Give each model call's usage to the first of `entries` that shows part of it.
    fn show(&mut self, entries: &[Value]) {
        let mut seen = HashSet::new();
        let mut shown = HashMap::new();
        for entry in entries {
            let blank = entry["type"].as_str() == Some("reasoning")
                && entry["text"].as_str().unwrap_or_default().trim().is_empty();
            if let Some(action) = self.action_of(entry).filter(|_| !blank)
                && seen.insert(action)
                && let (Some(id), Some(usage)) = (entry["id"].as_str(), self.usage.get(action))
            {
                shown.insert(id.to_owned(), *usage);
            }
        }
        self.shown = shown;
    }
}

/// A model call's usage as Vibe records it, whose `input_tokens` include the cached ones; `None`
/// for none, or the zero usage Vibe gives a reply recorded without it.
fn usage_of(usage: &Value) -> Option<Usage> {
    let field = |name: &str| usage[name].as_u64();
    if field("total_tokens").unwrap_or_default() == 0 {
        return None;
    }
    let (cache_read, cache_write) = (field("cached_input_tokens"), field("cache_write_tokens"));
    Some(Usage {
        input: field("input_tokens").map(|input| {
            input.saturating_sub(cache_read.unwrap_or(0)).saturating_sub(cache_write.unwrap_or(0))
        }),
        output: field("output_tokens"),
        cache_read,
        cache_write,
        reasoning: field("reasoning_tokens"),
    })
}

/// The id of the tool call an effect entry shows: a tool search's is only in the entry's id.
fn call_id(entry: &Value) -> Option<&str> {
    entry["toolCallId"]
        .as_str()
        .or_else(|| entry["id"].as_str()?.strip_prefix("effect-tool-discovery-"))
}

/// Read the newest generation of the session at `dir`, reusing `cached` when it is still the
/// newest, then fold its journal over it. Blocking.
fn load(
    dir: &Path,
    identity: &Identity,
    cached: Option<&Base>,
) -> std::io::Result<(Base, Snapshot)> {
    let mut attempt = 0;
    let base = loop {
        let generation = read_generation(dir)?;
        if let Some(cached) = cached.filter(|c| c.generation == generation.name) {
            break cached.clone();
        }
        match read_base(dir, generation, identity) {
            Ok(base) => break base,
            // Vibe removes a generation two generations on: CURRENT has moved on meanwhile.
            Err(err)
                if err.kind() == std::io::ErrorKind::NotFound && attempt < GENERATION_RETRIES =>
            {
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    };
    let mut entries = base.entries.clone();
    let mut envelope = base.envelope.clone();
    let mut recorded = Recorded::clone(&base.recorded);
    if let Some(journal) = &base.journal {
        match std::fs::read(journal) {
            Ok(bytes) => replay(&bytes, &mut entries, &mut envelope, &mut recorded),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    recorded.show(&entries);
    let snapshot = Snapshot {
        entries,
        envelope,
        context: Arc::clone(&base.context),
        recorded: Arc::new(recorded),
    };
    Ok((base, snapshot))
}

fn read_base(dir: &Path, generation: Generation, identity: &Identity) -> std::io::Result<Base> {
    let manifest: Manifest = read_json(&generation.dir.join("manifest.json"))?;
    let mut entries = Vec::new();
    for chunk in &manifest.projection_state.chunks {
        if !is_plain_name(chunk) {
            return Err(std::io::Error::other("a manifest names a chunk outside the store"));
        }
        let part: Vec<Value> = read_json(&dir.join("chunks").joined(format!("{chunk}.json")))?;
        entries.extend(part);
    }
    let projection: Value = read_json(&generation.dir.join("projection-state.json"))?;
    let runtime: Value = read_json(&generation.dir.join("runtime-state.json"))?;
    let meta: Value = read_json(&dir.join("meta.json")).unwrap_or(Value::Null);
    let metadata = &runtime["session_metadata"];
    let context = Context {
        cwd: metadata["cwd"]
            .as_str()
            .or_else(|| meta["environment"]["working_directory"].as_str())
            .map(PathBuf::from),
        model: metadata["active_model"].as_str().map(str::to_owned),
        git_branch: meta["git_branch"].as_str().map(str::to_owned),
        parent: identity.parent.clone(),
    };
    let journal = manifest
        .recovery_journal_segment
        .map(|segment| segment.path)
        .filter(|path| {
            let path = Path::new(path);
            path.parent() == Some(Path::new(JOURNAL_DIR))
                && path.file_name().and_then(|n| n.to_str()).is_some_and(is_plain_name)
        })
        .map(|path| dir.join(path));
    let mut recorded = Recorded::default();
    let mut older: Vec<PathBuf> = std::fs::read_dir(dir.join(JOURNAL_DIR))
        .into_iter()
        .flatten()
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .filter(|path| Some(path) != journal.as_ref())
        .collect();
    older.sort();
    for segment in older {
        // A segment Vibe removed meanwhile recorded nothing still shown.
        let bytes = std::fs::read(segment).unwrap_or_default();
        for record in complete_records(&bytes) {
            if record["type"].as_str() == Some("action_result") {
                recorded.add(&record["payload"]);
            }
        }
    }
    Ok(Base {
        generation: generation.name,
        entries,
        envelope: projection["snapshot"]["session"].clone(),
        context: Arc::new(context),
        journal,
        recorded: Arc::new(recorded),
    })
}

/// The records of a journal segment, but for a last line still being written.
fn complete_records(bytes: &[u8]) -> impl Iterator<Item = Value> + '_ {
    let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(&[][..], |end| &bytes[..end]);
    complete.split(|b| *b == b'\n').filter_map(|line| serde_json::from_slice(line).ok())
}

/// Fold a journal segment's `projection_delta` records over `entries` and `envelope`, as Vibe
/// replays them, and take its model calls into `recorded`. A last line still being written is
/// left for the next read.
fn replay(bytes: &[u8], entries: &mut Vec<Value>, envelope: &mut Value, recorded: &mut Recorded) {
    for record in complete_records(bytes) {
        if record["type"].as_str() == Some("action_result") {
            recorded.add(&record["payload"]);
            continue;
        }
        if record["type"].as_str() != Some("projection_delta") {
            continue;
        }
        let Some(ops) = record["payload"]["delta"].as_array() else {
            continue;
        };
        for op in ops {
            match op["op"].as_str() {
                Some("append_entry") => entries.push(op["entry"].clone()),
                Some("replace_entry") => {
                    let id = op["id"].as_str();
                    if let Some(entry) = entries.iter_mut().find(|e| e["id"].as_str() == id) {
                        *entry = op["entry"].clone();
                    }
                }
                Some("remove_entry") => {
                    let id = op["id"].as_str();
                    entries.retain(|e| e["id"].as_str() != id);
                }
                Some("set_history_entries") => {
                    if let Some(all) = op["entries"].as_array() {
                        entries.clone_from(all);
                    }
                }
                Some("set_envelope") => *envelope = op["state"]["session"].clone(),
                other => {
                    tracing::debug!(op = ?other, "skipping a Vibe journal op this build doesn't know");
                }
            }
        }
    }
}

/// Whether an entry is final: everything but a tool call that hasn't settled yet.
fn settled(entry: &Value) -> bool {
    entry["type"].as_str() != Some("effect")
        || entry["generationStatus"].as_str() != Some("in_progress")
}

/// The checkpoint just past the entry at `position` (counted from 1) with id `id`.
fn checkpoint(position: usize, id: &str, generation: u32) -> Checkpoint {
    Checkpoint {
        generation,
        ..Checkpoint::new(position as u64, id.as_bytes())
    }
}

#[derive(Debug, Clone)]
pub struct VibeSession {
    identity: Identity,
    /// The session's directory in the store.
    dir: PathBuf,
    /// Wakes [`messages`](Session::messages) when the listener sees the session change; `None`
    /// reads it once.
    changes: Option<watch::Receiver<u64>>,
    pool: BlockingPool,
}

impl VibeSession {
    fn new(
        dir: PathBuf,
        identity: Identity,
        pool: BlockingPool,
        changes: Option<watch::Receiver<u64>>,
    ) -> Self {
        Self {
            identity,
            dir,
            changes,
            pool,
        }
    }

    /// The session whose store directory is `dir`, under the session root `root`, read once as
    /// it stands; `None` when the directory holds no readable session.
    #[must_use]
    pub fn open(root: &Path, dir: PathBuf, pool: BlockingPool) -> Option<Self> {
        let identity = Identity::of(root, &dir)?;
        Some(Self::new(dir, identity, pool, None))
    }

    fn messages_at(
        self,
        from: Option<Checkpoint>,
    ) -> impl Stream<Item = Result<(Checkpoint, VibeMessage), MessageError>> + Send + 'static {
        async_stream::stream! {
            let mut changes = self.changes.clone();
            let mut cached: Option<Base> = None;
            // How many times this read started the session over: its checkpoints' generation.
            let mut starts = 0u32;
            // The position of the next entry to read, and the checkpoint of the last one read.
            let mut position = 0usize;
            let mut last: Option<Checkpoint> = None;
            let mut resume = from.filter(|from| from.at > 0);
            let mut title: Option<TitleChange> = None;
            let mut titled = 0u32;
            // Whether this read yielded the rows atuin wrote, which go before every entry.
            let mut written = resume.is_some();
            // The last row read, which the next goes on from.
            let mut previous: Option<String> = None;
            loop {
                let (dir, identity, base) = (self.dir.clone(), self.identity.clone(), cached.take());
                let read = self.pool.run(move || load(&dir, &identity, base.as_ref())).await;
                match read {
                    Ok(Ok((base, snapshot))) => {
                        cached = Some(base);
                        if let Some(from) = resume.take() {
                            match position_of(&snapshot.entries, from) {
                                Some(at) => {
                                    position = at;
                                    last = Some(from);
                                    previous = row_before(&snapshot, at, self.identity.rehydrated);
                                    if previous.is_none() && self.identity.written.is_some() {
                                        let identity = self.identity.clone();
                                        previous = self.pool.run(move || identity.written_rows()).await
                                            .ok().and_then(|mut rows| rows.pop()).map(|row| row.id);
                                    }
                                }
                                None => {
                                    starts += 1;
                                    written = false;
                                }
                            }
                        }
                        if let Some(at) = last
                            && position_of(&snapshot.entries, at).is_none()
                        {
                            tracing::debug!(dir = %self.dir.display(), "a Vibe session's history changed under a read; reading it again");
                            starts += 1;
                            position = 0;
                            last = None;
                            title = None;
                            titled = 0;
                            written = false;
                            previous = None;
                        }
                        if !written && self.identity.written.is_some() {
                            written = true;
                            let identity = self.identity.clone();
                            let rows = self.pool.run(move || identity.written_rows()).await;
                            // A checkpoint before every entry: a read from it reads them all.
                            let at = Checkpoint { generation: starts, ..Checkpoint::new(0, b"") };
                            for row in rows.unwrap_or_default() {
                                let mut row = VibeMessage::written(row, &snapshot.context);
                                row.previous = previous.replace(row.id.clone());
                                yield Ok((at, row));
                            }
                        }
                        while let Some(entry) = snapshot.entries.get(position) {
                            if !settled(entry) {
                                break;
                            }
                            position += 1;
                            let id = entry["id"].as_str().unwrap_or_default();
                            let before = last;
                            let at = checkpoint(position, id, starts);
                            last = Some(at);
                            if self.identity.rehydrated && id.starts_with(IMPORTED_PREFIX) {
                                continue;
                            }
                            let rows = rows_of(entry, &snapshot.context, &snapshot.recorded);
                            let count = rows.len();
                            for (n, mut row) in rows.into_iter().enumerate() {
                                row.previous = previous.replace(row.id.clone());
                                // Every row but an entry's last resumes before the entry, so a
                                // read stopped between them reads the whole entry again.
                                let row_at = if n + 1 == count {
                                    at
                                } else {
                                    before.unwrap_or(Checkpoint { generation: starts, ..Checkpoint::new(0, b"") })
                                };
                                yield Ok((row_at, row));
                            }
                        }
                        let current = title_of(&snapshot.envelope).or_else(|| {
                            let written = self.identity.written_title.as_deref()?;
                            let change = TitleChange::new(TitleSource::Generated, written);
                            change.text.is_some().then_some(change)
                        });
                        if current != title
                            && let Some(at) = last
                            && (current.is_some() || title.is_some())
                        {
                            let change = current.clone().unwrap_or(TitleChange {
                                source: title.as_ref().map_or(TitleSource::Generated, |t| t.source),
                                text: None,
                            });
                            let when = time_of(&snapshot.envelope["updatedAt"]);
                            yield Ok((at, VibeMessage::title_row(change, titled, when, &snapshot.context)));
                            titled += 1;
                            title = current;
                        }
                    }
                    Ok(Err(err)) => {
                        tracing::warn!(dir = %self.dir.display(), %err, "can't read a Vibe session");
                        yield Err(MessageError::Incomplete);
                    }
                    Err(cancelled) => {
                        tracing::warn!(dir = %self.dir.display(), %cancelled, "can't read a Vibe session");
                        yield Err(MessageError::Incomplete);
                    }
                }
                let Some(wake) = changes.as_mut() else { break };
                if wake.changed().await.is_err() {
                    // The listener is gone: what changed since it last looked is read once more,
                    // then the session ends.
                    changes = None;
                }
            }
        }
    }
}

/// The last row the entries before `position` make, which the next row goes on from.
fn row_before(snapshot: &Snapshot, position: usize, rehydrated: bool) -> Option<String> {
    snapshot
        .entries
        .get(..position)?
        .iter()
        .rev()
        .filter(|entry| {
            !(rehydrated && entry["id"].as_str().is_some_and(|id| id.starts_with(IMPORTED_PREFIX)))
        })
        .find_map(|entry| {
            rows_of(entry, &snapshot.context, &snapshot.recorded).pop().map(|row| row.id)
        })
}

/// Where a read from `from` goes on in `entries`: just past the entry it names, `None` when that
/// entry is no longer where it was.
fn position_of(entries: &[Value], from: Checkpoint) -> Option<usize> {
    let at = usize::try_from(from.at).ok()?;
    let entry = entries.get(at.checked_sub(1)?)?;
    from.names(entry["id"].as_str().unwrap_or_default().as_bytes()).then_some(at)
}

fn title_of(envelope: &Value) -> Option<TitleChange> {
    let text = envelope["title"].as_str()?;
    let source = match envelope["titleSource"].as_str() {
        Some("manual") => TitleSource::Named,
        _ => TitleSource::Generated,
    };
    let change = TitleChange::new(source, text);
    change.text.is_some().then_some(change)
}

/// `value`, a time in milliseconds since the epoch, as Vibe stores times.
fn time_of(value: &Value) -> Option<Timestamp> {
    let millis = value.as_i64()?;
    Timestamp::from_millisecond(millis).ok()
}

impl Session for VibeSession {
    type Message = VibeMessage;

    fn id(&self) -> SessionId {
        self.identity.id.clone()
    }

    fn messages_from(
        self,
        from: Option<Checkpoint>,
    ) -> impl Stream<Item = Result<(Checkpoint, VibeMessage), MessageError>> + Send + 'static {
        self.messages_at(from)
    }

    fn read(&self) -> impl Stream<Item = Result<VibeMessage, MessageError>> + Send + 'static {
        Self {
            changes: None,
            ..self.clone()
        }
        .messages_at(None)
        .map(|item| item.map(|(_, message)| message))
    }

    /// Once Vibe writes the session after `since`: at once if its newest file is that new, else
    /// when the listener next sees it change.
    async fn changed_after(mut self, since: SystemTime) -> Option<Self> {
        const SLACK: std::time::Duration = std::time::Duration::from_secs(2);
        let since = since.checked_sub(SLACK).unwrap_or(since);
        let dir = self.dir.clone();
        let touched = self.pool.run(move || crate::fs::newest_modified(dir)).await.ok().flatten();
        if touched.is_none_or(|at| at >= since) {
            return Some(self);
        }
        let changed = self.changes.as_mut()?.changed().await;
        changed.ok().map(|()| self)
    }
}

/// What a [`VibeMessage`] is.
#[derive(Debug, Clone)]
enum Kind {
    Row {
        role: Role,
        content: Vec<Content>,
        stop: Option<StopReason>,
        model: bool,
    },
    Title(TitleChange),
}

/// One row of a Vibe session: part of a transcript entry, or a title row the reader made.
#[derive(Debug, Clone)]
pub struct VibeMessage {
    id: String,
    at: Option<Timestamp>,
    kind: Kind,
    context: Arc<Context>,
    /// The row before this one, which it goes on from (none for a title row).
    previous: Option<String>,
    usage: Option<Usage>,
}

impl VibeMessage {
    fn row(
        id: String,
        at: Option<Timestamp>,
        role: Role,
        content: Vec<Content>,
        context: &Arc<Context>,
    ) -> Self {
        Self {
            id,
            at,
            kind: Kind::Row {
                model: role == Role::Assistant,
                role,
                content,
                stop: None,
            },
            context: Arc::clone(context),
            previous: None,
            usage: None,
        }
    }

    fn written(row: WrittenRow, context: &Context) -> Self {
        let context = Context {
            cwd: row.cwd.or_else(|| context.cwd.clone()),
            model: row.model,
            git_branch: row.git_branch.or_else(|| context.git_branch.clone()),
            parent: context.parent.clone(),
        };
        Self {
            id: row.id,
            at: Some(row.time),
            kind: Kind::Row {
                model: context.model.is_some(),
                role: row.role,
                content: row.content,
                stop: row.stop,
            },
            context: Arc::new(context),
            previous: None,
            usage: None,
        }
    }

    fn stopped(mut self, reason: StopReason) -> Self {
        if let Kind::Row { stop, .. } = &mut self.kind {
            *stop = Some(reason);
        }
        self
    }

    fn title_row(change: TitleChange, nth: u32, at: Option<Timestamp>, context: &Context) -> Self {
        let mut keyed = change.text.as_deref().unwrap_or_default().as_bytes().to_vec();
        keyed.extend_from_slice(&nth.to_le_bytes());
        Self {
            id: format!("{SYNTHETIC}title-{:016x}", xxh3_64(&keyed)),
            at,
            kind: Kind::Title(change),
            context: Arc::new(Context {
                parent: context.parent.clone(),
                ..Context::default()
            }),
            previous: None,
            usage: None,
        }
    }
}

/// The rows of a transcript entry: none for one that isn't conversation. The first carries the
/// usage of the model call the entry is the first of.
fn rows_of(entry: &Value, context: &Arc<Context>, recorded: &Recorded) -> Vec<VibeMessage> {
    let mut rows = entry_rows(entry, context, recorded);
    if let Some(first) = rows.first_mut() {
        first.usage = recorded.shown.get(&first_id(entry)).copied();
    }
    rows
}

fn first_id(entry: &Value) -> String {
    entry["id"].as_str().unwrap_or_default().to_owned()
}

fn entry_rows(entry: &Value, context: &Arc<Context>, recorded: &Recorded) -> Vec<VibeMessage> {
    let id = first_id(entry);
    let at = time_of(&entry["createdAt"]);
    let discarded = entry["outcome"]["type"].as_str() == Some("discarded");
    match entry["type"].as_str() {
        Some("message") => {
            let role = match entry["role"].as_str() {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                Some("system") => Role::System,
                Some(other) => Role::Other(other.to_owned()),
                None => return Vec::new(),
            };
            let content: Vec<Content> =
                entry["content"].as_array().into_iter().flatten().filter_map(block).collect();
            if content.is_empty() {
                return Vec::new();
            }
            let row = VibeMessage::row(id, at, role, content, context);
            vec![if discarded {
                row.stopped(StopReason::Aborted)
            } else {
                row
            }]
        }
        Some("reasoning") => {
            let text = entry["text"].as_str().unwrap_or_default();
            if text.trim().is_empty() {
                return Vec::new();
            }
            let row = VibeMessage::row(
                id,
                at,
                Role::Assistant,
                vec![Content::Reasoning(text.to_owned())],
                context,
            );
            vec![if discarded {
                row.stopped(StopReason::Aborted)
            } else {
                row
            }]
        }
        Some("effect") => effect_rows(entry, &id, at, context, recorded),
        _ => Vec::new(),
    }
}

/// A content block of a message: text as text, anything else (an image, a resource) as it is,
/// without the bytes of an inline image.
fn block(block: &Value) -> Option<Content> {
    match block["type"].as_str() {
        Some("text") => {
            let text = block["text"].as_str()?;
            (!text.is_empty()).then(|| Content::Text(text.to_owned()))
        }
        _ => {
            let mut block = block.clone();
            if let Some(map) = block.as_object_mut() {
                map.remove("data");
            }
            Some(Content::Other(block))
        }
    }
}

/// The name the model calls a tool by: Vibe's qualified name (`file_system.bash`) after its group.
fn model_name(qualified: &str) -> &str {
    qualified.rsplit_once('.').map_or(qualified, |(_, name)| name)
}

/// A tool call's rows: the call, and its result once it has one.
fn effect_rows(
    entry: &Value,
    id: &str,
    at: Option<Timestamp>,
    context: &Arc<Context>,
    recorded: &Recorded,
) -> Vec<VibeMessage> {
    let detail = &entry["detail"];
    let qualified = detail["toolName"].as_str().unwrap_or("tool");
    let name = model_name(qualified);
    let call = ToolCallId::from(call_id(entry).unwrap_or(id).to_owned());
    let mut input = match &detail["input"] {
        Value::Null => Value::Object(serde_json::Map::new()),
        input => input.clone(),
    };
    let made = recorded.calls.get(call.as_ref()).filter(|made| made.name == name);
    if let (Some(input), Some(made)) = (input.as_object_mut(), made.and_then(|m| m.input.as_ref()))
    {
        for (key, value) in made {
            input.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    let call_row = VibeMessage::row(
        format!("{id}:call"),
        at,
        Role::Assistant,
        vec![Content::ToolUse(ToolUse {
            id: call.clone(),
            name: name.to_owned(),
            input: input.clone(),
        })],
        context,
    )
    .stopped(StopReason::ToolUse);
    let state = &entry["state"];
    if state.is_null() {
        return vec![call_row];
    }
    let (output, error) = result_of(state);
    let mut content = vec![Content::ToolResult(ToolResult {
        call: call.clone(),
        output,
        error,
    })];
    if !error && let Some(patch) = patch_of(name, &call, &input, &state["output"]) {
        content.push(Content::Patch(patch));
    }
    let result_row = VibeMessage::row(
        format!("{id}:result"),
        time_of(&entry["updatedAt"]).or(at),
        Role::Tool,
        content,
        context,
    );
    vec![call_row, result_row]
}

/// A settled call's output, and whether it failed.
fn result_of(state: &Value) -> (Value, bool) {
    let output = &state["output"];
    let failed = state["status"].as_str() == Some("failed")
        || output["type"].as_str() == Some("failure")
        || !state["error"].is_null();
    if failed {
        let message = output["error"]["message"]
            .as_str()
            .or_else(|| state["error"]["message"].as_str())
            .or_else(|| state["display"]["message"].as_str())
            .unwrap_or("the tool call failed");
        return (Value::String(message.to_owned()), true);
    }
    let structured = &output["structured_content"];
    let exited = structured["returncode"].as_i64().is_some_and(|code| code != 0);
    let text = || {
        output["content"]
            .as_array()
            .filter(|blocks| !blocks.is_empty())
            .map(|blocks| Value::Array(blocks.clone()))
    };
    let value = match structured {
        // A tool search keeps only how many tools it found, in its output.
        Value::Null => text()
            .or_else(|| state["outputText"].as_str().filter(|t| !t.is_empty()).map(Value::from))
            .unwrap_or_else(|| output.clone()),
        Value::Object(map) if map.is_empty() => text().unwrap_or_else(|| structured.clone()),
        structured => structured.clone(),
    };
    (value, exited)
}

/// The changes an edit or a file write made, as Vibe recorded them beside its result.
fn patch_of(name: &str, call: &ToolCallId, input: &Value, output: &Value) -> Option<Patch> {
    let structured = &output["structured_content"];
    let file = match name {
        "edit" | "search_replace" => {
            let edits = &output["_meta"]["mistralai.vibe.sdk.search_replace"];
            let path = structured["file"].as_str().or_else(|| input["file_path"].as_str())?;
            let hunks: Vec<Hunk> = edits["blocks"]
                .as_array()?
                .iter()
                .filter_map(|block| {
                    // Vibe keeps each line's newline; a hunk's lines don't.
                    let lines = |key: &str| -> Option<Vec<&str>> {
                        Some(
                            block[key]
                                .as_array()?
                                .iter()
                                .filter_map(Value::as_str)
                                .map(|line| line.strip_suffix('\n').unwrap_or(line))
                                .collect(),
                        )
                    };
                    let old = lines("old_lines")?;
                    let new = lines("new_lines")?;
                    let lines = old
                        .iter()
                        .map(|line| format!("-{line}"))
                        .chain(new.iter().map(|line| format!("+{line}")))
                        .collect();
                    Some(Hunk {
                        old_start: block["old_start_line"].as_u64()?,
                        old_lines: old.len() as u64,
                        new_start: block["new_start_line"].as_u64()?,
                        new_lines: new.len() as u64,
                        lines,
                    })
                })
                .collect();
            if hunks.is_empty() {
                return None;
            }
            FilePatch {
                path: path.to_owned(),
                change: Change::Update,
                moved_to: None,
                hunks,
            }
        }
        "write_file" => {
            let path = structured["path"].as_str().or_else(|| input["path"].as_str())?;
            let content = input["content"].as_str()?;
            let previous = if structured["file_existed"].as_bool() == Some(true) {
                Some(structured["previous_content"].as_str()?)
            } else {
                None
            };
            FilePatch::between(path.to_owned(), previous, content)
        }
        _ => return None,
    };
    Some(Patch {
        call: call.clone(),
        files: vec![file],
    })
}

impl Message for VibeMessage {
    fn id(&self) -> Option<MessageId> {
        Some(MessageId::from(self.id.clone()))
    }

    /// Rows that share a timestamp (a reply and its reasoning) keep their order only by naming
    /// the row before them.
    fn parent_id(&self) -> Option<MessageId> {
        self.previous.clone().map(MessageId::from)
    }

    fn role(&self) -> Role {
        match &self.kind {
            Kind::Row { role, .. } => role.clone(),
            Kind::Title(_) => Role::Other(TITLE_ROW.to_owned()),
        }
    }

    fn timestamp(&self) -> Option<Timestamp> {
        self.at
    }

    fn content(&self) -> Vec<Content> {
        match &self.kind {
            Kind::Row { content, .. } => content.clone(),
            Kind::Title(_) => Vec::new(),
        }
    }

    fn model(&self) -> Option<String> {
        match &self.kind {
            Kind::Row { model: true, .. } => self.context.model.clone(),
            _ => None,
        }
    }

    fn usage(&self) -> Option<Usage> {
        self.usage
    }

    fn stop_reason(&self) -> Option<StopReason> {
        match &self.kind {
            Kind::Row { stop, .. } => stop.clone(),
            Kind::Title(_) => None,
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.context.cwd.clone()
    }

    fn git_branch(&self) -> Option<String> {
        self.context.git_branch.clone()
    }

    fn parent_session(&self) -> Option<SessionId> {
        self.context.parent.as_ref().map(|(id, _)| id.clone())
    }

    fn parent_kind(&self) -> Option<ParentKind> {
        self.context.parent.as_ref().map(|(_, kind)| *kind)
    }

    fn title(&self) -> Option<TitleChange> {
        match &self.kind {
            Kind::Title(change) => Some(change.clone()),
            Kind::Row { .. } => None,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
