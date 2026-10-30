//! Kiro CLI sessions: one directory per session, `<bucket>/sess_<uuid>/`, under
//! `$KIRO_HOME/sessions` (`~/.kiro/sessions`), holding `messages.jsonl`, the session's append-only
//! log, and `session.json`, a header Kiro rewrites whole with the session's directory, model,
//! title and status. A bucket is named by a hash of the directory the session was started in;
//! the bucket `cli` holds Kiro's old (V2) sessions, which are not read.
//!
//! The log is one row per line, `{id, timestamp, payload}`, with `payload.type` saying what the
//! row is (as of Kiro CLI 2.36.0; the format is not documented). The rows that are conversation:
//! - `user`: what the user typed (`content`), or sent mid-turn (`source: "steer"`). The user rows
//!   Kiro writes around a steer for its own bookkeeping (`_meta.kiro.syntheticUserMessageReason`,
//!   `steeringClearedIds`) are skipped.
//! - `assistant`: the model's reply, one row per part (`operationType`): `Say` and `Print` are
//!   text; `Reasoning` is its thinking, which Kiro redacts to `...` (read as a marker that the
//!   model reasoned) or leaves empty (skipped); `Summary` is the summary a compaction left to
//!   stand in for the conversation before it, read as a system row, as Claude Code's and
//!   Copilot's compaction summaries are.
//! - `tool_call`: a call (`toolCallId`, `toolName`, `args`), and `tool_result` its result
//!   (`content`, a string, `success`). A denied call has a result too, with `success: false`.
//! - `agent_note`, `system` and `error`: context Kiro gave the model (the `<session_context>`
//!   block, a hook's output) and errors, read as system rows, as Claude Code's injected context
//!   is.
//! - `sub_agent_start` and `sub_agent_complete`: a subagent spawned (`subSessionId`,
//!   `subAgentName`, `prompt`) and its answer (`response`, `status`). No `tool_call` names the
//!   spawn, so the start is read as the call (id `subSessionId`, name `invoke_sub_agent`, the tool
//!   Kiro gives the model) and the end as its result, an error unless the status is `success`.
//!   The subagent's own rows are a session of their own (see Subagents).
//!
//! The rest is bookkeeping, skipped: `session_start` (the system prompt, written after the first
//! turn), `turn_start` and `turn_end` (whose `stopReason` the reader stamps on the turn's last
//! reply), `usage_summary` (credits, not tokens), `session_event`, `session_metadata`,
//! `ContextualHookInvoked` (a hook run; atuin's own shows up here), `pending_interaction` and
//! `interaction_resolved` (tool approval), `tombstone` (a compaction or rewind: the rows it covers
//! are no longer what the model sees, but stay captured, as a Copilot compaction's do),
//! `sub_agent_progress`, and the rest of Kiro's internals.
//!
//! **Context.** No row says where the session works or which model answers: the header does
//! (`workspacePaths[0]`, the directory the session was started in, wherever it is resumed from;
//! `modelId`, `auto` unless the user picked one). The reader stamps both on every row. There is
//! no git branch, and no token usage.
//!
//! **Titles.** The header's `title`, which Kiro generates after the first turn and `/rename`
//! replaces (setting `titleSetByUser`), neither of which writes a row. The reader checks the
//! header as each line arrives
//! and, when the listener gave it a watch on the header, as the header is rewritten, and yields
//! the title as a row of its own (a system row with nothing but the title, timed by the header's
//! modification time) whenever it has changed, the first line of a read included. Its id is made
//! of the title and how many title rows the read yielded before it: a rename back to an earlier
//! title is a row of its own, while a read started over (an import, a restart) names the session
//! under the same row as any other read started over, so no new row. A title row waits until no
//! reply is held (below), so its checkpoint never passes a row yet to be yielded.
//!
//! **Stop reasons.** A turn's `turn_end` carries the stop reason of the reply before it. The
//! reader holds a turn's assistant rows back until the row after them, and stamps the stop reason
//! on the last one with text or a call: `turn_end`'s when that row is the `turn_end`, `ToolUse`
//! when it is a tool's result. The held rows keep their own lines' checkpoints.
//!
//! **Subagents.** Kiro writes a subagent's rows to a log of their own,
//! `sub-executions/<subSessionId>.jsonl` under the parent's directory, in the parent's row shapes
//! (`subExecutionId` naming the subagent), ending with a `tool_call` of `subagent_response` whose
//! `args.response` is the answer the parent got. Each is read as a session of its own, a child of
//! the parent ([`ParentKind::Subagent`]) on every row, under the `subSessionId` Kiro names it by
//! (a bare uuid: the parent's id is `sess_<uuid>`, so a Kiro id without that prefix is a
//! subagent's). It takes its directory and model from the parent's header, and its title from the
//! `sub_agent_start` in the parent's log that names it (`subAgentName`, a
//! [`TitleSource::Agent`] title, read as the header's is), which Kiro writes before the
//! subagent's first row. A subagent's log is reported as the parent's is, by the watcher as it
//! appears, with a checkpoint over its own file, and listed by a scan; Kiro resumes only the
//! parent.
//!
//! **Ids and links.** Row ids are unique within the log. Rows name no parent, so the reader names
//! each row's parent itself: the conversation row before it in file order (a title row, beside
//! the log, is no one's parent and names none). The database then models the session as a tree,
//! and two machines going on from one copy as two branches, which one time-ordered line could
//! never show diverging (see `heads.rs` in atuin-client). A read resumed past a checkpoint names
//! no parent for its first row, which follows the row before it all the same. Each row's
//! `executionId` (the turn) is its `turn_id`. Timestamps are not in file order (a `turn_end` is
//! stamped before the rows written before it); rows are read in file order.
//!
//! **Forks.** A session Kiro forked (a rewind, a tangent) or atuin wrote as a fork names the
//! session it came from in its header (`parentSessionId`); the reader stamps it on every row as
//! the fork's parent (a subagent's, when `createdReason` says so).

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{self, BufRead};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use futures::future::Either;
use futures::{Stream, StreamExt};
use parking_lot::Mutex;
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::watch;
use typed_builder::TypedBuilder;

use crate::fs::tree_watcher::{FileStat, TreeWatcher};
use crate::harnesstools::kiro::{Kiro, kiro_home};
use crate::harnesstools::resume;
use crate::harnesstools::session::model::{
    Content, MessageId, ParentKind, Role, StopReason, TitleChange, TitleSource, ToolCallId,
    ToolResult, ToolUse,
};
use crate::harnesstools::session::synthetic::SYNTHETIC;
use crate::harnesstools::session::{
    Checkpoint, Listener, Message, MessageError, Observable, RuntimeError, Session, SessionId,
    Sessions, WatchError, checkpoint_offset, modified_after, scan_sessions,
};
use crate::io::{FollowLines, PathLineReader, PooledReadLines};
use crate::json::jsonl::JsonlExt;
use crate::path::{AbsPathBuf, PathBufExt as _};
use crate::sync::BlockingPool;

/// A session's log, in its directory.
pub(crate) const LOG_FILE: &str = "messages.jsonl";
/// The header beside the log, with the session's directory, model, title and status.
pub(crate) const HEADER_FILE: &str = "session.json";
/// The directory beside the log holding the subagents' logs, `<subSessionId>.jsonl` each.
pub(crate) const SUB_EXECUTIONS_DIR: &str = "sub-executions";
/// The bucket of Kiro's old (V2) store, under the same root.
const V2_BUCKET: &str = "cli";

/// The prefix of every id Kiro gives a session, `sess_<uuid>`, which names its directory.
const SESSION_PREFIX: &str = "sess_";

/// Whether `id` names a subagent's session: the `subSessionId` Kiro names a subagent by is a
/// bare uuid, the name of its log under `sub-executions/`, with no [`SESSION_PREFIX`], as
/// Copilot's subagents are told by their `agent-` prefix.
pub(crate) fn is_subagent_id(id: &str) -> bool {
    resume::is_plain_name(id) && !id.starts_with(SESSION_PREFIX)
}

#[derive(Debug, Clone, TypedBuilder)]
pub struct KiroSessions {
    #[builder(default, setter(strip_option, into))]
    root: Option<PathBuf>,
    /// Runs every file read of the sessions this finds.
    pool: BlockingPool,
}

impl KiroSessions {
    fn resolve_root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(default_root)
    }
}

/// Kiro's sessions directory: `$KIRO_HOME/sessions`, else `~/.kiro/sessions`.
pub(crate) fn default_root() -> PathBuf {
    kiro_home().joined("sessions")
}

/// The buckets under the sessions directory `root`, the V2 store's left out.
fn buckets(root: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|bucket| bucket.file_name() != V2_BUCKET)
        .map(|bucket| bucket.path())
}

/// The directory of session `id` under the sessions directory `root`: in whichever bucket holds
/// it (a session never moves buckets, however it is resumed).
pub(crate) fn session_dir(root: &Path, id: &str) -> Option<PathBuf> {
    if !resume::is_plain_name(id) {
        return None;
    }
    buckets(root).map(|bucket| bucket.joined(id)).find(|dir| dir.is_dir())
}

/// The log of session `id` under `root`, when it is there. A subagent's names nothing but its
/// file, so every session's `sub-executions/` is looked in.
pub(crate) fn locate(root: &Path, id: &str) -> Option<PathBuf> {
    if !resume::is_plain_name(id) {
        return None;
    }
    let path = if is_subagent_id(id) {
        buckets(root)
            .filter_map(|bucket| std::fs::read_dir(bucket).ok())
            .flatten()
            .filter_map(Result::ok)
            .filter(|session| session.file_name().to_string_lossy().starts_with(SESSION_PREFIX))
            .map(|session| session.path().joined(SUB_EXECUTIONS_DIR).joined(format!("{id}.jsonl")))
            .find(|log| log.is_file())?
    } else {
        session_dir(root, id)?.joined(LOG_FILE)
    };
    path.is_file().then_some(path)
}

/// The rows of the log at `path`, holding `bytes`, as a read of the whole log yields them (the
/// header's title row after the first line included), for sync to key as capture does. Blocking;
/// a line that doesn't parse is skipped, as the readers skip it.
#[cfg_attr(not(test), allow(dead_code))] // used by sync once the write side is wired
pub(crate) fn read_log(path: &Path, bytes: &[u8]) -> Vec<KiroMessage> {
    let mut header = HeaderWatch::new(path.with_file_name(HEADER_FILE));
    let title = header.refresh();
    let context = header.context();
    let mut reading = Reading::default();
    let mut out = Vec::new();
    let lines = bytes.split(|b| *b == b'\n').filter(|l| !l.iter().all(u8::is_ascii_whitespace));
    for (n, line) in lines.enumerate() {
        let Ok(row) = Row::decode(line) else {
            continue;
        };
        let checkpoint = Checkpoint::new(n as u64 + 1, line);
        out.extend(reading.absorb(checkpoint, row, &context).into_iter().map(|(_, m)| m));
        if n == 0
            && let Some((title, at)) = title.clone()
        {
            out.push(title_row(title, at, 0, None));
        }
    }
    out.extend(reading.finish().into_iter().map(|(_, m)| m));
    out
}

impl Sessions for KiroSessions {
    type Listener = KiroListener;

    fn listener(&self) -> Result<KiroListener, RuntimeError> {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(KiroListener {
            root,
            pool: self.pool.clone(),
        })
    }

    fn existing(
        &self,
    ) -> Result<impl Stream<Item = Result<KiroSession, RuntimeError>> + Send + 'static, RuntimeError>
    {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        let pool = self.pool.clone();
        Ok(async_stream::stream! {
            let sessions_pool = pool.clone();
            let scan = pool
                .run(move || {
                    let sessions = root.clone();
                    scan_sessions(root, |path, is_file| {
                        KiroListener::open_session(&sessions, path, is_file, &sessions_pool)
                    })
                })
                .await;
            match scan {
                Ok(items) => {
                    for item in items {
                        yield item;
                    }
                }
                Err(cancelled) => yield Err(RuntimeError::Io(io::Error::other(cancelled))),
            }
        })
    }
}

impl Observable for Kiro {
    type Sessions = KiroSessions;

    fn sessions(&self, pool: BlockingPool) -> KiroSessions {
        KiroSessions::builder().pool(pool).build()
    }
}

#[derive(Debug, Clone)]
pub struct KiroListener {
    root: PathBuf,
    pool: BlockingPool,
}

/// A file of a session's the reader looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionFile {
    /// `<bucket>/<session>/messages.jsonl`.
    Log,
    /// `<bucket>/<session>/session.json`.
    Header,
    /// `<bucket>/<session>/sub-executions/<subSessionId>.jsonl`, a subagent's log, with its id.
    SubExecution(String),
}

/// What `path`, under the sessions directory `root`, is to the reader, with the id of the session
/// (the directory) it belongs to; `None` for the V2 bucket, a bucket's `.index/`, and the rest of
/// a session's directory, which is not conversation (`tool-outputs/`, `snapshots/`).
fn session_file(root: &Path, path: &Path) -> Option<(SessionId, SessionFile)> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Vec<&OsStr> = relative
        .components()
        .map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let (session, file) = match parts[..] {
        [bucket, ..] if bucket == V2_BUCKET => return None,
        [_, session, file] if file == LOG_FILE => (session, SessionFile::Log),
        [_, session, file] if file == HEADER_FILE => (session, SessionFile::Header),
        [_, session, dir, file] if dir == SUB_EXECUTIONS_DIR => {
            let file = Path::new(file);
            if file.extension() != Some(OsStr::new("jsonl")) {
                return None;
            }
            let sub = file.file_stem()?.to_string_lossy().into_owned();
            (session, SessionFile::SubExecution(sub))
        }
        _ => return None,
    };
    Some((SessionId::from(session.to_string_lossy().into_owned()), file))
}

/// A session's two files, as the watcher has reported them so far.
#[derive(Debug, Default)]
struct Files {
    log: Option<(PathBuf, watch::Receiver<FileStat>)>,
    header: Option<watch::Receiver<FileStat>>,
}

impl KiroListener {
    /// Build a read-once session for a log under `root` (no change signal), a parent's or a
    /// subagent's, or `None`.
    fn open_session(
        root: &Path,
        path: &Path,
        is_file: bool,
        pool: &BlockingPool,
    ) -> Option<KiroSession> {
        if !is_file {
            return None;
        }
        match session_file(root, path)? {
            (id, SessionFile::Log) => Some(KiroSession::open(id, path.to_path_buf(), pool.clone())),
            (parent, SessionFile::SubExecution(sub)) => {
                Some(KiroSession::open_child(parent, &sub, path.to_path_buf(), pool.clone()))
            }
            (_, SessionFile::Header) => None,
        }
    }
}

impl Listener for KiroListener {
    type Session = KiroSession;

    /// Every log as the watcher reports it, with a watch on the header beside it when the
    /// watcher has reported that too. The watcher lists a directory in no set order, so a header
    /// reported after its log reports the session again, now with the header watched; the
    /// consumer replaces its stream, as it does for a log made again. A subagent's log is
    /// reported as it appears, a session of its own under the parent whose directory holds it.
    fn watch(self) -> impl Stream<Item = Result<KiroSession, WatchError>> + Send + 'static {
        let root = self.root;
        let pool = self.pool;
        async_stream::stream! {
            // The watcher reports paths under the canonical root.
            let sessions = AbsPathBuf::canonicalize(&root).map_or_else(|_| root.clone(), PathBuf::from);
            let files = {
                let sessions = sessions.clone();
                TreeWatcher::builder()
                    .filter(move |path| session_file(&sessions, path).is_some())
                    .watch(&root)
            };
            let files = match files {
                Ok(files) => files,
                Err(err) => {
                    yield Err(WatchError::from(err));
                    return;
                }
            };
            // ponytail: entries are kept for every session ever seen; prune on end if the map
            // ever matters.
            let mut seen: HashMap<PathBuf, Files> = HashMap::new();
            for await file in files {
                let (path, stat) = file.into_parts();
                let Some((id, kind)) = session_file(&sessions, &path) else { continue };
                let Some(dir) = path.parent() else { continue };
                let files = seen.entry(dir.to_path_buf()).or_default();
                match kind {
                    SessionFile::Header => files.header = Some(stat),
                    SessionFile::Log => files.log = Some((path.to_path_buf(), stat)),
                    SessionFile::SubExecution(sub) => {
                        yield Ok(KiroSession {
                            changes: Some(stat),
                            ..KiroSession::open_child(id, &sub, path.to_path_buf(), pool.clone())
                        });
                        continue;
                    }
                }
                let Some((log, changes)) = &files.log else { continue };
                yield Ok(KiroSession {
                    id,
                    path: log.clone(),
                    changes: Some(changes.clone()),
                    header: files.header.clone(),
                    parent: None,
                    pool: pool.clone(),
                });
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct KiroSession {
    id: SessionId,
    /// The log.
    path: PathBuf,
    /// Wakes [`messages`](Session::messages) on each change to the log; `None` reads it once.
    changes: Option<watch::Receiver<FileStat>>,
    /// Wakes the reader when the header is rewritten (a title generated or changed after the
    /// log's last line); `None` checks the header only as lines come.
    header: Option<watch::Receiver<FileStat>>,
    /// For a subagent's session, the session that spawned it, whose directory holds the log.
    parent: Option<SessionId>,
    pool: BlockingPool,
}

impl KiroSession {
    /// A session over the log as it stands: [`messages`](Session::messages) ends at its end.
    #[must_use]
    pub fn open(id: SessionId, path: PathBuf, pool: BlockingPool) -> Self {
        Self {
            id,
            path,
            changes: None,
            header: None,
            parent: None,
            pool,
        }
    }

    /// The session of subagent `sub` of `parent`, over its log `path` (under the parent's
    /// directory) as it stands.
    #[must_use]
    pub fn open_child(parent: SessionId, sub: &str, path: PathBuf, pool: BlockingPool) -> Self {
        Self {
            parent: Some(parent),
            ..Self::open(SessionId::from(sub.to_owned()), path, pool)
        }
    }

    /// The parent's directory: the log's own for a parent, the one above `sub-executions/` for
    /// a subagent.
    fn session_dir(&self) -> &Path {
        let dir = self.path.parent().unwrap_or(Path::new(""));
        match self.parent {
            Some(_) => dir.parent().unwrap_or(dir),
            None => dir,
        }
    }

    /// The messages of the log from byte `start`, each with the checkpoint just past its line,
    /// following the log when `changes` is given.
    fn messages_at(
        self,
        start: u64,
        over: bool,
    ) -> impl Stream<Item = Result<(Checkpoint, KiroMessage), MessageError>> + Send + 'static {
        async_stream::stream! {
            let header = Arc::new(Mutex::new(Beside::of(&self)));
            // The header first: every row carries the directory it names. Its title waits for
            // the first line, whose checkpoint it goes with.
            let watch = Arc::clone(&header);
            let mut pending_title = self.pool.run(move || watch.lock().refresh()).await.ok().flatten();
            let lines = FollowLines::new(PooledReadLines::new(
                PathLineReader::at(&self.path, start),
                self.pool.clone(),
            ));
            let rows = match self.changes.clone() {
                Some(changes) => lines.follow(changes).left_stream(),
                None => lines.read_to_end().right_stream(),
            }
            .json_with(Row::decode);
            let rewrites = header_rewrites(self.header.clone());
            futures::pin_mut!(rows);
            futures::pin_mut!(rewrites);
            let mut reading = Reading::default();
            let mut generation = None;
            // The checkpoint of the last line read, which a title row between lines goes with.
            let mut last: Option<Checkpoint> = None;
            // The title rows yielded so far in this read: part of the next one's id.
            let mut titled: u32 = 0;
            let mut rewrites_done = false;
            loop {
                tokio::select! {
                    biased;
                    item = rows.next() => {
                        let Some(item) = item else {
                            for released in reading.finish() {
                                yield Ok(released);
                            }
                            if let (Some((title, at)), Some(checkpoint)) = (pending_title.take(), last) {
                                yield Ok((checkpoint, title_row(title, at, titled, self.parent.clone())));
                            }
                            break;
                        };
                        let (line, row) = match item {
                            Ok(item) => item,
                            Err(err) => {
                                yield Err(MessageError::from(err));
                                continue;
                            }
                        };
                        // Started over from the log's first byte: nothing read before counts,
                        // and the session is named again.
                        if generation.replace(line.generation).is_some_and(|seen| seen != line.generation) {
                            reading = Reading::default();
                            pending_title = header.lock().current();
                            titled = 0;
                        }
                        let checkpoint = Checkpoint::after(&line).started_over(over);
                        let context = header.lock().context();
                        for message in reading.absorb(checkpoint, row, &context) {
                            yield Ok(message);
                        }
                        last = Some(checkpoint);
                        // The header after the line's own row, so a crash between the two loses
                        // only the title row, which the next read names again.
                        let watch = Arc::clone(&header);
                        if let Some(renamed) = self.pool.run(move || watch.lock().refresh()).await.ok().flatten() {
                            pending_title = Some(renamed);
                        }
                        // Not while a reply is held: the title's checkpoint would pass it.
                        if !reading.holding()
                            && let Some((title, at)) = pending_title.take()
                        {
                            yield Ok((checkpoint, title_row(title, at, titled, self.parent.clone())));
                            titled += 1;
                        }
                    }
                    rewritten = rewrites.next(), if !rewrites_done => match rewritten {
                        Some(()) => {
                            let watch = Arc::clone(&header);
                            if let Some(renamed) = self.pool.run(move || watch.lock().refresh()).await.ok().flatten() {
                                pending_title = Some(renamed);
                            }
                            // With a line to go with, and no reply held; else the next line
                            // brings it.
                            if !reading.holding()
                                && let Some(checkpoint) = last
                                && let Some((title, at)) = pending_title.take()
                            {
                                yield Ok((checkpoint, title_row(title, at, titled, self.parent.clone())));
                                titled += 1;
                            }
                        }
                        None => rewrites_done = true,
                    },
                }
            }
        }
    }
}

/// How often a followed header is looked at between the watcher's reports of it.
const HEADER_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// A unit for each rewrite of the header `changes` watches, and one every [`HEADER_POLL`]
/// besides; nothing, for no watch.
///
/// The poll backs the watcher's events: a rewrite can go unreported (on Windows, a stat failing
/// mid-replace makes the watcher forget the file until its next scan, closing `changes`), and
/// a check that finds the header unchanged costs a stat.
fn header_rewrites(
    changes: Option<watch::Receiver<FileStat>>,
) -> impl Stream<Item = ()> + Send + 'static {
    async_stream::stream! {
        let Some(mut changes) = changes else {
            return;
        };
        let mut poll = tokio::time::interval(HEADER_POLL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        poll.tick().await;
        let mut reported = true;
        loop {
            tokio::select! {
                changed = changes.changed(), if reported => match changed {
                    Ok(()) => yield (),
                    Err(_) => reported = false,
                },
                _ = poll.tick() => yield (),
            }
        }
    }
}

impl Session for KiroSession {
    type Message = KiroMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages_from(
        self,
        from: Option<Checkpoint>,
    ) -> impl Stream<Item = Result<(Checkpoint, KiroMessage), MessageError>> + Send + 'static {
        async_stream::stream! {
            let start = match from {
                Some(from) => checkpoint_offset(&self.path, &self.pool, from).await,
                None => 0,
            };
            // A checkpoint names a line's end, never 0: read from there, it no longer held.
            let over = from.is_some() && start == 0;
            for await item in self.messages_at(start, over) {
                yield item;
            }
        }
    }

    fn read(&self) -> impl Stream<Item = Result<KiroMessage, MessageError>> + Send + 'static {
        Self {
            changes: None,
            header: None,
            ..self.clone()
        }
        .messages_at(0, false)
        .map(|item| item.map(|(_, message)| message))
    }

    /// Once the log changes after `since`, or the header does (a title given or changed while
    /// the log stands still), whichever is watched; at once with no watch on the log.
    async fn changed_after(mut self, since: SystemTime) -> Option<Self> {
        match (self.changes.as_mut(), self.header.as_mut()) {
            (Some(log), Some(header)) => {
                let log = modified_after(Some(log), since);
                let header = modified_after(Some(header), since);
                futures::pin_mut!(log);
                futures::pin_mut!(header);
                match futures::future::select(log, header).await {
                    Either::Left((Some(()), _)) | Either::Right((Some(()), _)) => {}
                    // One watch ended: the other may still wake.
                    Either::Left((None, header)) => header.await?,
                    Either::Right((None, log)) => log.await?,
                }
            }
            (log, _) => modified_after(log, since).await?,
        }
        Some(self)
    }
}

/// One line of the log: its envelope, with the payload kept as JSON and read as needed, so a
/// line whose shape a newer Kiro changed still parses.
#[derive(Debug, Clone, Deserialize)]
struct Row {
    id: Option<String>,
    timestamp: Option<String>,
    #[serde(default)]
    payload: serde_json::Value,
}

impl Row {
    /// A line, read as Kiro (a Node program) wrote it (see [`crate::json::js`]).
    fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        crate::json::js::from_slice(bytes)
    }

    fn kind(&self) -> &str {
        self.payload["type"].as_str().unwrap_or_default()
    }

    fn str(&self, field: &str) -> Option<&str> {
        self.payload[field].as_str()
    }

    fn text(&self, field: &str) -> Option<&str> {
        self.str(field).filter(|text| !text.is_empty())
    }

    /// A user row Kiro wrote for its own bookkeeping around a steer, not what the user said.
    fn synthetic(&self) -> bool {
        let meta = &self.payload["_meta"]["kiro"];
        !meta["syntheticUserMessageReason"].is_null() || !meta["steeringClearedIds"].is_null()
    }
}

/// What is stamped on every row: what the header says of the session, and for a subagent, its
/// parent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Context {
    cwd: Option<PathBuf>,
    model: Option<String>,
    /// The session this one was forked from (`parentSessionId`) or works for (a subagent's
    /// parent), and which.
    parent: Option<(SessionId, ParentKind)>,
}

/// What the reader carries from one row to the ones after it.
#[derive(Debug, Default)]
struct Reading {
    /// A turn's assistant rows since the last row that wasn't one, held for the turn's stop
    /// reason (see the module docs), each with its own line's checkpoint.
    held: Vec<(Checkpoint, KiroMessage)>,
    /// The id of the last conversation row, which the next names as its parent.
    last: Option<String>,
}

impl Reading {
    /// Take `row` in: the messages it yields, in file order, which are the rows it releases and
    /// then its own, or nothing for a row the reader skips or holds.
    fn absorb(
        &mut self,
        checkpoint: Checkpoint,
        row: Row,
        context: &Context,
    ) -> Vec<(Checkpoint, KiroMessage)> {
        if row.kind() == "turn_end" {
            self.stamp(stop_reason(row.str("stopReason")));
            return self.release();
        }
        let Some(mut message) = KiroMessage::of(row, context) else {
            return Vec::new();
        };
        message.parent_row = std::mem::replace(&mut self.last, message.row.id.clone());
        let mut out = match message.role() {
            Role::Assistant => {
                self.held.push((checkpoint, message));
                return Vec::new();
            }
            Role::Tool => {
                self.stamp(Some(StopReason::ToolUse));
                self.release()
            }
            _ => self.release(),
        };
        out.push((checkpoint, message));
        out
    }

    /// The rows still held at the log's end.
    fn finish(&mut self) -> Vec<(Checkpoint, KiroMessage)> {
        self.release()
    }

    /// Whether a reply is held back, so no checkpoint past its line may be yielded yet.
    fn holding(&self) -> bool {
        !self.held.is_empty()
    }

    /// Give the held reply (the last held row with text or a call, else the last held row) its
    /// stop reason.
    fn stamp(&mut self, stop: Option<StopReason>) {
        let reply = match self.held.iter().rposition(|(_, m)| m.carries_conversation()) {
            Some(at) => self.held.get_mut(at),
            None => self.held.last_mut(),
        };
        if let Some((_, reply)) = reply {
            reply.stop_reason = stop;
        }
    }

    fn release(&mut self) -> Vec<(Checkpoint, KiroMessage)> {
        std::mem::take(&mut self.held)
    }
}

/// A `turn_end`'s `stopReason` (the model API's: `end_turn` is the one seen).
fn stop_reason(raw: Option<&str>) -> Option<StopReason> {
    Some(match raw? {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "refusal" => StopReason::Refusal,
        "aborted" | "cancelled" => StopReason::Aborted,
        "error" => StopReason::Error,
        other => StopReason::Other(other.to_owned()),
    })
}

/// The kind of the reader's own title rows, which no Kiro row has: their role, as Claude Code's
/// title lines are read under theirs.
const TITLE_ROW: &str = "atuin-title";

/// A row carrying `title`, which the header (or for a subagent of `parent`, its spawn) gave the
/// session at `at`, the `nth` title row of its read (see the module docs): the same title read
/// again from the start is the same row, and a title the session is renamed back to is a new one.
fn title_row(
    title: TitleChange,
    at: Option<SystemTime>,
    nth: u32,
    parent: Option<SessionId>,
) -> KiroMessage {
    let mut keyed = title.text.as_deref().unwrap_or_default().as_bytes().to_vec();
    keyed.extend_from_slice(&nth.to_le_bytes());
    let id = format!("{SYNTHETIC}title-{:016x}", xxhash_rust::xxh3::xxh3_64(&keyed));
    KiroMessage {
        row: Row {
            id: Some(id),
            timestamp: None,
            payload: serde_json::json!({"type": TITLE_ROW}),
        },
        cwd: None,
        model: None,
        parent: parent.map(|parent| (parent, ParentKind::Subagent)),
        parent_row: None,
        stop_reason: None,
        title: Some(title),
        at: at.map(OffsetDateTime::from),
    }
}

/// What the reader looks at beside the log: the header, which says where the session works and,
/// for a parent, names it; and for a subagent, the parent's log, whose `sub_agent_start` names it.
#[derive(Debug)]
struct Beside {
    header: HeaderWatch,
    spawn: Option<SpawnWatch>,
}

impl Beside {
    fn of(session: &KiroSession) -> Self {
        let dir = session.session_dir();
        let spawn = session.parent.clone().map(|parent| SpawnWatch {
            log: dir.join(LOG_FILE),
            sub: session.path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
            parent,
            scanned: None,
            found: None,
        });
        Self {
            header: HeaderWatch::new(dir.join(HEADER_FILE)),
            spawn,
        }
    }

    /// The title the session is now given, and when, when it changed since the last call (or on
    /// the first; see [`HeaderWatch::refresh`] and [`SpawnWatch::refresh`]). Blocking.
    fn refresh(&mut self) -> Option<(TitleChange, Option<SystemTime>)> {
        let header = self.header.refresh();
        match &mut self.spawn {
            Some(spawn) => spawn.refresh(),
            None => header,
        }
    }

    /// The title as last read, with when it was given.
    fn current(&self) -> Option<(TitleChange, Option<SystemTime>)> {
        match &self.spawn {
            Some(spawn) => spawn.current(),
            None => self.header.current(),
        }
    }

    fn context(&self) -> Context {
        let mut context = self.header.context();
        if let Some(spawn) = &self.spawn {
            context.parent = Some((spawn.parent.clone(), ParentKind::Subagent));
        }
        context
    }
}

/// Follows the parent's log for the `sub_agent_start` naming the subagent `sub`, which Kiro
/// writes before the subagent's first row; nothing orders the two files for a reader, though, so
/// the log is read again as it grows until the row is there.
#[derive(Debug)]
struct SpawnWatch {
    /// The parent's log.
    log: PathBuf,
    sub: String,
    parent: SessionId,
    /// The log's length when last read: read again only once it has grown.
    scanned: Option<u64>,
    /// The subagent's name and when it was spawned, once its start row is read.
    found: Option<(TitleChange, Option<SystemTime>)>,
}

impl SpawnWatch {
    /// The subagent's name and when it was spawned, on the call that finds its start row: `None`
    /// before, and after. Blocking.
    fn refresh(&mut self) -> Option<(TitleChange, Option<SystemTime>)> {
        if self.found.is_some() {
            return None;
        }
        let len = std::fs::metadata(&self.log).ok()?.len();
        if self.scanned == Some(len) {
            return None;
        }
        self.scanned = Some(len);
        self.found = spawn_of(&self.log, &self.sub);
        self.found.clone()
    }

    fn current(&self) -> Option<(TitleChange, Option<SystemTime>)> {
        self.found.clone()
    }
}

/// The name the `sub_agent_start` row in the log at `path` gives subagent `sub`, and when it was
/// written; `None` with no such row yet, or no name on it. A line that doesn't parse (the last,
/// half written) is passed over. Blocking.
fn spawn_of(path: &Path, sub: &str) -> Option<(TitleChange, Option<SystemTime>)> {
    let log = io::BufReader::new(std::fs::File::open(path).ok()?);
    log.split(b'\n')
        .map_while(Result::ok)
        .filter_map(|line| Row::decode(&line).ok())
        .find(|row| row.kind() == "sub_agent_start" && row.str(SUB_SESSION_ID) == Some(sub))
        .and_then(|row| {
            let name = row.text("subAgentName")?;
            let at = row
                .timestamp
                .as_deref()
                .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
                .map(SystemTime::from);
            Some((TitleChange::new(TitleSource::Agent, name), at))
        })
}

/// The header, as far as the reader needs it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Header {
    context: Context,
    title: Option<String>,
    /// `titleSetByUser`: the title is the user's (`/rename`), not the generated one.
    user_titled: bool,
}

impl Header {
    fn parse(text: &str) -> serde_json::Result<Self> {
        let header: serde_json::Value = serde_json::from_str(text)?;
        // Kiro sets `parentSessionId` on a fork (a rewind, a tangent) and on a session it ran
        // for another (`createdReason`); a session atuin forks names its original there too.
        let parent = header["parentSessionId"].as_str().map(|parent| {
            let kind = match header["createdReason"].as_str() {
                Some("subagent") => ParentKind::Subagent,
                _ => ParentKind::Fork,
            };
            (SessionId::from(parent.to_owned()), kind)
        });
        Ok(Self {
            context: Context {
                cwd: header["workspacePaths"][0].as_str().map(PathBuf::from),
                model: header["modelId"].as_str().map(str::to_owned),
                parent,
            },
            title: header["title"].as_str().filter(|t| !t.trim().is_empty()).map(str::to_owned),
            user_titled: header["titleSetByUser"].as_bool() == Some(true),
        })
    }

    /// The session's title: the user's when they set it, else Kiro's.
    fn title(&self) -> Option<TitleChange> {
        let source = if self.user_titled {
            TitleSource::Named
        } else {
            TitleSource::Generated
        };
        self.title.as_deref().map(|title| TitleChange::new(source, title))
    }
}

/// Follows the header: what it said when last read, re-read when the file changes.
#[derive(Debug)]
struct HeaderWatch {
    path: PathBuf,
    modified: Option<SystemTime>,
    header: Header,
    title: Option<TitleChange>,
}

impl HeaderWatch {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            modified: None,
            header: Header::default(),
            title: None,
        }
    }

    /// The title the header now gives, and when it was written, when the title changed since
    /// the last call (or on the first): `None` when it hasn't, or the header can't be read. Kiro
    /// rewrites the header in place, so a read can catch it half written: that read says
    /// nothing, and the next tries again. Blocking.
    ///
    /// The header is read every time, not only when its modification time moved: Windows
    /// reported the same time for two rewrites 300 ms apart (seen on CI), which would hide the
    /// second for good. The file is a few kilobytes.
    fn refresh(&mut self) -> Option<(TitleChange, Option<SystemTime>)> {
        let modified = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        let header = Header::parse(&std::fs::read_to_string(&self.path).ok()?).ok()?;
        self.modified = modified;
        self.header = header;
        let title = self.header.title();
        if title == self.title {
            return None;
        }
        self.title = title;
        self.current()
    }

    /// The title as last read, with when the header was written.
    fn current(&self) -> Option<(TitleChange, Option<SystemTime>)> {
        self.title.clone().map(|title| (title, self.modified))
    }

    fn context(&self) -> Context {
        self.header.context.clone()
    }
}

/// One conversation row of a Kiro session's log (see the module docs).
#[derive(Debug, Clone)]
pub struct KiroMessage {
    row: Row,
    /// Where the session works, as the header says.
    cwd: Option<PathBuf>,
    /// The model the header names, on the model's rows.
    model: Option<String>,
    /// The session this one was forked from or works for, as the header or its spawn says.
    parent: Option<(SessionId, ParentKind)>,
    /// The conversation row before this one in the log (see the module docs).
    parent_row: Option<String>,
    /// On a turn's last reply, from its `turn_end` (or `ToolUse`, before a tool's result).
    stop_reason: Option<StopReason>,
    /// On a title row ([`TITLE_ROW`]), the session's title from the header.
    title: Option<TitleChange>,
    /// When a title row's header was written: the row's time.
    at: Option<OffsetDateTime>,
}

impl KiroMessage {
    /// `row` as a message, or `None` for a row the reader skips.
    fn of(row: Row, context: &Context) -> Option<Self> {
        let kept = match row.kind() {
            "user" => !row.synthetic() && row.text("content").is_some(),
            "assistant" => row.text("content").is_some(),
            "tool_call" | "tool_result" | "agent_note" | "system" | "error" => true,
            // The call and result stand on the subagent's id, which pairs them.
            "sub_agent_start" | "sub_agent_complete" => row.text(SUB_SESSION_ID).is_some(),
            _ => false,
        };
        kept.then(|| Self {
            cwd: context.cwd.clone(),
            model: context.model.clone(),
            parent: context.parent.clone(),
            parent_row: None,
            row,
            stop_reason: None,
            title: None,
            at: None,
        })
    }

    fn kind(&self) -> &str {
        self.row.kind()
    }

    fn operation(&self) -> Option<&str> {
        self.row.str("operationType")
    }

    /// Whether the row is a reply with something in it the user sees, or a call: text, not a
    /// reasoning marker.
    fn carries_conversation(&self) -> bool {
        match self.kind() {
            "tool_call" | "sub_agent_start" => true,
            "assistant" => self.operation() != Some("Reasoning"),
            _ => false,
        }
    }

    fn models_row(&self) -> bool {
        matches!(self.kind(), "assistant" | "tool_call" | "sub_agent_start")
    }
}

/// The field naming a subagent on the rows of its spawn, in the parent's log, and on its own
/// rows (as `subExecutionId`): the name of its file under `sub-executions/`.
const SUB_SESSION_ID: &str = "subSessionId";

/// The tool Kiro gives the model to spawn a subagent, which a `sub_agent_start` stands in for.
const SPAWN_TOOL: &str = "invoke_sub_agent";

impl Message for KiroMessage {
    fn id(&self) -> Option<MessageId> {
        self.row.id.clone().map(MessageId::from)
    }

    fn role(&self) -> Role {
        match self.kind() {
            "user" => Role::User,
            "assistant" if self.operation() == Some("Summary") => Role::System,
            "assistant" | "tool_call" | "sub_agent_start" => Role::Assistant,
            "tool_result" | "sub_agent_complete" => Role::Tool,
            "agent_note" | "system" | "error" => Role::System,
            other => Role::Other(other.to_owned()),
        }
    }

    fn parent_id(&self) -> Option<MessageId> {
        self.parent_row.clone().map(MessageId::from)
    }

    fn parent_session(&self) -> Option<SessionId> {
        self.parent.as_ref().map(|(session, _)| session.clone())
    }

    fn parent_kind(&self) -> Option<ParentKind> {
        self.parent.as_ref().map(|(_, kind)| *kind)
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.at.or_else(|| {
            self.row.timestamp.as_deref().and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        })
    }

    fn content(&self) -> Vec<Content> {
        let text = |field: &str| self.row.str(field).map(str::to_owned);
        let content = match self.kind() {
            "user" | "agent_note" | "system" => text("content").map(Content::Text),
            "assistant" => text("content").map(|content| match self.operation() {
                // Kiro redacts the model's reasoning to `...`: it reasoned, that is all.
                Some("Reasoning") if content == "..." => Content::ReasoningSummary { tokens: None },
                Some("Reasoning") => Content::Reasoning(content),
                Some("Summary") => Content::Summary(content),
                _ => Content::Text(content),
            }),
            "tool_call" => Some(Content::ToolUse(ToolUse {
                id: ToolCallId::from(self.row.str("toolCallId").unwrap_or_default().to_owned()),
                name: self.row.str("toolName").unwrap_or_default().to_owned(),
                input: self.row.payload["args"].clone(),
            })),
            "tool_result" => Some(Content::ToolResult(ToolResult {
                call: ToolCallId::from(self.row.str("toolCallId").unwrap_or_default().to_owned()),
                output: self.row.payload["content"].clone(),
                error: self.row.payload["success"].as_bool() == Some(false),
            })),
            "sub_agent_start" => Some(Content::ToolUse(ToolUse {
                id: ToolCallId::from(self.row.str(SUB_SESSION_ID).unwrap_or_default().to_owned()),
                name: SPAWN_TOOL.to_owned(),
                input: serde_json::json!({
                    "name": self.row.payload["subAgentName"],
                    "prompt": self.row.payload["prompt"],
                }),
            })),
            "sub_agent_complete" => Some(Content::ToolResult(ToolResult {
                call: ToolCallId::from(self.row.str(SUB_SESSION_ID).unwrap_or_default().to_owned()),
                output: match self.row.str("status") {
                    Some("success") | None => self.row.payload["response"].clone(),
                    // The answer, when there is one, over the status alone.
                    Some(status) => self
                        .row
                        .text("errorMessage")
                        .or_else(|| self.row.text("response"))
                        .map_or_else(|| serde_json::json!(status), |text| serde_json::json!(text)),
                },
                error: self.row.str("status").is_some_and(|status| status != "success"),
            })),
            "error" => Some(Content::Error(
                text("message").or_else(|| text("code")).unwrap_or_else(|| "error".to_owned()),
            )),
            _ => None,
        };
        content.into_iter().collect()
    }

    fn model(&self) -> Option<String> {
        self.models_row().then(|| self.model.clone()).flatten()
    }

    fn stop_reason(&self) -> Option<StopReason> {
        self.stop_reason.clone()
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.cwd.clone()
    }

    /// The turn (`executionId`), which every row of the turn after the prompt carries; a
    /// subagent's spawn and answer name it as `parentExecutionId`.
    fn turn_id(&self) -> Option<String> {
        self.row.str("executionId").or_else(|| self.row.str("parentExecutionId")).map(str::to_owned)
    }

    fn title(&self) -> Option<TitleChange> {
        self.title.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use futures::TryStreamExt;
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    const RESUMED: &str = include_str!("../../../tests/fixtures/kiro/resumed.jsonl");
    const RESUMED_HEADER: &str = include_str!("../../../tests/fixtures/kiro/resumed.session.json");
    const PROBE: &str = include_str!("../../../tests/fixtures/kiro/probe.jsonl");
    const PROBE_HEADER: &str = include_str!("../../../tests/fixtures/kiro/probe.session.json");
    const DENIED: &str = include_str!("../../../tests/fixtures/kiro/denied.jsonl");
    const COMPACTED: &str = include_str!("../../../tests/fixtures/kiro/compacted.jsonl");
    const COMPACTED_HEADER: &str =
        include_str!("../../../tests/fixtures/kiro/compacted.session.json");
    /// The log of the subagent the compacted session spawned, `sub-executions/<SUBAGENT>.jsonl`.
    const COMPACTED_SUBAGENT: &str =
        include_str!("../../../tests/fixtures/kiro/compacted.subagent.jsonl");

    const RESUMED_ID: &str = "sess_7c362b91-6e72-4c10-be1f-76a31a8d5f48";

    fn pool() -> BlockingPool {
        BlockingPool::new(NonZeroUsize::MIN)
    }

    /// A sessions directory holding one session, `id`, in one bucket, with `log` and `header`.
    fn root_with(id: &str, log: &str, header: Option<&str>) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("c15e787aea4828dd").joined(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(LOG_FILE), log).unwrap();
        if let Some(header) = header {
            std::fs::write(dir.join(HEADER_FILE), header).unwrap();
        }
        root
    }

    /// Write the log of subagent `sub` of session `id` under `root`: its path.
    fn with_subagent(root: &Path, id: &str, sub: &str, log: &str) -> PathBuf {
        let dir = session_dir(root, id).expect("the parent is under the root");
        let dir = dir.joined(SUB_EXECUTIONS_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.joined(format!("{sub}.jsonl"));
        std::fs::write(&path, log).unwrap();
        path
    }

    /// The session `id` under `root` (a parent's or a subagent's), read once.
    fn open(root: &Path, id: &str) -> KiroSession {
        let path = locate(root, id).expect("the session is under the root");
        if is_subagent_id(id) {
            // The parent's directory holds `sub-executions/`.
            let parent = path.parent().unwrap().parent().unwrap().file_name().unwrap();
            let parent = SessionId::from(parent.to_string_lossy().into_owned());
            KiroSession::open_child(parent, id, path, pool())
        } else {
            KiroSession::open(SessionId::from(id.to_owned()), path, pool())
        }
    }

    async fn read_all(root: &Path, id: &str) -> Vec<KiroMessage> {
        open(root, id).messages().try_collect().await.unwrap()
    }

    /// The messages of a log, read without a header.
    fn parse(log: &str) -> Vec<KiroMessage> {
        let mut reading = Reading::default();
        let context = Context::default();
        let mut out: Vec<KiroMessage> = log
            .lines()
            .enumerate()
            .flat_map(|(n, line)| {
                let checkpoint = Checkpoint::new(n as u64 + 1, line.as_bytes());
                reading.absorb(checkpoint, Row::decode(line.as_bytes()).unwrap(), &context)
            })
            .map(|(_, message)| message)
            .collect();
        out.extend(reading.finish().into_iter().map(|(_, message)| message));
        out
    }

    fn texts(messages: &[KiroMessage], role: &Role) -> Vec<String> {
        messages
            .iter()
            .filter(|m| m.role() == *role)
            .flat_map(|m| {
                m.content().into_iter().filter_map(|c| match c {
                    Content::Text(text) => Some(text),
                    _ => None,
                })
            })
            .collect()
    }

    fn row(payload: serde_json::Value) -> Row {
        let mut line = json!({"id": "r1", "timestamp": "2026-10-30T17:55:12.610Z"});
        line["payload"] = payload;
        Row::decode(line.to_string().as_bytes()).unwrap()
    }

    /// A session's log, header and subagents' logs are its files, named by its directory; the
    /// rest of what Kiro keeps is not.
    #[rstest]
    #[case::log("c15e787aea4828dd/sess_1/messages.jsonl", Some(SessionFile::Log))]
    #[case::header("c15e787aea4828dd/sess_1/session.json", Some(SessionFile::Header))]
    #[case::subagent("c15e787aea4828dd/sess_1/sub-executions/x.jsonl", Some(SessionFile::SubExecution("x".to_owned())))]
    #[case::v2("cli/sess_1/messages.jsonl", None)]
    #[case::v2_header("cli/sess_1/session.json", None)]
    #[case::v2_subagent("cli/sess_1/sub-executions/x.jsonl", None)]
    #[case::index("c15e787aea4828dd/.index/index.json", None)]
    #[case::at_root("messages.jsonl", None)]
    #[case::tool_output("c15e787aea4828dd/sess_1/tool-outputs/read_file-0a1b2c3d.txt", None)]
    #[case::snapshot("c15e787aea4828dd/sess_1/snapshots/x/messages.jsonl", None)]
    #[case::not_a_log("c15e787aea4828dd/sess_1/sub-executions/x.txt", None)]
    #[case::too_deep("c15e787aea4828dd/sess_1/sub-executions/x/y.jsonl", None)]
    fn a_sessions_files_are_named_by_its_directory(
        #[case] relative: &str,
        #[case] want: Option<SessionFile>,
    ) {
        let root = Path::new("/sessions");
        let found = session_file(root, &root.join(relative));
        assert_eq!(found.as_ref().map(|(_, file)| file), want.as_ref());
        assert!(found.is_none_or(|(id, _)| id.as_ref() == "sess_1"));
        assert_eq!(session_file(root, Path::new("/elsewhere/b/sess_1/messages.jsonl")), None);
    }

    /// A subagent's id is the bare id Kiro files it under; a session's has Kiro's prefix.
    #[rstest]
    #[case::session("sess_7c362b91-6e72-4c10-be1f-76a31a8d5f48", false)]
    #[case::subagent("5d13f3d7-73ae-431d-847f-04729b40503e", true)]
    #[case::empty("", false)]
    #[case::escaping("../x", false)]
    #[case::a_path("a/b", false)]
    fn a_subagents_id_has_no_session_prefix(#[case] id: &str, #[case] want: bool) {
        assert_eq!(is_subagent_id(id), want);
    }

    #[rstest]
    fn locate_finds_the_log_in_its_bucket() {
        let root = root_with(RESUMED_ID, RESUMED, None);
        let dir = root.path().join("c15e787aea4828dd").joined(RESUMED_ID);
        assert_eq!(locate(root.path(), RESUMED_ID), Some(dir.joined(LOG_FILE)));
        assert_eq!(locate(root.path(), "sess_missing"), None);
        assert_eq!(locate(root.path(), "../c15e787aea4828dd"), None, "no path escapes the root");
        // The V2 store is not read.
        let v2 = root.path().join(V2_BUCKET).joined("sess_old");
        std::fs::create_dir_all(&v2).unwrap();
        std::fs::write(v2.join(LOG_FILE), RESUMED).unwrap();
        assert_eq!(locate(root.path(), "sess_old"), None);
        // A subagent's log, by its id alone, under whichever session holds it; not a V2
        // session's.
        let sub = with_subagent(root.path(), RESUMED_ID, "x", PROBE);
        assert_eq!(locate(root.path(), "x"), Some(sub));
        assert_eq!(locate(root.path(), "y"), None);
        std::fs::create_dir_all(v2.join(SUB_EXECUTIONS_DIR)).unwrap();
        std::fs::write(v2.join(SUB_EXECUTIONS_DIR).joined("z.jsonl"), PROBE).unwrap();
        assert_eq!(locate(root.path(), "z"), None);
    }

    /// The conversation in file order: three prompts (the last two from another directory),
    /// the replies, a read and a shell call with their results, and the context Kiro injected
    /// (the session context block, the output of a hook) as system rows.
    #[rstest]
    fn keeps_the_conversation_in_file_order() {
        let messages = parse(RESUMED);
        let roles: Vec<_> = messages.iter().map(Message::role).collect();
        assert_eq!(roles, [
            Role::System,
            Role::User,
            Role::System,
            Role::Assistant,
            Role::Assistant,
            Role::Tool,
            Role::Assistant,
            Role::Assistant,
            Role::Tool,
            Role::Assistant,
            Role::Assistant,
            Role::User,
            Role::System,
            Role::Assistant,
            Role::Assistant,
            Role::User,
            Role::System,
            Role::Assistant,
            Role::Assistant,
        ]);
        let user = texts(&messages, &Role::User);
        assert!(user[0].starts_with("Please do two small things"), "{user:?}");
        assert_eq!(&user[1..], ["Reply with exactly: elsewhere", "Reply with exactly: noflag"]);
        let system = texts(&messages, &Role::System);
        assert!(system[0].starts_with("<session_context>"), "{system:?}");
        assert_eq!(system[1], "Output:\n\n\nExit Code: 0");
        assert_eq!(texts(&messages, &Role::Assistant).last().map(String::as_str), Some("noflag"));
        let ids: std::collections::HashSet<_> = messages.iter().map(|m| m.id().unwrap()).collect();
        assert_eq!(ids.len(), messages.len(), "ids are unique");
        assert!(messages.iter().all(|m| m.timestamp().is_some()));
    }

    #[rstest]
    fn calls_and_results_name_each_other() {
        let messages = parse(RESUMED);
        let calls: Vec<_> = messages
            .iter()
            .filter_map(|m| match m.content().first() {
                Some(Content::ToolUse(call)) => Some(call.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].input["path"], "README.md");
        assert_eq!(calls[1].name, "execute_bash");
        assert_eq!(calls[1].input["command"], "echo probe-hello");
        assert!(calls[1].id.as_ref().starts_with("run_command_tooluse_"), "{:?}", calls[1].id);
        let results: Vec<_> = messages
            .iter()
            .filter_map(|m| match m.content().first() {
                Some(Content::ToolResult(result)) => Some(result.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].call, calls[0].id);
        assert_eq!(results[1].call, calls[1].id);
        assert_eq!(
            results[1].output_text().as_deref(),
            Some("Output:\nprobe-hello\n\n\nExit Code: 0")
        );
        assert!(results.iter().all(|r| !r.error));
    }

    /// The turn's `turn_end` names the stop reason of the reply before it; a reply before a
    /// call's result stopped to use the tool; a reasoning marker is no reply.
    #[rstest]
    fn stop_reasons_land_on_the_turns_replies() {
        let messages = parse(RESUMED);
        let stops: Vec<_> = messages
            .iter()
            .filter(|m| m.role() == Role::Assistant)
            .map(|m| {
                (m.operation().or_else(|| Some(m.kind())).unwrap().to_owned(), m.stop_reason())
            })
            .collect();
        assert_eq!(stops, [
            ("Say".to_owned(), None),
            ("tool_call".to_owned(), Some(StopReason::ToolUse)),
            ("Reasoning".to_owned(), None),
            ("tool_call".to_owned(), Some(StopReason::ToolUse)),
            ("Say".to_owned(), Some(StopReason::EndTurn)),
            ("Reasoning".to_owned(), None),
            ("Say".to_owned(), Some(StopReason::EndTurn)),
            ("Reasoning".to_owned(), None),
            ("Say".to_owned(), Some(StopReason::EndTurn)),
            ("Reasoning".to_owned(), None),
        ]);
        assert!(
            messages
                .iter()
                .filter(|m| matches!(m.role(), Role::Assistant | Role::Tool))
                .all(|m| m.turn_id().is_some())
        );
        assert!(
            messages.iter().filter(|m| m.role() == Role::User).all(|m| m.turn_id().is_none()),
            "a prompt belongs to the turn that follows it"
        );
    }

    /// A reply Kiro holds back to redact is a marker that the model reasoned; one it keeps is
    /// its text; an empty one is nothing.
    #[rstest]
    #[case::redacted("...", Some(Content::ReasoningSummary { tokens: None }))]
    #[case::kept("First read the file.", Some(Content::Reasoning("First read the file.".to_owned())))]
    #[case::empty("", None)]
    fn reasoning_is_a_marker_unless_kiro_kept_it(
        #[case] text: &str,
        #[case] want: Option<Content>,
    ) {
        let row = row(json!({"type": "assistant", "content": text, "operationType": "Reasoning",
            "executionId": "e1", "reasoningModelId": "qdev::auto"}));
        let message = KiroMessage::of(row, &Context::default());
        assert_eq!(message.map(|m| m.content().remove(0)), want);
    }

    #[rstest]
    #[case::session_start(json!({"type": "session_start", "agentType": "vibe", "content": "You are Kiro"}))]
    #[case::turn_start(json!({"type": "turn_start", "executionId": "e1", "boundarySource": "observed"}))]
    #[case::session_event(json!({"type": "session_event", "category": "session_pause"}))]
    #[case::usage(json!({"type": "usage_summary", "promptTurnSummaries": [], "executionId": "e1"}))]
    #[case::metadata(json!({"type": "session_metadata", "key": "contextUsage", "value": {}}))]
    #[case::hook(json!({"type": "ContextualHookInvoked", "name": "atuin-pre", "status": "completed"}))]
    #[case::approval(json!({"type": "pending_interaction", "interactionType": "tool_approval"}))]
    #[case::resolved(json!({"type": "interaction_resolved", "outcome": "cancelled"}))]
    #[case::tombstone(json!({"type": "tombstone", "kind": "summarization", "effectiveFromMessageId": "x"}))]
    #[case::subagent_progress(json!({"type": "sub_agent_progress", "subSessionId": "s", "message": "reading"}))]
    #[case::subagent_unnamed(json!({"type": "sub_agent_start", "subAgentName": "reviewer", "prompt": "Review"}))]
    #[case::mode(json!({"type": "mode_change", "fromMode": "vibe", "toMode": "probe"}))]
    #[case::synthetic_user(json!({"type": "user", "content": "x", "_meta": {"kiro": {"syntheticUserMessageReason": "steer"}}}))]
    #[case::cleared_steer(json!({"type": "user", "content": "x", "_meta": {"kiro": {"steeringClearedIds": ["a"]}}}))]
    #[case::empty_prompt(json!({"type": "user", "content": ""}))]
    #[case::empty_reply(json!({"type": "assistant", "content": "", "operationType": "Say"}))]
    fn bookkeeping_rows_are_skipped(#[case] payload: serde_json::Value) {
        assert!(KiroMessage::of(row(payload), &Context::default()).is_none());
    }

    /// A prompt sent mid-turn is the user's; Kiro's injected context and errors are the
    /// harness's.
    #[rstest]
    #[case::steer(json!({"type": "user", "content": "stop", "source": "steer"}), Role::User, Content::Text("stop".to_owned()))]
    #[case::note(json!({"type": "agent_note", "content": "Output:\n\n\nExit Code: 0"}), Role::System, Content::Text("Output:\n\n\nExit Code: 0".to_owned()))]
    #[case::system(json!({"type": "system", "content": "context"}), Role::System, Content::Text("context".to_owned()))]
    #[case::error(json!({"type": "error", "message": "boom", "code": "E1"}), Role::System, Content::Error("boom".to_owned()))]
    #[case::print(json!({"type": "assistant", "content": "shown", "operationType": "Print"}), Role::Assistant, Content::Text("shown".to_owned()))]
    fn other_rows_keep_their_text(
        #[case] payload: serde_json::Value,
        #[case] role: Role,
        #[case] want: Content,
    ) {
        let message = KiroMessage::of(row(payload), &Context::default()).unwrap();
        assert_eq!(message.role(), role);
        assert_eq!(message.content(), [want]);
    }

    /// A denied call has a result of its own, an error; the turn still ends.
    #[rstest]
    fn a_denied_call_is_an_error_result() {
        let messages = parse(DENIED);
        let Some(Content::ToolResult(result)) =
            messages.iter().find(|m| m.role() == Role::Tool).map(|m| m.content().remove(0))
        else {
            panic!("a result");
        };
        assert!(result.error);
        assert!(result.output_text().unwrap().ends_with("Exit Code: -1"), "{result:?}");
        let call = messages
            .iter()
            .find(|m| matches!(m.content().first(), Some(Content::ToolUse(_))))
            .unwrap();
        assert_eq!(call.stop_reason(), Some(StopReason::ToolUse));
        assert_eq!(texts(&messages, &Role::Assistant), ["done"]);
        assert_eq!(messages.last().unwrap().stop_reason(), None, "the reasoning marker");
        assert_eq!(
            messages.iter().rev().find(|m| m.carries_conversation()).unwrap().stop_reason(),
            Some(StopReason::EndTurn)
        );
    }

    /// A compaction leaves a tombstone, which yields nothing, and a summary, a system row
    /// standing in for the conversation before it; a subagent's spawn is a call in the parent's
    /// story and its answer the call's result (the subagent's own rows live in
    /// `sub-executions/`), and a failed command is an error result.
    #[rstest]
    #[tokio::test]
    async fn a_compaction_is_a_summary_and_a_subagents_spawn_is_a_call() {
        let root = root_with("sess_d3a5510d", COMPACTED, Some(COMPACTED_HEADER));
        let messages = read_all(root.path(), "sess_d3a5510d").await;
        let summary = messages
            .iter()
            .find(|m| m.operation() == Some("Summary"))
            .expect("the compaction's summary");
        assert_eq!(summary.role(), Role::System);
        assert!(
            matches!(&summary.content()[..], [Content::Summary(text)] if text.starts_with("## Overview")),
            "{:?}",
            summary.content()
        );
        assert_eq!(summary.stop_reason(), None);

        let spawn = messages.iter().find(|m| m.kind() == "sub_agent_start").expect("the spawn");
        let Some(Content::ToolUse(call)) = spawn.content().first().cloned() else {
            panic!("a call: {:?}", spawn.content());
        };
        assert_eq!(call.id.as_ref(), SUBAGENT);
        assert_eq!(call.name, SPAWN_TOOL);
        assert_eq!(call.input["name"], "kit-reviewer");
        assert!(call.input["prompt"].as_str().unwrap().starts_with("Review the file calc.py"));
        assert_eq!(spawn.role(), Role::Assistant);
        assert_eq!(spawn.model().as_deref(), Some("auto"));
        assert_eq!(spawn.turn_id().as_deref(), Some("e514a741-9199-48c3-a3d2-a0daa755eb32"));
        assert_eq!(spawn.stop_reason(), Some(StopReason::ToolUse));
        let answer = messages.iter().find(|m| m.kind() == "sub_agent_complete").expect("its end");
        let Some(Content::ToolResult(result)) = answer.content().first().cloned() else {
            panic!("a result: {:?}", answer.content());
        };
        assert_eq!(result.call, call.id);
        assert!(result.output_text().unwrap().starts_with("Reviewed calc.py"), "{result:?}");
        assert!(!result.error);
        assert_eq!(answer.role(), Role::Tool);
        assert_eq!(answer.turn_id(), spawn.turn_id());

        let user = texts(&messages, &Role::User);
        assert_eq!(user.len(), 3);
        assert!(user[1].starts_with("Use the kit-reviewer subagent"), "{user:?}");
        let failed = messages
            .iter()
            .filter_map(|m| match m.content().first() {
                Some(Content::ToolResult(r)) if r.error => Some(r.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(failed.len(), 1);
        assert!(failed[0].output_text().unwrap().contains("FAILED"), "{:?}", failed[0]);
        assert_eq!(messages.iter().filter_map(Message::title).count(), 1);
        assert!(messages.iter().all(|m| m.cwd().is_none()
            || m.cwd().as_deref() == Some(Path::new("/work/20261008T150143Z-K18-v3"))));
    }

    /// The subagent of the compacted session: its file under `sub-executions/`.
    const SUBAGENT: &str = "5d13f3d7-73ae-431d-847f-04729b40503e";
    const COMPACTED_ID: &str = "sess_d3a5510d";

    /// The subagent's own conversation, read from its log: its rows (its reasoning, a read, its
    /// reply, and the `subagent_response` call that answered the parent) name the parent and
    /// take its directory and model, and the agent's name, from the parent's `sub_agent_start`,
    /// is its title, a row of its own once no reply is held.
    #[rstest]
    #[tokio::test]
    async fn a_subagent_is_a_child_session_over_its_own_log() {
        let root = root_with(COMPACTED_ID, COMPACTED, Some(COMPACTED_HEADER));
        with_subagent(root.path(), COMPACTED_ID, SUBAGENT, COMPACTED_SUBAGENT);
        let child = SUBAGENT;
        let session = open(root.path(), child);
        assert_eq!(session.id().as_ref(), child);
        let messages: Vec<KiroMessage> = session.messages().try_collect().await.unwrap();
        let roles: Vec<_> = messages.iter().map(Message::role).collect();
        assert_eq!(roles, [
            Role::Assistant,
            Role::Assistant,
            Role::Tool,
            Role::Other(TITLE_ROW.to_owned()),
            Role::Assistant,
            Role::Assistant,
            Role::Assistant,
            Role::Tool,
        ]);
        assert_eq!(texts(&messages, &Role::Assistant), ["The file has one bug."]);
        let calls: Vec<_> = messages
            .iter()
            .filter_map(|m| match m.content().first() {
                Some(Content::ToolUse(call)) => Some(call.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), [
            "read_file",
            "subagent_response"
        ]);
        assert!(calls[1].input["response"].as_str().unwrap().starts_with("Reviewed calc.py"));
        assert!(
            messages
                .iter()
                .all(|m| m.parent_session().as_ref().map(AsRef::as_ref) == Some(COMPACTED_ID))
        );
        assert!(messages.iter().all(|m| m.parent_kind() == Some(ParentKind::Subagent)));
        let (title, rows): (Vec<_>, Vec<_>) = messages.iter().partition(|m| m.title().is_some());
        assert_eq!(title.len(), 1);
        assert_eq!(title[0].title(), Some(TitleChange::new(TitleSource::Agent, "kit-reviewer")));
        assert_eq!(
            title[0].timestamp().map(|at| at.unix_timestamp()),
            Some(
                OffsetDateTime::parse("2026-10-08T15:02:33.394Z", &Rfc3339)
                    .unwrap()
                    .unix_timestamp()
            ),
            "the title is given when the subagent was spawned"
        );
        assert!(
            rows.iter()
                .all(|m| m.cwd().as_deref() == Some(Path::new("/work/20261008T150143Z-K18-v3")))
        );
        assert!(
            rows.iter().filter(|m| m.models_row()).all(|m| m.model().as_deref() == Some("auto"))
        );
        assert!(
            rows.iter()
                .all(|m| m.turn_id().as_deref() == Some("e514a741-9199-48c3-a3d2-a0daa755eb32"))
        );
        assert_eq!(
            rows.iter().rev().find(|m| m.carries_conversation()).unwrap().stop_reason(),
            Some(StopReason::ToolUse),
            "the answer is a call, before its result"
        );
        // The parent's rows are its own.
        let parent = read_all(root.path(), COMPACTED_ID).await;
        assert!(parent.iter().all(|m| m.parent_session().is_none() && m.parent_kind().is_none()));
        assert!(
            parent.iter().all(|m| m.title() != title[0].title()),
            "the agent's name isn't the parent's"
        );

        // Resumed past a checkpoint, the rest, and the title again.
        let all: Vec<(Checkpoint, KiroMessage)> =
            open(root.path(), child).messages_from(None).try_collect().await.unwrap();
        let (at, _) = &all[4];
        let rest: Vec<(Checkpoint, KiroMessage)> =
            open(root.path(), child).messages_from(Some(*at)).try_collect().await.unwrap();
        let (titles, rows): (Vec<_>, Vec<_>) = rest.iter().partition(|(_, m)| m.title().is_some());
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].1.id(), all[3].1.id(), "the same title row");
        assert_eq!(
            rows.iter().map(|(_, m)| m.id()).collect::<Vec<_>>(),
            all[5..].iter().map(|(_, m)| m.id()).collect::<Vec<_>>()
        );
    }

    /// A subagent whose spawn the parent's log doesn't hold yet (or at all) has no title; it is
    /// named once the row lands, by the next read, or the next line of a read that goes on.
    #[rstest]
    #[tokio::test]
    async fn a_subagent_is_named_once_its_spawn_is_in_the_parents_log() {
        let spawn = COMPACTED
            .lines()
            .find(|line| line.contains("\"sub_agent_start\""))
            .expect("the fixture's spawn");
        let unspawned: String =
            COMPACTED.lines().filter(|l| *l != spawn).map(|l| format!("{l}\n")).collect();
        let root = root_with(COMPACTED_ID, &unspawned, Some(COMPACTED_HEADER));
        with_subagent(root.path(), COMPACTED_ID, SUBAGENT, COMPACTED_SUBAGENT);
        let messages = read_all(root.path(), SUBAGENT).await;
        assert!(messages.iter().all(|m| m.title().is_none()), "no spawn, no name");
        assert_eq!(messages.len(), 7, "the rows are read all the same");
        assert!(messages.iter().all(|m| m.parent_session().is_some()));

        let log = locate(root.path(), COMPACTED_ID).unwrap();
        let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut file, format!("{spawn}\n").as_bytes()).unwrap();
        let messages = read_all(root.path(), SUBAGENT).await;
        assert_eq!(messages.iter().filter_map(Message::title).collect::<Vec<_>>(), [
            TitleChange::new(TitleSource::Agent, "kit-reviewer")
        ]);
        assert!(spawn_of(&log, "no-such-subagent").is_none());
        assert!(spawn_of(Path::new("/no/such/log"), SUBAGENT).is_none());
    }

    /// A subagent's log is reported as it appears, a session under its parent followed on its
    /// own: its rows come through as they are written, and so does its name, when the parent's
    /// spawn row lands after the subagent's first rows.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_listener_reports_a_subagents_log_as_it_appears() {
        let spawn = COMPACTED
            .lines()
            .find(|line| line.contains("\"sub_agent_start\""))
            .expect("the fixture's spawn");
        let unspawned: String =
            COMPACTED.lines().filter(|l| *l != spawn).map(|l| format!("{l}\n")).collect();
        let root = root_with(COMPACTED_ID, &unspawned, Some(COMPACTED_HEADER));
        let (mut watched, parent) = watched(root.path()).await;
        assert_eq!(parent.id().as_ref(), COMPACTED_ID);

        // The subagent's first rows: its reasoning, a read and the read's result.
        let mut lines = COMPACTED_SUBAGENT.lines();
        let first: String = lines.by_ref().take(4).map(|l| format!("{l}\n")).collect();
        let path = with_subagent(root.path(), COMPACTED_ID, SUBAGENT, &first);
        let child = next_report(&mut watched).await;
        assert_eq!(child.id().as_ref(), SUBAGENT);
        assert_eq!(child.parent.as_ref().map(AsRef::as_ref), Some(COMPACTED_ID));
        assert!(child.changes.is_some(), "followed");
        assert!(child.header.is_none(), "the header names the parent, not the subagent");
        let mut messages = Box::pin(child.messages_from(None));
        let mut got = Vec::new();
        for n in 0..3 {
            got.push(next_row_for(&mut messages, &format!("row {n}"), 15).await.1);
        }
        assert_eq!(got.iter().map(Message::role).collect::<Vec<_>>(), [
            Role::Assistant,
            Role::Assistant,
            Role::Tool
        ]);
        assert!(got.iter().all(|m| m.parent_session().is_some()));
        assert!(got.iter().all(|m| m.cwd().is_some()), "the parent's header");

        // The parent names the subagent: the next row of its log brings the name through.
        let log = locate(root.path(), COMPACTED_ID).unwrap();
        let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut file, format!("{spawn}\n").as_bytes()).unwrap();
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(&mut file, format!("{}\n", lines.next().unwrap()).as_bytes())
            .unwrap();
        let (_, named) = next_row_for(&mut messages, "the name", 15).await;
        assert_eq!(
            named.title(),
            Some(TitleChange::new(TitleSource::Agent, "kit-reviewer")),
            "{named:?}"
        );
        // The rest of the subagent's log.
        std::io::Write::write_all(
            &mut file,
            lines.map(|l| format!("{l}\n")).collect::<String>().as_bytes(),
        )
        .unwrap();
        let mut rest = Vec::new();
        for n in 0..4 {
            rest.push(next_row_for(&mut messages, &format!("later row {n}"), 15).await.1);
        }
        assert_eq!(rest.iter().map(Message::role).collect::<Vec<_>>(), [
            Role::Assistant,
            Role::Assistant,
            Role::Assistant,
            Role::Tool
        ]);
        assert!(rest.iter().all(|m| m.title().is_none()), "named once");
    }

    /// A subagent's answer is the parent's result: its response when it succeeded; an error
    /// otherwise, saying what went wrong, or the status when nothing does.
    #[rstest]
    #[case::success(json!({"status": "success", "response": "Found one bug."}), "Found one bug.", false)]
    #[case::error(json!({"status": "error", "errorMessage": "model refused", "response": ""}), "model refused", true)]
    #[case::cancelled(json!({"status": "cancelled"}), "cancelled", true)]
    #[case::partial_answer(json!({"status": "error", "response": "halfway"}), "halfway", true)]
    fn a_subagents_answer_is_its_spawns_result(
        #[case] end: serde_json::Value,
        #[case] output: &str,
        #[case] error: bool,
    ) {
        let mut payload =
            json!({"type": "sub_agent_complete", "subSessionId": "s1", "parentExecutionId": "e1"});
        payload.as_object_mut().unwrap().extend(end.as_object().unwrap().clone());
        let message = KiroMessage::of(row(payload), &Context::default()).unwrap();
        let Some(Content::ToolResult(result)) = message.content().first().cloned() else {
            panic!("a result: {:?}", message.content());
        };
        assert_eq!(result.call.as_ref(), "s1");
        assert_eq!(result.output_text().as_deref(), Some(output));
        assert_eq!(result.error, error);
        assert_eq!(message.turn_id().as_deref(), Some("e1"));
    }

    /// The header names where the session works and which model answers, on every row.
    #[rstest]
    #[tokio::test]
    async fn rows_carry_the_headers_directory_and_model() {
        let root = root_with(RESUMED_ID, RESUMED, Some(RESUMED_HEADER));
        let messages = read_all(root.path(), RESUMED_ID).await;
        let rows: Vec<_> = messages.iter().filter(|m| m.title().is_none()).collect();
        assert_eq!(rows.len(), parse(RESUMED).len());
        assert!(rows.iter().all(|m| m.cwd().as_deref() == Some(Path::new("/tmp/kiro-play"))));
        assert!(rows.iter().all(|m| m.git_branch().is_none()));
        assert!(rows.iter().all(|m| m.usage().is_none()));
        let (models, rest): (Vec<&KiroMessage>, Vec<&KiroMessage>) =
            rows.iter().partition(|m| m.models_row());
        assert!(models.iter().all(|m| m.model().as_deref() == Some("auto")), "the model's rows");
        assert!(rest.iter().all(|m| m.model().is_none()));
    }

    /// Each row names the conversation row before it as its parent, title rows aside, so two
    /// machines going on from one copy make two branches.
    #[rstest]
    #[tokio::test]
    async fn rows_name_the_row_before_them() {
        let root = root_with(RESUMED_ID, RESUMED, Some(RESUMED_HEADER));
        let messages = read_all(root.path(), RESUMED_ID).await;
        let rows: Vec<_> = messages.iter().filter(|m| m.title().is_none()).collect();
        assert_eq!(rows[0].parent_id(), None, "the first row is the root");
        for pair in rows.windows(2) {
            assert_eq!(pair[1].parent_id(), pair[0].id(), "{:?}", pair[1].id());
        }
        let title = messages.iter().find(|m| m.title().is_some()).unwrap();
        assert_eq!(title.parent_id(), None);
        assert!(!rows.iter().any(|m| m.parent_id() == title.id()), "a title row is no parent");
    }

    /// The header names the session this one was forked from (`parentSessionId`), as Kiro's
    /// rewinds and tangents do and as atuin's forks do, or works for (`createdReason`).
    #[rstest]
    #[case::none(r#"{"workspacePaths": ["/w"]}"#, None)]
    #[case::fork(r#"{"parentSessionId": "sess_p"}"#, Some(("sess_p", ParentKind::Fork)))]
    #[case::rewind(r#"{"parentSessionId": "sess_p", "createdReason": "rewind"}"#, Some(("sess_p", ParentKind::Fork)))]
    #[case::subagent(r#"{"parentSessionId": "sess_p", "createdReason": "subagent"}"#, Some(("sess_p", ParentKind::Subagent)))]
    fn the_header_names_the_sessions_parent(
        #[case] json: &str,
        #[case] want: Option<(&str, ParentKind)>,
    ) {
        let context = Header::parse(json).unwrap().context;
        let message =
            KiroMessage::of(row(json!({"type": "user", "content": "hi"})), &context).unwrap();
        let found =
            message.parent_session().map(|s| (s.to_string(), message.parent_kind().unwrap()));
        assert_eq!(found, want.map(|(id, kind)| (id.to_owned(), kind)));
    }

    /// The whole log read at once, for sync, yields what a read of it does: every row, linked
    /// the same, and the header's title after the first line.
    #[rstest]
    #[tokio::test]
    async fn the_log_read_at_once_is_what_a_read_yields() {
        let root = root_with(RESUMED_ID, RESUMED, Some(RESUMED_HEADER));
        let path = locate(root.path(), RESUMED_ID).unwrap();
        let at_once = read_log(&path, &std::fs::read(&path).unwrap());
        let read = read_all(root.path(), RESUMED_ID).await;
        let ids = |messages: &[KiroMessage]| -> Vec<(Option<MessageId>, Option<MessageId>)> {
            messages.iter().map(|m| (m.id(), m.parent_id())).collect()
        };
        assert_eq!(ids(&at_once), ids(&read));
        assert_eq!(at_once[1].title(), read[1].title());
        assert!(at_once.iter().all(|m| m.cwd().is_some() || m.title().is_some()));
    }

    /// The header's title is a row of its own, read with the log's first line so the session is
    /// named at once, and again only once the title changes.
    #[rstest]
    #[tokio::test]
    async fn the_headers_title_is_a_row_of_its_own() {
        let root = root_with(RESUMED_ID, RESUMED, Some(RESUMED_HEADER));
        let messages = read_all(root.path(), RESUMED_ID).await;
        // After the first line's own row.
        let title = &messages[1];
        assert_eq!(title.role(), Role::Other(TITLE_ROW.to_owned()));
        assert!(title.content().is_empty());
        assert_eq!(
            title.title(),
            Some(TitleChange::new(TitleSource::Generated, "Read README and Run Probe Command"))
        );
        assert!(title.timestamp().is_some());
        assert_eq!(title.cwd(), None, "the log's rows say where the session works");
        assert_eq!(messages.iter().filter(|m| m.title().is_some()).count(), 1);
        assert_eq!(messages.len(), parse(RESUMED).len() + 1);

        // Read again, the same row, under the same id: no new row for an unchanged title.
        let id = title.id().expect("a title row has an id of its own");
        assert!(id.as_ref().starts_with("syn-title-"), "{id}");
        let again = read_all(root.path(), RESUMED_ID).await;
        assert_eq!(again[1].id(), Some(id.clone()));

        // Renamed with `/rename`, another row, the user's.
        let renamed = RESUMED_HEADER.replace(
            r#""title": "Read README and Run Probe Command","#,
            r#""title": "renamed", "titleSetByUser": true,"#,
        );
        assert_ne!(renamed, RESUMED_HEADER);
        let header = root.path().join("c15e787aea4828dd").joined(RESUMED_ID).joined(HEADER_FILE);
        std::fs::write(&header, renamed).unwrap();
        let again = read_all(root.path(), RESUMED_ID).await;
        let titles: Vec<_> = again.iter().filter_map(Message::title).collect();
        assert_eq!(titles, [TitleChange::new(TitleSource::Named, "renamed")]);
        assert_ne!(again[1].id(), Some(id));

        // A header half written says nothing; the rows keep what the last whole one said.
        std::fs::write(&header, &RESUMED_HEADER[..RESUMED_HEADER.len() / 2]).unwrap();
        let partial = read_all(root.path(), RESUMED_ID).await;
        assert!(partial.iter().all(|m| m.title().is_none()));
    }

    /// Kiro's generated title, or the user's own once `/rename` set it (`titleSetByUser`).
    #[rstest]
    #[case::generated(r#"{"title": "Read README", "workspacePaths": ["/w"]}"#, Some((TitleSource::Generated, "Read README")))]
    #[case::renamed(r#"{"title": "probe run", "titleSetByUser": true}"#, Some((TitleSource::Named, "probe run")))]
    #[case::flag_off(r#"{"title": "Read README", "titleSetByUser": false}"#, Some((TitleSource::Generated, "Read README")))]
    #[case::untitled(r#"{"title": "", "titleSetByUser": false}"#, None)]
    #[case::no_title(r#"{"workspacePaths": ["/w"]}"#, None)]
    fn the_header_names_the_session(#[case] json: &str, #[case] want: Option<(TitleSource, &str)>) {
        let title = Header::parse(json).unwrap().title();
        assert_eq!(
            title.map(|t| (t.source, t.text.unwrap())),
            want.map(|(source, text)| (source, text.to_owned()))
        );
        assert!(Header::parse("{").is_err());
    }

    #[rstest]
    #[tokio::test]
    async fn a_missing_header_names_nothing() {
        let root = root_with(RESUMED_ID, RESUMED, None);
        let messages = read_all(root.path(), RESUMED_ID).await;
        assert!(messages.iter().all(|m| m.title().is_none() && m.cwd().is_none()));
        assert_eq!(messages.len(), parse(RESUMED).len());
    }

    /// Resumed past a checkpoint, the reader yields the rest, each row with its own line's
    /// checkpoint (a held reply's is its own, not its releasing row's), and the title again.
    #[rstest]
    #[tokio::test]
    async fn resuming_from_a_checkpoint_yields_the_rest() {
        let root = root_with(PROBE_ID, PROBE, Some(PROBE_HEADER));
        let path = locate(root.path(), PROBE_ID).unwrap();
        let session =
            || KiroSession::open(SessionId::from(PROBE_ID.to_owned()), path.clone(), pool());
        let all: Vec<(Checkpoint, KiroMessage)> =
            session().messages_from(None).try_collect().await.unwrap();
        for pair in all.windows(2) {
            assert!(pair[0].0.at <= pair[1].0.at, "checkpoints never go back");
        }
        // Just past the reply before the shell call.
        let (at, say) = &all[4];
        assert_eq!(say.operation(), Some("Say"));
        let rest: Vec<(Checkpoint, KiroMessage)> =
            session().messages_from(Some(*at)).try_collect().await.unwrap();
        // The rows after the reply, and the title again (the same row), once the replies the
        // read starts on are out.
        let (titles, rows): (Vec<_>, Vec<_>) = rest.iter().partition(|(_, m)| m.title().is_some());
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].1.id(), all[1].1.id());
        assert_eq!(
            rows.iter().map(|(_, m)| m.id()).collect::<Vec<_>>(),
            all[5..].iter().map(|(_, m)| m.id()).collect::<Vec<_>>()
        );
        assert!(
            rest.iter().all(|(_, m)| m.title().is_some()
                || m.cwd().as_deref() == Some(Path::new("/tmp/kiro-play")))
        );
    }

    const PROBE_ID: &str = "sess_0550199b-937b-4dd4-94e0-6901bd990fb0";

    #[rstest]
    #[tokio::test]
    async fn existing_lists_every_session_under_every_bucket() {
        let root = root_with(RESUMED_ID, RESUMED, Some(RESUMED_HEADER));
        let other = root.path().join("3c5d3fdb8f014f30").joined(PROBE_ID);
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(LOG_FILE), PROBE).unwrap();
        std::fs::write(other.join("session.json"), PROBE_HEADER).unwrap();
        std::fs::create_dir_all(other.join("sub-executions")).unwrap();
        std::fs::write(other.join("sub-executions").joined("x.jsonl"), PROBE).unwrap();
        let v2 = root.path().join(V2_BUCKET);
        std::fs::create_dir_all(&v2).unwrap();
        std::fs::write(v2.join("old.jsonl"), PROBE).unwrap();
        std::fs::create_dir_all(root.path().join("c15e787aea4828dd").joined(".index")).unwrap();
        std::fs::write(root.path().join("c15e787aea4828dd").joined(".index").joined(".lock"), "")
            .unwrap();
        let sessions = KiroSessions::builder().root(root.path().to_path_buf()).pool(pool()).build();
        let mut ids: Vec<String> = sessions
            .existing()
            .unwrap()
            .map_ok(|s| s.id().to_string())
            .try_collect()
            .await
            .unwrap();
        // The subagent is a session too, listed after its parent.
        let at = |id: &str| ids.iter().position(|found| found == id).expect(id);
        assert!(at(PROBE_ID) < at("x"), "{ids:?}");
        ids.sort();
        assert_eq!(ids, [PROBE_ID, RESUMED_ID, "x"]);
        assert!(matches!(
            KiroSessions::builder().root(PathBuf::from("/no/such")).pool(pool()).build().listener(),
            Err(RuntimeError::NotFound(_))
        ));
    }

    /// A session the listener reports is followed: rows appended come through, and a title Kiro
    /// writes to the header after the log's last line comes through as a row of its own without
    /// waiting for the next line.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_listener_follows_the_log_and_the_header() {
        let root = tempfile::tempdir().unwrap();
        let sessions = KiroSessions::builder().root(root.path().to_path_buf()).pool(pool()).build();
        let listener = sessions.listener().unwrap();
        let mut watched = Box::pin(listener.watch());

        // A new session: the header first, as Kiro writes it, with no title yet.
        let dir = root.path().join("c15e787aea4828dd").joined(PROBE_ID);
        std::fs::create_dir_all(&dir).unwrap();
        let header = dir.join(HEADER_FILE);
        std::fs::write(&header, PROBE_HEADER.replace("Get Unix Time In Nanoseconds", "")).unwrap();
        let mut lines = PROBE.lines();
        let log = dir.join(LOG_FILE);
        std::fs::write(&log, format!("{}\n", lines.next().unwrap())).unwrap();
        async fn reported<S>(watched: &mut S) -> KiroSession
        where
            S: Stream<Item = Result<KiroSession, WatchError>> + Unpin,
        {
            tokio::time::timeout(std::time::Duration::from_secs(5), watched.next())
                .await
                .expect("the session is reported")
                .unwrap()
                .unwrap()
        }
        let mut session = reported(&mut watched).await;
        assert_eq!(session.id().as_ref(), PROBE_ID);
        // The watcher lists the directory in no set order: a header listed after its log
        // reports the session again, with the header watched.
        if session.header.is_none() {
            session = reported(&mut watched).await;
            assert_eq!(session.id().as_ref(), PROBE_ID);
        }
        assert!(session.header.is_some(), "the header is watched too");

        let mut messages = Box::pin(session.messages_from(None));
        async fn next<S>(messages: &mut S) -> (Checkpoint, KiroMessage)
        where
            S: Stream<Item = Result<(Checkpoint, KiroMessage), MessageError>> + Unpin,
        {
            tokio::time::timeout(std::time::Duration::from_secs(5), messages.next())
                .await
                .expect("a row")
                .unwrap()
                .unwrap()
        }
        let (_, first) = next(&mut messages).await;
        assert_eq!(first.role(), Role::System, "the session context block");
        assert_eq!(first.cwd().as_deref(), Some(Path::new("/tmp/kiro-play")));

        // The prompt appended.
        let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut file, format!("{}\n", lines.next().unwrap()).as_bytes())
            .unwrap();
        let (_, prompt) = next(&mut messages).await;
        assert_eq!(prompt.role(), Role::User);

        // The title, written to the header alone: a row of its own at once.
        std::fs::write(&header, PROBE_HEADER).unwrap();
        let (_, title) = next(&mut messages).await;
        assert_eq!(
            title.title(),
            Some(TitleChange::new(TitleSource::Generated, "Get Unix Time In Nanoseconds"))
        );

        // The rest of the log: the rows, and no title row again.
        std::io::Write::write_all(
            &mut file,
            lines.map(|l| format!("{l}\n")).collect::<String>().as_bytes(),
        )
        .unwrap();
        let want = parse(PROBE).len() - 2;
        let mut got = Vec::new();
        while got.len() < want {
            let (_, m) = next(&mut messages).await;
            got.push(m);
        }
        assert!(
            got.iter().all(|m| m.title().is_none()),
            "{:?}",
            got.iter().filter_map(Message::title).collect::<Vec<_>>()
        );
        assert_eq!(got.last().unwrap().operation(), Some("Reasoning"));
        assert_eq!(
            got.iter().rev().find(|m| m.carries_conversation()).unwrap().stop_reason(),
            Some(StopReason::EndTurn)
        );
    }

    /// The watcher's report of the one session under `root`, with its header watched (the
    /// header may be listed after the log, which reports the session again), and the watch
    /// itself, which keeps the session's receivers alive.
    async fn watched(
        root: &Path,
    ) -> (std::pin::Pin<Box<dyn Stream<Item = Result<KiroSession, WatchError>> + Send>>, KiroSession)
    {
        let sessions = KiroSessions::builder().root(root.to_path_buf()).pool(pool()).build();
        let mut watched = Box::pin(sessions.listener().unwrap().watch());
        let mut session = next_report(&mut watched).await;
        if session.header.is_none() {
            session = next_report(&mut watched).await;
        }
        assert!(session.header.is_some(), "the header is watched too");
        (watched, session)
    }

    async fn next_report<S>(watched: &mut S) -> KiroSession
    where
        S: Stream<Item = Result<KiroSession, WatchError>> + Unpin,
    {
        tokio::time::timeout(std::time::Duration::from_secs(5), watched.next())
            .await
            .expect("the session is reported")
            .unwrap()
            .unwrap()
    }

    /// The next row, saying what it is for when it doesn't come within `secs`.
    async fn next_row_for<S>(messages: &mut S, what: &str, secs: u64) -> (Checkpoint, KiroMessage)
    where
        S: Stream<Item = Result<(Checkpoint, KiroMessage), MessageError>> + Unpin,
    {
        tokio::time::timeout(std::time::Duration::from_secs(secs), messages.next())
            .await
            .unwrap_or_else(|_| panic!("{what}: no row within {secs}s"))
            .unwrap()
            .unwrap()
    }

    /// Make `path` an hour old.
    fn age(path: &Path) {
        let old = SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(old).unwrap();
    }

    /// A title row waits until no reply is held: yielded with a checkpoint past a reply still
    /// held, a read resumed from it would skip the reply. Here the title is there before the
    /// log's first line, a reply, which `turn_end` releases; the title follows it, and a read
    /// resumed from the title's checkpoint skips nothing.
    #[rstest]
    #[tokio::test]
    async fn a_title_row_waits_for_held_replies() {
        let log = [
            json!({"id": "r1-say", "timestamp": "2026-10-30T17:55:14.316Z", "payload": {"type": "assistant", "content": "hello", "operationType": "Say", "executionId": "e1"}}),
            json!({"id": "m1", "timestamp": "2026-10-30T17:55:14.317Z", "payload": {"type": "session_metadata", "key": "contextUsage", "value": {}, "executionId": "e1"}}),
            json!({"id": "e1-turn-end", "timestamp": "2026-10-30T17:55:14.318Z", "payload": {"type": "turn_end", "stopReason": "end_turn", "executionId": "e1"}}),
            json!({"id": "u2", "timestamp": "2026-10-30T17:55:20.000Z", "payload": {"type": "user", "content": "next"}}),
        ]
        .iter()
        .map(|row| row.to_string() + "\n")
        .collect::<String>();
        let root = root_with("sess_held", &log, Some(PROBE_HEADER));
        let path = locate(root.path(), "sess_held").unwrap();
        let session =
            || KiroSession::open(SessionId::from("sess_held".to_owned()), path.clone(), pool());
        let all: Vec<(Checkpoint, KiroMessage)> =
            session().messages_from(None).try_collect().await.unwrap();
        let kinds: Vec<_> = all.iter().map(|(_, m)| m.role()).collect();
        assert_eq!(kinds, [Role::Assistant, Role::Other(TITLE_ROW.to_owned()), Role::User]);
        let (reply, title, user) = (&all[0], &all[1], &all[2]);
        assert_eq!(reply.1.stop_reason(), Some(StopReason::EndTurn));
        assert!(reply.0.at < title.0.at && title.0.at < user.0.at, "{all:?}");
        let rest: Vec<(Checkpoint, KiroMessage)> =
            session().messages_from(Some(title.0)).try_collect().await.unwrap();
        let ids: Vec<_> = rest.iter().filter_map(|(_, m)| m.id()).collect();
        assert!(ids.contains(&user.1.id().unwrap()), "{ids:?}");
        assert!(!ids.contains(&reply.1.id().unwrap()), "the reply was yielded before the title");
    }

    /// Renamed to another title and back, each rename is a row of its own; a read started over
    /// names the session under the first row again.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_rename_back_to_an_earlier_title_is_a_row_of_its_own() {
        let root = root_with(PROBE_ID, PROBE, Some(PROBE_HEADER));
        let header = root.path().join("c15e787aea4828dd").joined(PROBE_ID).joined(HEADER_FILE);
        let (_watch, session) = watched(root.path()).await;
        let mut messages = Box::pin(session.messages_from(None));
        let mut rows = Vec::new();
        for n in 0..=parse(PROBE).len() {
            rows.push(next_row_for(&mut messages, &format!("row {n} of the log"), 15).await.1);
        }
        let first = rows.iter().find(|m| m.title().is_some()).unwrap().id().unwrap();
        let renamed = |to: &str| {
            PROBE_HEADER.replace(
                r#""title": "Get Unix Time In Nanoseconds","#,
                &format!(r#""title": "{to}", "titleSetByUser": true,"#),
            )
        };
        std::fs::write(&header, renamed("B")).unwrap();
        let (_, b) = next_row_for(&mut messages, "the rename to B", 15).await;
        assert_eq!(b.title(), Some(TitleChange::new(TitleSource::Named, "B")));
        // Written right after the first rename was seen: Windows can stamp both rewrites with
        // one modification time, which the header watch must not go by.
        std::fs::write(&header, renamed("Get Unix Time In Nanoseconds")).unwrap();
        let (_, a) = next_row_for(&mut messages, "the rename back", 15).await;
        assert_eq!(
            a.title(),
            Some(TitleChange::new(TitleSource::Named, "Get Unix Time In Nanoseconds"))
        );
        let ids = [first.clone(), b.id().unwrap(), a.id().unwrap()];
        assert_eq!(ids.iter().collect::<std::collections::HashSet<_>>().len(), 3, "{ids:?}");

        let again = read_all(root.path(), PROBE_ID).await;
        let title = again.iter().find(|m| m.title().is_some()).unwrap();
        assert_eq!(title.id(), Some(first), "a read started over: the same row");
    }

    /// A session with no checkpoint is read once it changes: a header rewritten on its own (a
    /// `/rename` of an idle session) counts, not only a line appended to the log.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_header_rewrite_is_a_change() {
        let root = root_with(PROBE_ID, PROBE, Some(PROBE_HEADER));
        let dir = root.path().join("c15e787aea4828dd").joined(PROBE_ID);
        age(&dir.join(LOG_FILE));
        age(&dir.join(HEADER_FILE));
        let (_watch, session) = watched(root.path()).await;
        let since = SystemTime::now() - std::time::Duration::from_secs(1);
        let mut changed = Box::pin(session.clone().changed_after(since));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(300), &mut changed)
                .await
                .is_err(),
            "nothing changed since"
        );
        std::fs::write(
            dir.join(HEADER_FILE),
            PROBE_HEADER.replace("Get Unix Time In Nanoseconds", "renamed"),
        )
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), changed)
            .await
            .expect("the header's rewrite wakes it")
            .expect("the watch is live");
    }
}
