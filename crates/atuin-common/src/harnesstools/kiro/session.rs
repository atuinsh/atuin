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
//! **Ids and links.** Row ids are unique within the log. Rows name no parent: the session is
//! linear, and each row's `executionId` (the turn) is its `turn_id`. Timestamps are not in file
//! order (a `turn_end` is stamped before the rows written before it); rows are read in file
//! order.

use std::collections::HashMap;
use std::io;
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
    Content, MessageId, Role, StopReason, TitleChange, TitleSource, ToolCallId, ToolResult, ToolUse,
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
/// The bucket of Kiro's old (V2) store, under the same root.
const V2_BUCKET: &str = "cli";

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

/// The directory of session `id` under the sessions directory `root`: in whichever bucket holds
/// it (a session never moves buckets, however it is resumed).
pub(crate) fn session_dir(root: &Path, id: &str) -> Option<PathBuf> {
    if !resume::is_plain_name(id) {
        return None;
    }
    let buckets = std::fs::read_dir(root).ok()?;
    buckets
        .filter_map(Result::ok)
        .filter(|bucket| bucket.file_name() != V2_BUCKET)
        .map(|bucket| bucket.path().joined(id))
        .find(|dir| dir.is_dir())
}

/// The log of session `id` under `root`, when it is there.
pub(crate) fn locate(root: &Path, id: &str) -> Option<PathBuf> {
    let path = session_dir(root, id)?.joined(LOG_FILE);
    path.is_file().then_some(path)
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

/// Whether `path`, under the sessions directory `root`, is a session's file called `name`:
/// `<root>/<bucket>/<session>/<name>`, outside the V2 bucket. The rest of a session's directory
/// is not conversation (`sub-executions/`, `tool-outputs/`, `snapshots/`), and neither is a
/// bucket's `.index/`.
fn is_session_file(root: &Path, path: &Path, name: &str) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let mut parts = relative.components();
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (
            Some(Component::Normal(bucket)),
            Some(Component::Normal(_)),
            Some(Component::Normal(file)),
            None,
        ) => bucket != V2_BUCKET && file == name,
        _ => false,
    }
}

fn is_transcript(root: &Path, path: &Path) -> bool {
    is_session_file(root, path, LOG_FILE)
}

fn is_header(root: &Path, path: &Path) -> bool {
    is_session_file(root, path, HEADER_FILE)
}

/// A session's two files, as the watcher has reported them so far.
#[derive(Debug, Default)]
struct Files {
    log: Option<(PathBuf, watch::Receiver<FileStat>)>,
    header: Option<watch::Receiver<FileStat>>,
}

impl KiroListener {
    /// The session id a log's (or header's) directory is named with.
    fn session_id(path: &Path) -> Option<SessionId> {
        let dir = path.parent()?.file_name()?;
        Some(SessionId::from(dir.to_string_lossy().into_owned()))
    }

    /// Build a read-once session for a log under `root` (no change signal), or `None`.
    fn open_session(
        root: &Path,
        path: &Path,
        is_file: bool,
        pool: &BlockingPool,
    ) -> Option<KiroSession> {
        if !is_file || !is_transcript(root, path) {
            return None;
        }
        Some(KiroSession::open(Self::session_id(path)?, path.to_path_buf(), pool.clone()))
    }
}

impl Listener for KiroListener {
    type Session = KiroSession;

    /// Every log as the watcher reports it, with a watch on the header beside it when the
    /// watcher has reported that too. The watcher lists a directory in no set order, so a header
    /// reported after its log reports the session again, now with the header watched; the
    /// consumer replaces its stream, as it does for a log made again.
    fn watch(self) -> impl Stream<Item = Result<KiroSession, WatchError>> + Send + 'static {
        let root = self.root;
        let pool = self.pool;
        async_stream::stream! {
            // The watcher reports paths under the canonical root.
            let sessions = AbsPathBuf::canonicalize(&root).map_or_else(|_| root.clone(), PathBuf::from);
            let files = TreeWatcher::builder()
                .filter(move |path| is_transcript(&sessions, path) || is_header(&sessions, path))
                .watch(&root);
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
                let Some(dir) = path.parent() else { continue };
                let Some(id) = Self::session_id(&path) else { continue };
                let files = seen.entry(dir.to_path_buf()).or_default();
                if path.file_name().is_some_and(|name| name == HEADER_FILE) {
                    files.header = Some(stat);
                } else {
                    files.log = Some((path.to_path_buf(), stat));
                }
                let Some((log, changes)) = &files.log else { continue };
                yield Ok(KiroSession {
                    id,
                    path: log.clone(),
                    changes: Some(changes.clone()),
                    header: files.header.clone(),
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
            pool,
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
            let header = Arc::new(Mutex::new(HeaderWatch::new(self.path.with_file_name(HEADER_FILE))));
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
                                yield Ok((checkpoint, title_row(title, at, titled)));
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
                            yield Ok((checkpoint, title_row(title, at, titled)));
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
                                yield Ok((checkpoint, title_row(title, at, titled)));
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

/// What the header says of the session, stamped on its rows.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Context {
    cwd: Option<PathBuf>,
    model: Option<String>,
}

/// What the reader carries from one row to the ones after it.
#[derive(Debug, Default)]
struct Reading {
    /// A turn's assistant rows since the last row that wasn't one, held for the turn's stop
    /// reason (see the module docs), each with its own line's checkpoint.
    held: Vec<(Checkpoint, KiroMessage)>,
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
        let Some(message) = KiroMessage::of(row, context) else {
            return Vec::new();
        };
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

/// A row carrying `title`, which the header gave the session at `at`, the `nth` title row of its
/// read (see the module docs): the same title read again from the start is the same row, and a
/// title the session is renamed back to is a new one.
fn title_row(title: TitleChange, at: Option<SystemTime>, nth: u32) -> KiroMessage {
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
        stop_reason: None,
        title: Some(title),
        at: at.map(OffsetDateTime::from),
    }
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
        Ok(Self {
            context: Context {
                cwd: header["workspacePaths"][0].as_str().map(PathBuf::from),
                model: header["modelId"].as_str().map(str::to_owned),
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

    async fn read_all(root: &Path, id: &str) -> Vec<KiroMessage> {
        let path = locate(root, id).expect("the session is under the root");
        KiroSession::open(SessionId::from(id.to_owned()), path, pool())
            .messages()
            .try_collect()
            .await
            .unwrap()
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

    #[rstest]
    #[case::log("c15e787aea4828dd/sess_1/messages.jsonl", true)]
    #[case::header("c15e787aea4828dd/sess_1/session.json", false)]
    #[case::v2("cli/sess_1/messages.jsonl", false)]
    #[case::subagent("c15e787aea4828dd/sess_1/sub-executions/x.jsonl", false)]
    #[case::index("c15e787aea4828dd/.index/index.json", false)]
    #[case::at_root("messages.jsonl", false)]
    fn logs_are_the_sessions(#[case] relative: &str, #[case] expected: bool) {
        let root = Path::new("/sessions");
        assert_eq!(is_transcript(root, &root.join(relative)), expected);
        assert!(!is_transcript(root, Path::new("/elsewhere/b/sess_1/messages.jsonl")));
        assert!(is_header(root, Path::new("/sessions/b/sess_1/session.json")));
        assert!(!is_header(root, Path::new("/sessions/cli/sess_1/session.json")));
    }

    #[rstest]
    fn session_id_is_the_directory_name() {
        for file in [LOG_FILE, HEADER_FILE] {
            let path = Path::new("/sessions/c15e787aea4828dd").join(RESUMED_ID).joined(file);
            assert_eq!(KiroListener::session_id(&path).unwrap().as_ref(), RESUMED_ID);
        }
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
        ids.sort();
        assert_eq!(ids, [PROBE_ID, RESUMED_ID]);
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
