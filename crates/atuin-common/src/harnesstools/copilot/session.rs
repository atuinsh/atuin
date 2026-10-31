//! GitHub Copilot CLI sessions: one directory per session under `$COPILOT_HOME/session-state`
//! (`~/.copilot/session-state`), holding `events.jsonl`, the session's event log, and
//! `workspace.yaml`, a sidecar Copilot rewrites with the session's directory, branch and name.
//!
//! The log is one JSON envelope per line, `{type, data, id, parentId, timestamp}`, appended as
//! the session goes on (as of Copilot CLI 1.0.104; the format is not documented). Three kinds
//! of event are conversation:
//! - `user.message`: what the user typed (`content`). One with a `source` is Copilot's own: the
//!   `memory` it injects before a prompt, or a subagent's prompt (`agent-<session>`), written by
//!   the agent that spawned it.
//! - `assistant.message`: the model's reply: `content` (text), `toolRequests` (its tool calls),
//!   and its reasoning (`reasoningText`, a summary; `reasoningOpaque` and `encryptedContent`,
//!   which can't be read). Its `apiCallId` names the model call, whose usage Copilot writes
//!   just before it as a `session.usage_record` with the same id.
//! - `tool.execution_complete`: a tool's result (`result.content`, `success`), and for an edit
//!   the files it changed (`fileEdits`) and the diff (`result.detailedContent`).
//! - `session.compaction_complete`: a compaction (`/compact`, or Copilot's own) that was applied,
//!   with the summary that now stands in for the conversation before it (`summaryContent`).
//!
//! The rest is bookkeeping, skipped: `tool.execution_start` repeats the call the
//! `assistant.message` holds; `model.*` logs the request Copilot sent (the whole conversation
//! again); `system.message` is the system prompt; `session.start` and `session.resume` say where
//! the session works (`context.cwd`, `context.branch`), which the reader stamps on every message
//! instead; `session.usage_checkpoint` and `session.shutdown` are totals; `permission.*`,
//! `assistant.turn_*`, `subagent.*` and `session.model_change` are lifecycle.
//!
//! **Subagents.** Copilot writes a subagent's conversation into its parent's log: a
//! `subagent.started` naming the `task` call that spawned it, then its rows under an interaction
//! of its own (`interactionId`), opened by a `user.message` whose `source` is `agent-<parent>`
//! (the prompt the parent wrote), its replies naming the call too (`parentToolCallId`), and a
//! `subagent.completed`. Each subagent is read as a session of its own, `agent-<call>`, a child
//! of the parent ([`ParentKind::Subagent`]) titled with the agent's name, over the same log
//! filtered to its rows; the parent's own rows leave them out, and keep the `task` call and its
//! result. A log's listener reports a child when the parent's reader comes across its start (so
//! only logs the daemon reads are scanned), and a one-shot scan reads each log for them.
//!
//! **Titles.** No event names the session. Copilot generates a name at the first prompt and
//! writes it to `workspace.yaml` (`name`, with `user_named` when `--name` or `/rename` set it,
//! neither of which writes an event). The reader checks the sidecar as each line of the log
//! arrives, and yields the name as a row of its own (a system row with nothing but the title,
//! timed by the sidecar's modification time, and with an id made of the name) whenever it has
//! changed, the first line of a read included: a re-read, or a read resumed past a checkpoint,
//! yields the same row again under the same id, so the session is named again without a new
//! row, and a rename is a row of its own however long after the last message it came. The id
//! is the name's alone so that a copy of the session atuin wrote elsewhere, whose sidecar it
//! names the same, reads back with the same row rather than a new one (a session named back
//! to an earlier name stays under the later one in atuin). A rename after the log's last line
//! (then `/exit`, which writes one) is picked up by the next line written. Such a row says
//! nothing of where the session works: the context the log's lines carry is theirs.
//!
//! **Links.** Copilot's events chain (`parentId`), through the bookkeeping the reader skips, so
//! each row names the row before it in its lane as its parent (a subagent's rows chain among
//! themselves, the parent's among themselves): sync sees the session as a tree, so two machines
//! going on from one copy are two branches of it. A title row names none: keyed on its content
//! (`syn-`), it sits beside the tree, with the branch it was read on.
//!
//! **Forks and continuations.** Copilot forks nothing itself. A fork atuin wrote names the
//! session it was forked from in its `session.start`, under `atuin` (see the writer), which
//! every row of the fork then names as its parent. A continuation's marker is a `user.message`
//! atuin wrote (`source: "atuin"`), its text in `transformedContent`, which Copilot gives the
//! model as it gives its own memory: read as a system row, which capture reads the marker from.
//!
//! **Ids.** Every event has a `parentId`, the event before it, bookkeeping included: a chain
//! through rows the reader skips. The rows are read in file order, so none names a parent.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use futures::{Stream, StreamExt};
use parking_lot::Mutex;
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::{mpsc, watch};
use typed_builder::TypedBuilder;

use crate::fs::tree_watcher::{FileStat, TreeWatcher};
use crate::harnesstools::copilot::rehydrate::ATUIN_SOURCE;
use crate::harnesstools::copilot::{Copilot, copilot_home};
use crate::harnesstools::resume;
use crate::harnesstools::session::model::{
    Content, MessageId, ParentKind, Role, StopReason, TitleChange, TitleSource, ToolCallId,
    ToolResult, ToolUse, Usage,
};
use crate::harnesstools::session::synthetic::SYNTHETIC;
use crate::harnesstools::session::{
    Change, Checkpoint, FilePatch, Listener, Message, MessageError, Observable, Patch,
    RuntimeError, Session, SessionId, Sessions, WatchError, checkpoint_offset, modified_after,
    scan_sessions,
};
use crate::io::{FollowLines, PathLineReader, PooledReadLines};
use crate::json::jsonl::JsonlExt;
use crate::path::{AbsPathBuf, PathBufExt as _};
use crate::sync::BlockingPool;

/// A session's event log, in its directory.
pub(crate) const EVENTS_FILE: &str = "events.jsonl";
/// The sidecar beside the log, with the session's directory, branch and name.
pub(crate) const SIDECAR_FILE: &str = "workspace.yaml";

#[derive(Debug, Clone, TypedBuilder)]
pub struct CopilotSessions {
    #[builder(default, setter(strip_option, into))]
    root: Option<PathBuf>,
    /// Runs every file read of the sessions this finds.
    pool: BlockingPool,
}

impl CopilotSessions {
    fn resolve_root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(default_root)
    }
}

/// Copilot's sessions directory: `$COPILOT_HOME/session-state`, else `~/.copilot/session-state`.
pub(crate) fn default_root() -> PathBuf {
    copilot_home().joined("session-state")
}

/// The directory of session `id` under the sessions directory `root`.
pub(crate) fn session_dir(root: &Path, id: &str) -> Option<PathBuf> {
    resume::is_plain_name(id).then(|| root.join(id))
}

/// The event log of session `id` under `root`, when it is there.
pub(crate) fn locate(root: &Path, id: &str) -> Option<PathBuf> {
    let path = session_dir(root, id)?.joined(EVENTS_FILE);
    path.is_file().then_some(path)
}

impl Sessions for CopilotSessions {
    type Listener = CopilotListener;

    fn listener(&self) -> Result<CopilotListener, RuntimeError> {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(CopilotListener {
            root,
            pool: self.pool.clone(),
        })
    }

    fn existing(
        &self,
    ) -> Result<
        impl Stream<Item = Result<CopilotSession, RuntimeError>> + Send + 'static,
        RuntimeError,
    > {
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
                        CopilotListener::open_session(&sessions, path, is_file, &sessions_pool)
                    })
                })
                .await;
            match scan {
                Ok(items) => {
                    for item in items {
                        let Ok(parent) = &item else {
                            yield item;
                            continue;
                        };
                        // The subagents the log holds, each a session of its own.
                        let (path, parent) = (parent.path.clone(), parent.clone());
                        let lanes = match pool.run(move || lanes_in(&path)).await {
                            Ok(Ok(lanes)) => lanes,
                            // Reading the log for the parent's rows reports what is wrong with it.
                            Ok(Err(_)) => Vec::new(),
                            Err(cancelled) => {
                                yield Err(RuntimeError::Io(io::Error::other(cancelled)));
                                return;
                            }
                        };
                        yield item;
                        for lane in lanes {
                            yield Ok(parent.child(&lane));
                        }
                    }
                }
                Err(cancelled) => yield Err(RuntimeError::Io(io::Error::other(cancelled))),
            }
        })
    }
}

impl Observable for Copilot {
    type Sessions = CopilotSessions;

    fn sessions(&self, pool: BlockingPool) -> CopilotSessions {
        CopilotSessions::builder().pool(pool).build()
    }
}

#[derive(Debug, Clone)]
pub struct CopilotListener {
    root: PathBuf,
    pool: BlockingPool,
}

/// Whether `path`, under the sessions directory `root`, is a session's event log:
/// `<root>/<session>/events.jsonl`. The rest of a session's directory is not conversation
/// (`checkpoints/`, `files/`, `rewind-file-snapshots/`, its locks), nor is what Copilot keeps
/// beside the sessions (`.session-operation-locks/`).
fn is_transcript(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let mut parts = relative.components();
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(Component::Normal(_)), Some(Component::Normal(file)), None) if file == EVENTS_FILE
    )
}

impl CopilotListener {
    /// The session id a log's directory is named with.
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
    ) -> Option<CopilotSession> {
        if !is_file || !is_transcript(root, path) {
            return None;
        }
        Some(CopilotSession::open(Self::session_id(path)?, path.to_path_buf(), pool.clone()))
    }
}

impl Listener for CopilotListener {
    type Session = CopilotSession;

    /// Every log as the watcher reports it, and every subagent a log's reader comes across: a
    /// parent session carries a sender its reader announces children on, and the children are
    /// reported here as they are announced (each once, however many times a re-read announces
    /// it).
    fn watch(self) -> impl Stream<Item = Result<CopilotSession, WatchError>> + Send + 'static {
        let root = self.root;
        let pool = self.pool;
        async_stream::stream! {
            // The watcher reports paths under the canonical root.
            let sessions = AbsPathBuf::canonicalize(&root).map_or_else(|_| root.clone(), PathBuf::from);
            let files = TreeWatcher::builder()
                .filter(move |path| is_transcript(&sessions, path))
                .watch(&root);
            let files = match files {
                Ok(files) => files,
                Err(err) => {
                    yield Err(WatchError::from(err));
                    return;
                }
            };
            let (announce, mut announced) = mpsc::unbounded_channel::<CopilotSession>();
            // The children reported for each parent's current watch. A log removed and made
            // again is reported again with a fresh watch, and so are its children then, each
            // once more: their old watches ended with the old log.
            let mut reported: HashMap<SessionId, HashSet<SessionId>> = HashMap::new();
            futures::pin_mut!(files);
            let mut files_done = false;
            loop {
                tokio::select! {
                    biased;
                    file = files.next(), if !files_done => match file {
                        Some(file) => {
                            let (path, stat) = file.into_parts();
                            let Some(id) = Self::session_id(&path) else { continue };
                            reported.remove(&id);
                            yield Ok(CopilotSession {
                                id,
                                path: path.to_path_buf(),
                                changes: Some(stat),
                                pool: pool.clone(),
                                lane: None,
                                announce: Some(announce.clone()),
                            });
                        }
                        None => files_done = true,
                    },
                    child = announced.recv() => match child {
                        Some(child) => {
                            let Some(parent) = child.parent_id() else { continue };
                            if reported.entry(parent).or_default().insert(child.id.clone()) {
                                yield Ok(child);
                            }
                        }
                        // Every sender is this stream's own, or a session's it yielded: none
                        // left means neither will announce again.
                        None => break,
                    },
                }
                if files_done && announced.is_closed() {
                    break;
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct CopilotSession {
    id: SessionId,
    path: PathBuf,
    /// Wakes [`messages`](Session::messages) on each change to the log; `None` reads it once.
    changes: Option<watch::Receiver<FileStat>>,
    pool: BlockingPool,
    /// For a subagent's session, the `task` call that spawned it: the rows read are the
    /// subagent's. `None` reads the parent's rows.
    lane: Option<Lane>,
    /// Where a parent's reader announces the subagents it comes across, to the listener that
    /// yielded it (see [`CopilotListener::watch`]).
    announce: Option<mpsc::UnboundedSender<Self>>,
}

/// The id a subagent's session takes: `agent-<call>`, after the `task` call that spawned it.
const SUBAGENT_PREFIX: &str = "agent-";

/// Whether `id` names a subagent's session.
pub(crate) fn is_subagent_id(id: &str) -> bool {
    id.starts_with(SUBAGENT_PREFIX)
}

impl CopilotSession {
    /// A session over the log as it stands: [`messages`](Session::messages) ends at its end.
    #[must_use]
    pub fn open(id: SessionId, path: PathBuf, pool: BlockingPool) -> Self {
        Self {
            id,
            path,
            changes: None,
            pool,
            lane: None,
            announce: None,
        }
    }

    /// The session of the subagent `lane` spawned in this (parent) session: over the same log,
    /// woken by the same changes.
    fn child(&self, lane: &Lane) -> Self {
        Self {
            id: SessionId::from(format!("{SUBAGENT_PREFIX}{}", lane.call)),
            path: self.path.clone(),
            changes: self.changes.clone(),
            pool: self.pool.clone(),
            lane: Some(lane.clone()),
            announce: None,
        }
    }

    /// The parent of a subagent's session, from the log's directory.
    fn parent_id(&self) -> Option<SessionId> {
        self.lane.as_ref().and_then(|_| CopilotListener::session_id(&self.path))
    }

    /// The messages of the log from byte `start`, each with the checkpoint just past its line,
    /// following the log when `changes` is given.
    fn messages_at(
        self,
        start: u64,
        over: bool,
    ) -> impl Stream<Item = Result<(Checkpoint, CopilotMessage), MessageError>> + Send + 'static
    {
        async_stream::stream! {
            let (path, pool) = (self.path.clone(), self.pool.clone());
            let reading = pool.run(move || Reading::before(&path, start)).await;
            let mut reading = match reading.map_err(io::Error::other).and_then(|read| read) {
                Ok(reading) => reading,
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        path = %self.path.display(),
                        "failed to read the events before a checkpoint; reading on without them"
                    );
                    Reading::default()
                }
            };
            // A parent announces the subagents the events before the checkpoint started too:
            // each is a session the listener may not have reported yet.
            self.announce_lanes(&reading.lanes);
            let sidecar = Arc::new(Mutex::new(SidecarWatch::new(self.path.with_file_name(SIDECAR_FILE))));
            let lines = FollowLines::new(PooledReadLines::new(
                PathLineReader::at(&self.path, start),
                self.pool.clone(),
            ));
            let events = match self.changes.clone() {
                Some(changes) => lines.follow(changes).left_stream(),
                None => lines.read_to_end().right_stream(),
            }
            .json_with(Envelope::decode);
            let mut generation = None;
            let mut titled = false;
            for await item in events {
                let (line, event) = match item {
                    Ok(item) => item,
                    Err(err) => {
                        yield Err(MessageError::from(err));
                        continue;
                    }
                };
                // Started over from the log's first byte: nothing read before counts.
                if generation.replace(line.generation).is_some_and(|seen| seen != line.generation) {
                    reading = Reading::default();
                    titled = false;
                }
                let checkpoint = Checkpoint::after(&line).started_over(over);
                let lanes = reading.lanes.len();
                let row = reading.absorb(event);
                if reading.lanes.len() > lanes {
                    self.announce_lanes(&reading.lanes[lanes..]);
                }
                let mine = self.lane.as_ref().map(|lane| lane.call.as_str());
                if let Some((owner, mut message)) = row.filter(|(owner, _)| owner.as_deref() == mine) {
                    let _ = owner;
                    if let Some(lane) = &self.lane {
                        message.parent = self.parent_id().map(|id| (id, ParentKind::Subagent));
                        // The agent's name rides on the subagent's first row, a stable one.
                        if !titled {
                            message.title = lane.title();
                            titled = true;
                        }
                    }
                    yield Ok((checkpoint, message));
                }
                // The sidecar names the parent: after the line's own row, so a crash between the
                // two loses only the title row, which the next read names again.
                if self.lane.is_none() {
                    let watch = Arc::clone(&sidecar);
                    let renamed = self.pool.run(move || watch.lock().refresh()).await.ok().flatten();
                    if let Some((title, at)) = renamed {
                        yield Ok((checkpoint, reading.title_row(title, at)));
                    }
                }
            }
        }
    }

    /// Announce the subagents `lanes` to the listener, as children of this (parent) session.
    /// A child announces none: its subagents are the parent's to announce. Nothing listening
    /// (a one-shot read) is fine.
    fn announce_lanes(&self, lanes: &[Lane]) {
        let Some(announce) = self.announce.as_ref().filter(|_| self.lane.is_none()) else {
            return;
        };
        for lane in lanes {
            let _ = announce.send(self.child(lane));
        }
    }
}

impl Session for CopilotSession {
    type Message = CopilotMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages_from(
        self,
        from: Option<Checkpoint>,
    ) -> impl Stream<Item = Result<(Checkpoint, CopilotMessage), MessageError>> + Send + 'static
    {
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

    fn read(&self) -> impl Stream<Item = Result<CopilotMessage, MessageError>> + Send + 'static {
        Self {
            changes: None,
            announce: None,
            ..self.clone()
        }
        .messages_at(0, false)
        .map(|item| item.map(|(_, message)| message))
    }

    async fn changed_after(mut self, since: SystemTime) -> Option<Self> {
        modified_after(self.changes.as_mut(), since).await?;
        Some(self)
    }
}

/// One line of the log: its envelope, with the event's `data` kept as JSON and read as needed, so
/// a line whose shape a newer Copilot changed still parses.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    timestamp: Option<String>,
    #[serde(default)]
    data: serde_json::Value,
}

impl Envelope {
    /// A line, read as Copilot (a Node program) wrote it (see [`crate::json::js`]).
    fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        crate::json::js::from_slice(bytes)
    }

    fn str(&self, field: &str) -> Option<&str> {
        self.data[field].as_str()
    }

    fn interaction(&self) -> Option<&str> {
        self.str("interactionId")
    }
}

/// A subagent a log holds (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lane {
    /// The `task` call that spawned it.
    call: String,
    /// The agent's name, as Copilot shows it (`agentDisplayName`, else `agentName`).
    name: Option<String>,
    /// Its interaction, once its first row (its prompt) said.
    interaction: Option<String>,
    /// The id of its last row, which its next row names as its parent.
    last_row: Option<String>,
}

impl Lane {
    /// The subagent's name as its session's title.
    fn title(&self) -> Option<TitleChange> {
        self.name.as_deref().map(|name| TitleChange::new(TitleSource::Agent, name))
    }
}

/// The subagents the log at `path` holds, in the order they started. Blocking.
fn lanes_in(path: &Path) -> io::Result<Vec<Lane>> {
    Ok(Reading::before(path, u64::MAX)?.lanes)
}

/// The parent's rows of the log at `path`, holding `bytes`, as a read of the whole log yields
/// them (the sidecar's title row after the first line included), for sync to key as capture
/// does. Blocking; a line that doesn't parse is skipped, as the readers skip it.
pub(crate) fn read_log(path: &Path, bytes: &[u8]) -> Vec<CopilotMessage> {
    let mut reading = Reading::default();
    let mut sidecar = SidecarWatch::new(path.with_file_name(SIDECAR_FILE));
    let mut out = Vec::new();
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.iter().all(u8::is_ascii_whitespace)) {
        let Ok(event) = Envelope::decode(line) else {
            continue;
        };
        if let Some((None, message)) = reading.absorb(event) {
            out.push(message);
        }
        if let Some((title, at)) = sidecar.refresh() {
            out.push(reading.title_row(title, at));
        }
    }
    out
}

/// What the reader carries from one event to the ones after it.
#[derive(Debug, Default)]
struct Reading {
    /// Where the session works, from the last `session.start` or `session.resume`.
    cwd: Option<PathBuf>,
    git_branch: Option<String>,
    /// The session this one was forked from, as its `session.start` says (a fork atuin wrote).
    fork_of: Option<SessionId>,
    /// The subagents started so far, in order.
    lanes: Vec<Lane>,
    /// The id of the parent's last row, which its next row names as its parent.
    last_row: Option<String>,
    /// The last `session.usage_record`: the model call it is for and its usage, claimed by the
    /// `assistant.message` of that call, which Copilot writes right after it.
    usage: Option<(String, Usage)>,
}

impl Reading {
    /// The state as of byte `start` of the log at `path`: what the events before it say, for a
    /// read that resumes from a checkpoint there. A line that doesn't parse says nothing.
    fn before(path: &Path, start: u64) -> io::Result<Self> {
        let mut reading = Self::default();
        if start == 0 {
            return Ok(reading);
        }
        let lines = BufReader::new(File::open(path)?.take(start));
        for line in lines.split(b'\n') {
            if let Ok(event) = Envelope::decode(&line?) {
                reading.absorb(event);
            }
        }
        Ok(reading)
    }

    /// The subagent `event` is a row of, by its interaction, or for a reply by the call it
    /// names (which also tells the reader the interaction, when the reply comes first).
    fn lane_of(&mut self, event: &Envelope) -> Option<usize> {
        if let Some(call) = event.str("parentToolCallId") {
            let found = self.lanes.iter().position(|lane| lane.call == call)?;
            if self.lanes[found].interaction.is_none() {
                self.lanes[found].interaction = event.interaction().map(str::to_owned);
            }
            return Some(found);
        }
        let interaction = event.interaction()?;
        self.lanes.iter().position(|lane| lane.interaction.as_deref() == Some(interaction))
    }

    /// Take `event` in: the message it is, with the subagent (the `task` call) whose row it is
    /// (`None` for the parent's); or `None` for an event the reader skips.
    fn absorb(&mut self, event: Envelope) -> Option<(Option<String>, CopilotMessage)> {
        match event.kind.as_str() {
            "session.start" | "session.resume" => {
                let context = &event.data["context"];
                if let Some(cwd) = context["cwd"].as_str() {
                    self.cwd = Some(PathBuf::from(cwd));
                }
                if let Some(branch) = context["branch"].as_str() {
                    self.git_branch = Some(branch.to_owned());
                }
                if let Some(of) = event.data["atuin"]["forkedFrom"].as_str() {
                    self.fork_of = Some(SessionId::from(of.to_owned()));
                }
                None
            }
            "session.usage_record" => {
                let usage = &event.data["usage"];
                if let Some(call) = usage["apiCallId"].as_str() {
                    self.usage = Some((call.to_owned(), usage_of(usage)));
                }
                None
            }
            "subagent.started" => {
                if let Some(call) = event.str("toolCallId") {
                    let name = event.str("agentDisplayName").or_else(|| event.str("agentName"));
                    self.lanes.push(Lane {
                        call: call.to_owned(),
                        name: name.map(str::to_owned),
                        interaction: None,
                        last_row: None,
                    });
                }
                None
            }
            "user.message" => match event.str("source") {
                // A subagent's prompt opens its interaction: the first started that has none
                // yet, as Copilot starts them in order.
                Some(source) if source.starts_with(SUBAGENT_PREFIX) => {
                    let interaction = event.interaction()?.to_owned();
                    let lane = self.lanes.iter_mut().find(|lane| lane.interaction.is_none())?;
                    lane.interaction = Some(interaction);
                    let call = lane.call.clone();
                    Some(self.row(Some(call), event, None))
                }
                // Text atuin put in the user's turn (a continuation's marker): the model's to
                // read, not what the user said.
                Some(source) if source == ATUIN_SOURCE => {
                    let owner = self.owner_of(&event);
                    Some(self.row(owner, event, None))
                }
                Some(_) => None,
                None if event.str("content").is_none_or(str::is_empty) => None,
                None => {
                    let owner = self.owner_of(&event);
                    Some(self.row(owner, event, None))
                }
            },
            "assistant.message" => {
                let owner = self.owner_of(&event);
                let usage = match (event.str("apiCallId"), &self.usage) {
                    (Some(call), Some((of, _))) if call == of => self.usage.take().map(|(_, u)| u),
                    _ => None,
                };
                Some(self.row(owner, event, usage))
            }
            "tool.execution_complete" => {
                let owner = self.owner_of(&event);
                Some(self.row(owner, event, None))
            }
            // A compaction that failed, or was abandoned, changed nothing.
            "session.compaction_complete"
                if event.data["success"] == true && event.str("outcome") == Some("applied") =>
            {
                Some(self.row(None, event, None))
            }
            _ => None,
        }
    }

    /// The row `event` is, in the lane of `owner` (a subagent's `task` call, or `None` for the
    /// parent's): named after the lane's last row, and the lane's last row from now on.
    fn row(
        &mut self,
        owner: Option<String>,
        event: Envelope,
        usage: Option<Usage>,
    ) -> (Option<String>, CopilotMessage) {
        let mut message = self.message(event, usage);
        let last = match &owner {
            Some(call) => match self.lanes.iter_mut().find(|lane| lane.call == *call) {
                Some(lane) => &mut lane.last_row,
                None => &mut self.last_row,
            },
            None => &mut self.last_row,
        };
        message.parent_row.clone_from(last);
        if let Some(id) = &message.event.id {
            *last = Some(id.clone());
        }
        (owner, message)
    }

    /// The subagent whose row `event` is, by its `task` call; `None` for the parent's.
    fn owner_of(&mut self, event: &Envelope) -> Option<String> {
        self.lane_of(event).map(|at| self.lanes[at].call.clone())
    }

    fn message(&self, event: Envelope, usage: Option<Usage>) -> CopilotMessage {
        CopilotMessage {
            event,
            cwd: self.cwd.clone(),
            git_branch: self.git_branch.clone(),
            usage,
            title: None,
            at: None,
            parent: self.fork_of.clone().map(|of| (of, ParentKind::Fork)),
            parent_row: None,
        }
    }

    /// A row carrying `title`, which the sidecar gave the session at `at` (see the module docs).
    /// Its id is the name's alone, so the same name read again, here or from a copy of the
    /// session written elsewhere, is the same row; keyed on its content ([`SYNTHETIC`]), it is
    /// no node of the session's tree (see the module docs).
    fn title_row(&self, title: TitleChange, at: Option<SystemTime>) -> CopilotMessage {
        let at = at.map(OffsetDateTime::from);
        let id = format!(
            "{SYNTHETIC}title-{:016x}",
            xxhash_rust::xxh3::xxh3_64(title.text.as_deref().unwrap_or_default().as_bytes())
        );
        CopilotMessage {
            event: Envelope {
                kind: TITLE_ROW.to_owned(),
                id: Some(id),
                timestamp: None,
                data: serde_json::Value::Null,
            },
            cwd: None,
            git_branch: None,
            usage: None,
            title: Some(title),
            at,
            parent: self.fork_of.clone().map(|of| (of, ParentKind::Fork)),
            parent_row: None,
        }
    }
}

/// The kind of the reader's own title rows, which no Copilot event has: their role, as Claude
/// Code's title lines are read under theirs.
const TITLE_ROW: &str = "atuin-title";

/// A `session.usage_record`'s `usage` as [`Usage`] counts it: Copilot's `inputTokens` is the
/// whole input, cached included, so the cached part is taken out of it.
fn usage_of(usage: &serde_json::Value) -> Usage {
    let field = |name: &str| usage[name].as_u64();
    let (cache_read, cache_write) = (field("cacheReadTokens"), field("cacheWriteTokens"));
    let input = field("inputTokens").map(|input| {
        input.saturating_sub(cache_read.unwrap_or(0)).saturating_sub(cache_write.unwrap_or(0))
    });
    Usage {
        input,
        output: field("outputTokens"),
        cache_read,
        cache_write,
        reasoning: field("reasoningTokens"),
    }
}

/// What `workspace.yaml` says of the session. Copilot writes it as plain `key: value` lines, so
/// it is read as such, without a YAML parser.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Sidecar {
    name: Option<String>,
    user_named: bool,
}

impl Sidecar {
    fn parse(text: &str) -> Self {
        let mut sidecar = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            let value = match value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
                Some(quoted) => unescape(quoted),
                None => value
                    .strip_prefix('\'')
                    .and_then(|v| v.strip_suffix('\''))
                    .unwrap_or(value)
                    .to_owned(),
            };
            match key.trim() {
                "name" => sidecar.name = Some(value),
                "user_named" => sidecar.user_named = value == "true",
                _ => {}
            }
        }
        sidecar
    }

    /// The session's name as a title: the user's when they set it, else Copilot's.
    fn title(&self) -> Option<TitleChange> {
        let source = if self.user_named {
            TitleSource::Named
        } else {
            TitleSource::Generated
        };
        self.name.as_deref().map(|name| TitleChange::new(source, name))
    }
}

/// The text of a double-quoted YAML scalar `quoted`, its escapes decoded: the ones the writer
/// and Copilot use (`\"`, `\\`, `\n`, `\r`, `\t`); any other backslash is kept as it is.
fn unescape(quoted: &str) -> String {
    let mut out = String::with_capacity(quoted.len());
    let mut chars = quoted.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Follows the sidecar: what its title was when last read, re-read when the file changes.
#[derive(Debug)]
struct SidecarWatch {
    path: PathBuf,
    modified: Option<SystemTime>,
    title: Option<TitleChange>,
}

impl SidecarWatch {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            modified: None,
            title: None,
        }
    }

    /// The title the sidecar now gives, and when it was written, when the title changed since
    /// the last call (or on the first): `None` when it hasn't, or the sidecar can't be read.
    /// Blocking.
    fn refresh(&mut self) -> Option<(TitleChange, Option<SystemTime>)> {
        let modified = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if modified.is_some() && modified == self.modified {
            return None;
        }
        self.modified = modified;
        let title = Sidecar::parse(&std::fs::read_to_string(&self.path).ok()?).title();
        if title == self.title {
            return None;
        }
        self.title = title;
        self.title.clone().map(|title| (title, modified))
    }
}

/// One conversation event of a Copilot session's log (see the module docs).
#[derive(Debug, Clone)]
pub struct CopilotMessage {
    event: Envelope,
    /// Where the session works, as the log said last (`session.start`, `session.resume`).
    cwd: Option<PathBuf>,
    git_branch: Option<String>,
    /// The model call's usage, on an `assistant.message`, from the `session.usage_record`
    /// before it.
    usage: Option<Usage>,
    /// On a title row ([`TITLE_ROW`]), the session's name from the sidecar; on a subagent's
    /// first row, the agent's name.
    title: Option<TitleChange>,
    /// When a title row's sidecar was written: the row's time.
    at: Option<OffsetDateTime>,
    /// On a subagent's rows, the session that spawned it; on a fork's, the session it was
    /// forked from.
    parent: Option<(SessionId, ParentKind)>,
    /// The row before this one in its lane (see the module docs); none on a title row.
    parent_row: Option<String>,
}

impl CopilotMessage {
    fn kind(&self) -> &str {
        &self.event.kind
    }

    /// The tool calls an `assistant.message` requested.
    fn tool_requests(&self) -> impl Iterator<Item = &serde_json::Value> {
        self.event.data["toolRequests"].as_array().into_iter().flatten()
    }

    /// Whether an `assistant.message` reasoned: Copilot keeps a summary when the model gives
    /// one, and the rest encrypted.
    fn reasoned(&self) -> bool {
        ["reasoningText", "reasoningOpaque", "encryptedContent"]
            .iter()
            .any(|field| self.event.str(field).is_some_and(|text| !text.is_empty()))
    }

    /// The result of a `tool.execution_complete`.
    fn tool_result(&self) -> ToolResult {
        let result = &self.event.data["result"];
        let output = match result.get("content") {
            Some(content) => content.clone(),
            None => result.clone(),
        };
        ToolResult {
            call: ToolCallId::from(self.event.str("toolCallId").unwrap_or_default().to_owned()),
            output,
            error: self.event.data["success"].as_bool() == Some(false),
        }
    }

    /// The files an edit changed (`fileEdits`), with the hunks of the diff Copilot shows for it
    /// (`result.detailedContent`, a git-style diff whose `a/` and `b/` prefixes swallow the
    /// paths' leading `/`, so the paths come from `fileEdits`).
    fn patch(&self, call: &ToolCallId) -> Option<Patch> {
        let edits = self.event.data["fileEdits"].as_array().filter(|edits| !edits.is_empty())?;
        let diff = self.event.data["result"]["detailedContent"].as_str().unwrap_or_default();
        let mut diffed = FilePatch::from_diff(diff).into_iter();
        let files = edits
            .iter()
            .filter_map(|edit| {
                let path = edit["path"].as_str()?.to_owned();
                let change = match edit["kind"].as_str() {
                    Some("create") => Change::Add,
                    Some("delete") => Change::Delete,
                    _ => Change::Update,
                };
                let hunks = diffed.next().map(|file| file.hunks).unwrap_or_default();
                Some(FilePatch {
                    path,
                    change,
                    moved_to: None,
                    hunks,
                })
            })
            .collect();
        Some(Patch {
            call: call.clone(),
            files,
        })
    }
}

impl Message for CopilotMessage {
    fn id(&self) -> Option<MessageId> {
        self.event.id.clone().map(MessageId::from)
    }

    fn role(&self) -> Role {
        match self.kind() {
            "user.message" if self.event.str("source") == Some(ATUIN_SOURCE) => Role::System,
            "user.message" => Role::User,
            "assistant.message" => Role::Assistant,
            "tool.execution_complete" => Role::Tool,
            "session.compaction_complete" => Role::System,
            other => Role::Other(other.to_owned()),
        }
    }

    fn parent_id(&self) -> Option<MessageId> {
        self.parent_row.clone().map(MessageId::from)
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.at.or_else(|| {
            self.event.timestamp.as_deref().and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        })
    }

    fn content(&self) -> Vec<Content> {
        match self.kind() {
            "user.message" => {
                // What atuin put in the user's turn is the transformed text, as the model
                // reads it.
                let field = if self.event.str("source") == Some(ATUIN_SOURCE) {
                    "transformedContent"
                } else {
                    "content"
                };
                self.event
                    .str(field)
                    .map(|text| Content::Text(text.to_owned()))
                    .into_iter()
                    .collect()
            }
            "assistant.message" => {
                let mut content = Vec::new();
                if self.reasoned() {
                    content.push(Content::ReasoningSummary {
                        tokens: self.usage.and_then(|usage| usage.reasoning),
                    });
                }
                if let Some(text) = self.event.str("content").filter(|text| !text.is_empty()) {
                    content.push(Content::Text(text.to_owned()));
                }
                content.extend(self.tool_requests().map(|request| {
                    Content::ToolUse(ToolUse {
                        id: ToolCallId::from(
                            request["toolCallId"].as_str().unwrap_or_default().to_owned(),
                        ),
                        name: request["name"].as_str().unwrap_or_default().to_owned(),
                        input: request["arguments"].clone(),
                    })
                }));
                content
            }
            "tool.execution_complete" => {
                let result = self.tool_result();
                let patch = self.patch(&result.call);
                std::iter::once(Content::ToolResult(result))
                    .chain(patch.map(Content::Patch))
                    .collect()
            }
            "session.compaction_complete" => self
                .event
                .str("summaryContent")
                .map(|summary| Content::Summary(summary.to_owned()))
                .into_iter()
                .collect(),
            _ => Vec::new(),
        }
    }

    fn model(&self) -> Option<String> {
        (self.kind() == "assistant.message")
            .then(|| self.event.str("model").map(str::to_owned))
            .flatten()
    }

    fn provider(&self) -> Option<String> {
        (self.kind() == "assistant.message")
            .then(|| self.event.str("providerId").map(str::to_owned))
            .flatten()
    }

    fn usage(&self) -> Option<Usage> {
        self.usage
    }

    fn stop_reason(&self) -> Option<StopReason> {
        if self.kind() != "assistant.message" {
            return None;
        }
        Some(if self.tool_requests().next().is_some() {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        })
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.cwd.clone()
    }

    fn git_branch(&self) -> Option<String> {
        self.git_branch.clone()
    }

    /// The model call (`apiCallId`), which the usage record before the message names too.
    fn turn_id(&self) -> Option<String> {
        (self.kind() == "assistant.message")
            .then(|| self.event.str("apiCallId").map(str::to_owned))
            .flatten()
    }

    fn parent_session(&self) -> Option<SessionId> {
        self.parent.as_ref().map(|(session, _)| session.clone())
    }

    fn parent_kind(&self) -> Option<ParentKind> {
        self.parent.as_ref().map(|(_, kind)| *kind)
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

    const TASK_AND_EDIT: &str = include_str!("../../../tests/fixtures/copilot/task-and-edit.jsonl");
    const TASK_AND_EDIT_SIDECAR: &str =
        include_str!("../../../tests/fixtures/copilot/task-and-edit.workspace.yaml");
    const REASONING: &str = include_str!("../../../tests/fixtures/copilot/reasoning.jsonl");
    const PROBE: &str = include_str!("../../../tests/fixtures/copilot/probe.jsonl");
    const COMPACTED: &str = include_str!("../../../tests/fixtures/copilot/compacted.jsonl");
    const COMPACTED_SIDECAR: &str =
        include_str!("../../../tests/fixtures/copilot/compacted.workspace.yaml");

    fn pool() -> BlockingPool {
        BlockingPool::new(NonZeroUsize::MIN)
    }

    /// A sessions directory holding one session, `id`, with `events` and `sidecar`.
    fn root_with(id: &str, events: &str, sidecar: Option<&str>) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(id);
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join(EVENTS_FILE), events).unwrap();
        if let Some(sidecar) = sidecar {
            std::fs::write(dir.join(SIDECAR_FILE), sidecar).unwrap();
        }
        root
    }

    async fn read_all(root: &Path, id: &str) -> Vec<CopilotMessage> {
        let path = root.join(id).joined(EVENTS_FILE);
        CopilotSession::open(SessionId::from(id.to_owned()), path, pool())
            .messages()
            .try_collect()
            .await
            .unwrap()
    }

    /// The parent's messages of a log, read without a sidecar.
    fn parse(events: &str) -> Vec<CopilotMessage> {
        parse_lane(events, None)
    }

    /// The messages of `lane`'s subagent (`None`: the parent's) in a log, read without a
    /// sidecar.
    fn parse_lane(events: &str, lane: Option<&str>) -> Vec<CopilotMessage> {
        let mut reading = Reading::default();
        events
            .lines()
            .filter_map(|line| reading.absorb(Envelope::decode(line.as_bytes()).unwrap()))
            .filter(|(owner, _)| owner.as_deref() == lane)
            .map(|(_, message)| message)
            .collect()
    }

    fn texts(messages: &[CopilotMessage], role: &Role) -> Vec<String> {
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

    #[rstest]
    #[case::log("s/events.jsonl", true)]
    #[case::sidecar("s/workspace.yaml", false)]
    #[case::lock("s/inuse.1.lock", false)]
    #[case::nested("s/checkpoints/events.jsonl", false)]
    #[case::operation_lock(".session-operation-locks/s.lock", false)]
    #[case::at_root("events.jsonl", false)]
    fn event_logs_are_the_sessions(#[case] relative: &str, #[case] expected: bool) {
        let root = Path::new("/sessions");
        assert_eq!(is_transcript(root, &root.join(relative)), expected);
        assert!(!is_transcript(root, Path::new("/elsewhere/s/events.jsonl")));
    }

    #[rstest]
    fn session_id_is_the_directory_name() {
        let path = Path::new("/sessions/8623b9dc-b2aa-40dc-a8ce-1f3738d4d4db/events.jsonl");
        assert_eq!(
            CopilotListener::session_id(path).unwrap().as_ref(),
            "8623b9dc-b2aa-40dc-a8ce-1f3738d4d4db"
        );
    }

    #[rstest]
    fn locate_finds_the_log_by_id() {
        let root = root_with("abc", PROBE, None);
        assert_eq!(locate(root.path(), "abc"), Some(root.path().join("abc").joined(EVENTS_FILE)));
        assert_eq!(locate(root.path(), "missing"), None);
        assert_eq!(locate(root.path(), "../abc"), None, "no path escapes the root");
    }

    /// The parent's conversation only: its prompts, replies and tool results, in order, with
    /// the subagent's rows (its prompt from the parent, its reply and its own `bash` call)
    /// left out.
    #[rstest]
    fn keeps_the_parents_conversation_and_skips_the_subagents() {
        let messages = parse(TASK_AND_EDIT);
        let roles: Vec<_> = messages.iter().map(Message::role).collect();
        assert_eq!(roles, [
            Role::User,
            Role::Assistant,
            Role::Tool,
            Role::Tool,
            Role::Assistant,
            Role::Tool,
            Role::Assistant,
            Role::Tool,
            Role::Assistant,
            // Resumed from another directory: a second prompt.
            Role::User,
            Role::Assistant,
            Role::Tool,
            Role::Assistant,
        ]);
        let user = texts(&messages, &Role::User);
        assert!(user[0].starts_with("Do three things in this repo"), "{user:?}");
        assert_eq!(
            user.len(),
            2,
            "the subagent's prompt is the parent's `task` call, not a prompt"
        );
        let assistant = texts(&messages, &Role::Assistant);
        assert!(!assistant.iter().any(|text| text == "2"), "the subagent's reply is its result");
    }

    #[rstest]
    fn assistant_message_holds_its_calls_and_usage() {
        let messages = parse(TASK_AND_EDIT);
        let first = &messages[1];
        let content = first.content();
        assert!(!matches!(content[0], Content::ReasoningSummary { .. }), "{content:?}");
        let calls: Vec<_> = content
            .iter()
            .filter_map(|c| match c {
                Content::ToolUse(call) => Some(call.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(calls, ["task", "view"]);
        assert_eq!(first.model().as_deref(), Some("claude-sonnet-5.5"));
        assert_eq!(first.provider().as_deref(), Some("copilot"));
        assert_eq!(first.stop_reason(), Some(StopReason::ToolUse));
        let usage = first.usage().expect("the usage record before the message is its usage");
        assert_eq!(usage.output, Some(368));
        assert_eq!(usage.cache_read, Some(0));
        assert_eq!(usage.cache_write, Some(30345));
        assert_eq!(usage.input, Some(4), "the cached input is taken out of the total");
        assert_eq!(usage.reasoning, Some(0));
        assert_eq!(first.turn_id().as_deref(), Some("msg_011CgTD8KcDkGFTtVmMFgF5b"));
        assert!(
            messages.iter().filter(|m| m.role() == Role::Assistant).all(|m| m.usage().is_some())
        );
        let last = messages.last().unwrap();
        assert_eq!(last.stop_reason(), Some(StopReason::EndTurn));
    }

    /// A reply that reasoned is marked once, with the call's reasoning tokens, before its text,
    /// whether Copilot kept a readable summary of the reasoning or only its encrypted form.
    #[rstest]
    fn a_reply_that_reasoned_carries_a_marker_with_its_tokens() {
        let messages = parse(REASONING);
        let replies: Vec<_> = messages.iter().filter(|m| m.role() == Role::Assistant).collect();
        let content = replies[0].content();
        assert!(
            matches!(content[0], Content::ReasoningSummary { tokens: Some(10) }),
            "{content:?}"
        );
        assert!(
            matches!(&content[1], Content::Text(text) if text.starts_with("I\u{2019}ll inspect")),
            "{content:?}"
        );
        assert_eq!(replies[0].usage().unwrap().reasoning, Some(10));
        assert!(matches!(replies[1].content()[0], Content::ReasoningSummary { .. }));
    }

    #[rstest]
    fn tool_results_name_their_call_and_an_edit_its_patch() {
        let messages = parse(TASK_AND_EDIT);
        let results: Vec<_> =
            messages.iter().filter(|m| m.role() == Role::Tool).map(|m| m.content()).collect();
        let task = results
            .iter()
            .find_map(|content| match &content[0] {
                Content::ToolResult(r) if r.call.as_ref() == "toolu_01Cz14Gbx74QYDbB2WUL3D5T" => {
                    Some(r)
                }
                _ => None,
            })
            .expect("the `task` call's result is the subagent's answer");
        assert_eq!(task.output_text().as_deref(), Some("2"));
        assert!(!task.error);
        let edit = results.iter().find(|content| content.len() == 2).expect("the edit has a patch");
        let Content::Patch(patch) = &edit[1] else {
            panic!("{edit:?}");
        };
        assert_eq!(patch.files.len(), 1);
        assert_eq!(patch.files[0].path, "/tmp/copilot-play/hello.py");
        assert_eq!(patch.files[0].change, Change::Update);
        assert_eq!(patch.files[0].hunks.len(), 1);
        assert!(patch.files[0].hunks[0].lines.iter().any(|l| l == "+    return f\"hi {name}\""));
    }

    #[rstest]
    fn messages_carry_the_sessions_directory_and_branch() {
        let messages = parse(TASK_AND_EDIT);
        assert!(
            messages.iter().all(|m| m.cwd().as_deref() == Some(Path::new("/tmp/copilot-play")))
        );
        assert!(messages.iter().all(|m| m.git_branch().as_deref() == Some("main")));
        assert!(messages.iter().all(|m| m.timestamp().is_some()));
        assert!(messages.iter().all(|m| m.id().is_some()));
    }

    /// Each row names the row before it in its lane: the parent's rows chain past the
    /// subagent's, and the subagent's among themselves from a root of their own.
    #[rstest]
    fn rows_name_the_row_before_them_in_their_lane() {
        for lane in [None, Some(TASK_CALL)] {
            let messages = parse_lane(TASK_AND_EDIT, lane);
            assert!(messages.len() > 1, "{lane:?}");
            assert_eq!(messages[0].parent_id(), None, "{lane:?}: a root");
            for pair in messages.windows(2) {
                assert_eq!(pair[1].parent_id(), pair[0].id(), "{lane:?}");
            }
        }
    }

    /// Copilot's injected rows are not the user's: the `memory` it adds before a prompt (an
    /// interactive session's `user.message` with `source: "memory"`, its text in
    /// `transformedContent`), the system prompt, and its lifecycle events.
    #[rstest]
    #[case::memory(json!({"type": "user.message", "data": {"content": "", "transformedContent": "<memories>\nNone.\n</memories>", "source": "memory", "interactionId": "i1"}}))]
    #[case::system_prompt(json!({"type": "system.message", "data": {"role": "system", "content": "You are the GitHub Copilot CLI", "interactionId": "i1"}}))]
    #[case::folder_trust(json!({"type": "session.info", "data": {"message": "Folder /w has been added to trusted folders.", "infoType": "folder_trust"}}))]
    #[case::turn(json!({"type": "assistant.turn_start", "data": {"turnId": "0", "interactionId": "i1"}}))]
    #[case::permission(json!({"type": "permission.requested", "data": {"requestId": "r1", "permissionRequest": {"kind": "shell"}}}))]
    #[case::empty_prompt(json!({"type": "user.message", "data": {"content": "", "interactionId": "i1"}}))]
    fn injected_rows_are_skipped(#[case] event: serde_json::Value) {
        let mut line =
            json!({"id": "e1", "parentId": null, "timestamp": "2026-10-27T18:18:27.862Z"});
        line.as_object_mut().unwrap().extend(event.as_object().unwrap().clone());
        let event = Envelope::decode(line.to_string().as_bytes()).unwrap();
        assert!(Reading::default().absorb(event).is_none());
    }

    /// `/compact` leaves a summary standing in for the conversation before it, a system row;
    /// `/rename` writes no event, only the sidecar, which names the session with the user's
    /// title. The session resumed after both goes on in the same log.
    #[rstest]
    #[tokio::test]
    async fn a_compaction_is_a_summary_and_a_rename_only_the_sidecar() {
        let root = root_with("s", COMPACTED, Some(COMPACTED_SIDECAR));
        let messages = read_all(root.path(), "s").await;
        let roles: Vec<_> = messages.iter().map(Message::role).collect();
        assert_eq!(roles, [
            // The sidecar's name, with the log's first line.
            Role::Other(TITLE_ROW.to_owned()),
            Role::User,
            Role::Assistant,
            Role::Tool,
            Role::Tool,
            Role::Assistant,
            Role::Tool,
            Role::Assistant,
            Role::System,
            Role::User,
            Role::Assistant,
        ]);
        assert_eq!(texts(&messages, &Role::User), ["What is this folder?", "Hi!"]);
        let summary = &messages[8];
        assert!(
            matches!(&summary.content()[..], [Content::Summary(text)] if text.starts_with("<overview>")),
            "{:?}",
            summary.content()
        );
        assert!(summary.usage().is_none());
        assert_eq!(messages.iter().filter_map(Message::title).collect::<Vec<_>>(), [
            TitleChange::new(TitleSource::Named, "CopilotTestRun")
        ]);
    }

    #[rstest]
    #[case::failed(json!({"success": false, "outcome": "failed", "summaryContent": "x"}))]
    #[case::not_applied(json!({"success": true, "outcome": "skipped", "summaryContent": "x"}))]
    fn a_compaction_that_changed_nothing_is_skipped(#[case] data: serde_json::Value) {
        let line = json!({"type": "session.compaction_complete", "id": "e1", "parentId": null,
            "timestamp": "2026-10-27T23:35:54.000Z", "data": data});
        let event = Envelope::decode(line.to_string().as_bytes()).unwrap();
        assert!(Reading::default().absorb(event).is_none());
    }

    const TASK_CALL: &str = "toolu_01Cz14Gbx74QYDbB2WUL3D5T";

    /// The subagent's own conversation: the prompt its parent wrote, its reply calling `bash`,
    /// the result, and its answer, each row naming the parent session, with the agent's name
    /// as its title.
    #[rstest]
    #[tokio::test]
    async fn a_subagent_is_a_child_session_over_the_same_log() {
        let root = root_with("s", TASK_AND_EDIT, Some(TASK_AND_EDIT_SIDECAR));
        let path = root.path().join("s").joined(EVENTS_FILE);
        let parent = CopilotSession::open(SessionId::from("s".to_owned()), path, pool());
        let lane = Lane {
            call: TASK_CALL.to_owned(),
            name: Some("count-lines".to_owned()),
            interaction: None,
            last_row: None,
        };
        let child = parent.child(&lane);
        assert_eq!(child.id().as_ref(), format!("agent-{TASK_CALL}"));
        let messages: Vec<CopilotMessage> = child.messages().try_collect().await.unwrap();
        let roles: Vec<_> = messages.iter().map(Message::role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::Tool, Role::Assistant]);
        let user = texts(&messages, &Role::User);
        assert!(user[0].starts_with("Count the lines in hello.py"), "{user:?}");
        assert_eq!(texts(&messages, &Role::Assistant), ["2"]);
        assert!(
            messages.iter().all(|m| m.parent_session().as_ref().map(AsRef::as_ref) == Some("s"))
        );
        assert!(messages.iter().all(|m| m.parent_kind() == Some(ParentKind::Subagent)));
        assert!(
            messages.iter().all(|m| m.cwd().as_deref() == Some(Path::new("/tmp/copilot-play")))
        );
        assert_eq!(messages.iter().filter_map(Message::title).collect::<Vec<_>>(), [
            TitleChange::new(TitleSource::Agent, "count-lines")
        ]);
        assert!(
            messages.iter().filter(|m| m.role() == Role::Assistant).all(|m| m.usage().is_some())
        );
        let parent_rows: Vec<CopilotMessage> = parent.messages().try_collect().await.unwrap();
        assert!(parent_rows.iter().all(|m| m.parent_session().is_none()));
        // The sidecar's title row names no parent; the rest chain past the subagent's rows.
        let (titles, rows): (Vec<_>, Vec<_>) =
            parent_rows.iter().partition(|m| m.title().is_some());
        assert!(titles.iter().all(|m| m.parent_id().is_none()));
        for pair in rows.windows(2) {
            assert_eq!(pair[1].parent_id(), pair[0].id());
        }
        // The parent's rows and its title row, and the subagent's four.
        assert_eq!(
            parent_rows.len() + messages.len(),
            parse_lane(TASK_AND_EDIT, None).len() + 1 + 4
        );
    }

    /// A subagent's rows are told from the parent's by the call its replies name, or the
    /// interaction its prompt opened, so a reply whose prompt was not seen still finds its lane.
    #[rstest]
    fn lanes_are_found_by_call_or_interaction() {
        let mut reading = Reading::default();
        let event =
            |value: serde_json::Value| Envelope::decode(value.to_string().as_bytes()).unwrap();
        assert!(reading.absorb(event(json!({"type": "subagent.started", "id": "e1", "timestamp": "2026-10-27T00:00:00Z",
            "data": {"toolCallId": "call_1", "agentName": "task", "agentDisplayName": "worker"}}))).is_none());
        let (owner, _) = reading.absorb(event(json!({"type": "assistant.message", "id": "e2", "timestamp": "2026-10-27T00:00:01Z",
            "data": {"messageId": "m1", "content": "hi", "toolRequests": [], "interactionId": "i-sub", "parentToolCallId": "call_1"}}))).unwrap();
        assert_eq!(owner.as_deref(), Some("call_1"));
        let (owner, _) = reading.absorb(event(json!({"type": "tool.execution_complete", "id": "e3", "timestamp": "2026-10-27T00:00:02Z",
            "data": {"toolCallId": "c", "success": true, "result": {"content": "x"}, "interactionId": "i-sub"}}))).unwrap();
        assert_eq!(owner.as_deref(), Some("call_1"), "the interaction the reply named");
        let (owner, _) = reading.absorb(event(json!({"type": "tool.execution_complete", "id": "e4", "timestamp": "2026-10-27T00:00:03Z",
            "data": {"toolCallId": "d", "success": true, "result": {"content": "y"}, "interactionId": "i-main"}}))).unwrap();
        assert_eq!(owner, None, "the parent's");
        assert!(lanes_in(Path::new("/no/such")).is_err());
    }

    #[rstest]
    #[tokio::test]
    async fn existing_lists_subagents_after_their_parent() {
        let root = root_with("s", TASK_AND_EDIT, None);
        let sessions =
            CopilotSessions::builder().root(root.path().to_path_buf()).pool(pool()).build();
        let ids: Vec<String> = sessions
            .existing()
            .unwrap()
            .map_ok(|s| s.id().to_string())
            .try_collect()
            .await
            .unwrap();
        assert_eq!(ids, ["s", &format!("agent-{TASK_CALL}")]);
    }

    /// A parent's reader announces the subagents it comes across, the ones before its
    /// checkpoint included, and the listener reports each once.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_listener_reports_a_subagent_its_parent_comes_across() {
        let root = root_with("s", TASK_AND_EDIT, None);
        let sessions =
            CopilotSessions::builder().root(root.path().to_path_buf()).pool(pool()).build();
        let listener = sessions.listener().unwrap();
        let mut watched = Box::pin(listener.watch());
        let parent = watched.next().await.unwrap().unwrap();
        assert_eq!(parent.id().as_ref(), "s");
        // Reading the parent past the subagent's start announces it (the third row the parent
        // yields, the `view` result, comes after it).
        let _: Vec<_> = parent.clone().messages_from(None).take(3).try_collect().await.unwrap();
        let child = tokio::time::timeout(std::time::Duration::from_secs(5), watched.next())
            .await
            .expect("the child is reported")
            .unwrap()
            .unwrap();
        assert_eq!(child.id().as_ref(), format!("agent-{TASK_CALL}"));
        assert_eq!(child.parent_id().unwrap().as_ref(), "s");
        // Read again, the parent announces it again; the listener does not report it twice.
        let _: Vec<_> = parent.clone().messages_from(None).take(3).try_collect().await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(300), watched.next())
                .await
                .is_err(),
            "reported once"
        );

        // The log removed and made again is a fresh watch: the parent is reported again, and
        // once its reader announces the child, so is the child, whose old watch ended.
        let path = root.path().join("s").joined(EVENTS_FILE);
        let mut ended = parent.changes.clone().unwrap();
        std::fs::remove_file(&path).unwrap();
        while ended.changed().await.is_ok() {}
        std::fs::write(&path, TASK_AND_EDIT).unwrap();
        let again = tokio::time::timeout(std::time::Duration::from_secs(5), watched.next())
            .await
            .expect("the recreated log is reported")
            .unwrap()
            .unwrap();
        assert_eq!(again.id().as_ref(), "s");
        let _: Vec<_> = again.messages_from(None).take(3).try_collect().await.unwrap();
        let child = tokio::time::timeout(std::time::Duration::from_secs(5), watched.next())
            .await
            .expect("the child is reported again")
            .unwrap()
            .unwrap();
        assert_eq!(child.id().as_ref(), format!("agent-{TASK_CALL}"));
    }

    #[rstest]
    fn a_failed_command_is_an_ok_result_with_its_exit_in_the_output() {
        let messages = parse(PROBE);
        let result = messages.iter().find(|m| m.role() == Role::Tool).unwrap();
        let Content::ToolResult(result) = &result.content()[0] else {
            panic!()
        };
        assert!(!result.error, "Copilot reports the call as a success");
        assert!(result.output_text().unwrap().contains("exit code 1"));
    }

    #[rstest]
    #[case::shrunk(
        serde_json::json!({"inputTokens": 29138, "outputTokens": 226, "cacheReadTokens": 0,
            "cacheWriteTokens": 29135, "reasoningTokens": 70}),
        Usage { input: Some(3), output: Some(226), cache_read: Some(0), cache_write: Some(29135), reasoning: Some(70) }
    )]
    #[case::missing(serde_json::json!({"outputTokens": 1}), Usage { output: Some(1), ..Default::default() })]
    fn usage_takes_the_cached_input_out(#[case] raw: serde_json::Value, #[case] want: Usage) {
        assert_eq!(usage_of(&raw), want);
    }

    #[rstest]
    #[case::generated("name: Project Overview\nuser_named: false\n", Some((TitleSource::Generated, "Project Overview")))]
    #[case::named("id: x\nname: atuin-sample-edit\nuser_named: true\n", Some((TitleSource::Named, "atuin-sample-edit")))]
    #[case::quoted("name: \"a: b\"\nuser_named: true\n", Some((TitleSource::Named, "a: b")))]
    #[case::escaped("name: \"Fix \\\"login\\\" in C:\\\\x\\tnow\"\nuser_named: true\n", Some((TitleSource::Named, "Fix \"login\" in C:\\x\tnow")))]
    #[case::single_quoted("name: 'a \"b\"'\n", Some((TitleSource::Generated, "a \"b\"")))]
    #[case::unnamed("id: x\ncwd: /tmp\n", None)]
    fn sidecar_names_the_session(#[case] yaml: &str, #[case] want: Option<(TitleSource, &str)>) {
        let title = Sidecar::parse(yaml).title();
        assert_eq!(
            title.map(|t| (t.source, t.text.unwrap())),
            want.map(|(source, text)| (source, text.to_owned()))
        );
    }

    /// The sidecar's name is a row of its own, read with the log's first line so the session is
    /// named at once, and again only once the name changes; a rename after the last message is
    /// a row the next line brings.
    #[rstest]
    #[tokio::test]
    async fn the_sidecars_name_is_a_row_of_its_own() {
        let root = root_with("s", TASK_AND_EDIT, Some(TASK_AND_EDIT_SIDECAR));
        let messages = read_all(root.path(), "s").await;
        let title = &messages[0];
        assert_eq!(title.role(), Role::Other(TITLE_ROW.to_owned()));
        assert!(title.content().is_empty());
        assert_eq!(title.parent_id(), None, "beside the tree");
        assert_eq!(title.title(), Some(TitleChange::new(TitleSource::Named, "atuin-sample-edit")));
        assert!(title.timestamp().is_some());
        // Nothing of where the session works: the log's lines say, and a title row read again
        // long after must not say otherwise.
        assert_eq!(title.cwd(), None);
        assert_eq!(title.git_branch(), None);
        assert_eq!(messages.iter().filter(|m| m.title().is_some()).count(), 1);
        assert_eq!(messages.len(), parse(TASK_AND_EDIT).len() + 1);

        // Read again, the same row, under the same id: no new row for an unchanged name.
        let id = title.id().expect("a title row has an id of its own");
        assert!(id.as_ref().starts_with("syn-title-"), "{id}");
        let again = read_all(root.path(), "s").await;
        assert_eq!(again[0].id(), Some(id.clone()));

        // Renamed, another row.
        let renamed = TASK_AND_EDIT_SIDECAR.replace("atuin-sample-edit", "renamed");
        std::fs::write(root.path().join("s").joined(SIDECAR_FILE), renamed).unwrap();
        let again = read_all(root.path(), "s").await;
        let titles: Vec<_> = again.iter().filter_map(Message::title).collect();
        assert_eq!(titles, [TitleChange::new(TitleSource::Named, "renamed")]);
        assert_ne!(again[0].id(), Some(id));
    }

    #[rstest]
    #[tokio::test]
    async fn a_missing_sidecar_names_nothing() {
        let root = root_with("s", PROBE, None);
        let messages = read_all(root.path(), "s").await;
        assert!(messages.iter().all(|m| m.title().is_none()));
        assert_eq!(messages.len(), parse(PROBE).len());
    }

    /// Resumed past a checkpoint, the reader still knows what the events before it said: the
    /// session's directory, which subagent interactions to skip, and the usage record a reply
    /// claims.
    #[rstest]
    #[tokio::test]
    async fn resuming_from_a_checkpoint_keeps_the_state_before_it() {
        let root = root_with("s", TASK_AND_EDIT, None);
        let path = root.path().join("s").joined(EVENTS_FILE);
        let session =
            || CopilotSession::open(SessionId::from("s".to_owned()), path.clone(), pool());
        let all: Vec<(Checkpoint, CopilotMessage)> =
            session().messages_from(None).try_collect().await.unwrap();
        // Just past the subagent's `task` result, before the edit.
        let (at, _) = &all[2];
        let rest: Vec<(Checkpoint, CopilotMessage)> =
            session().messages_from(Some(*at)).try_collect().await.unwrap();
        assert_eq!(rest.len(), all.len() - 3);
        // The subagent's session resumes past its own checkpoint the same way.
        let child = session().child(&Lane {
            call: TASK_CALL.to_owned(),
            name: None,
            interaction: None,
            last_row: None,
        });
        let child_all: Vec<(Checkpoint, CopilotMessage)> =
            child.clone().messages_from(None).try_collect().await.unwrap();
        assert_eq!(child_all.len(), 4);
        let (child_at, _) = &child_all[1];
        let child_rest: Vec<(Checkpoint, CopilotMessage)> =
            child.messages_from(Some(*child_at)).try_collect().await.unwrap();
        assert_eq!(
            child_rest.iter().map(|(_, m)| m.id()).collect::<Vec<_>>(),
            child_all[2..].iter().map(|(_, m)| m.id()).collect::<Vec<_>>()
        );
        assert!(
            rest.iter().all(|(_, m)| m.cwd().as_deref() == Some(Path::new("/tmp/copilot-play")))
        );
        let replies: Vec<_> = rest.iter().filter(|(_, m)| m.role() == Role::Assistant).collect();
        assert!(replies.iter().all(|(_, m)| m.usage().is_some()), "each reply claims its usage");
        assert_eq!(
            rest.iter().map(|(_, m)| m.id()).collect::<Vec<_>>(),
            all[3..].iter().map(|(_, m)| m.id()).collect::<Vec<_>>()
        );
    }

    #[rstest]
    #[tokio::test]
    async fn existing_lists_every_session_under_the_root() {
        let root = root_with("a", PROBE, None);
        let other = root.path().join("b");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join(EVENTS_FILE), PROBE).unwrap();
        std::fs::create_dir(root.path().join(".session-operation-locks")).unwrap();
        std::fs::write(root.path().join(".session-operation-locks").joined("a.lock"), "").unwrap();
        let sessions =
            CopilotSessions::builder().root(root.path().to_path_buf()).pool(pool()).build();
        let mut ids: Vec<String> = sessions
            .existing()
            .unwrap()
            .map_ok(|s| s.id().to_string())
            .try_collect()
            .await
            .unwrap();
        ids.sort();
        assert_eq!(ids, ["a", "b"]);
        assert!(matches!(
            CopilotSessions::builder()
                .root(PathBuf::from("/no/such"))
                .pool(pool())
                .build()
                .listener(),
            Err(RuntimeError::NotFound(_))
        ));
    }
}
