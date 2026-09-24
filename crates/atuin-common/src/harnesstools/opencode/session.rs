//! Live capture of opencode sessions from its SQLite event log.
//!
//! opencode appends every durable session event to the `event` table of one global database
//! (`opencode.db` in its data directory). [`OpencodeListener::watch`] tails that table with
//! [`SqliteObserver`] and demultiplexes rows by `aggregate_id` (the session id) into per-session
//! [`OpencodeSession`] streams.
//!
//! The tail carries no payloads: it reads a row's position and type only (`rowid, id,
//! aggregate_id, seq, type`) and wakes the session that row belongs to. Each session's
//! [`messages`](Session::messages) stream then pages its own aggregate's rows out of the `event`
//! table, resuming at the last row it delivered. The durable table is the queue.
//!
//! # Guarantees
//!
//! - **Nothing is lost.** A session resumes at the row it last delivered and reads forward, so a
//!   row is only ever missed by a consumer that stops reading the session. A failed read leaves
//!   the rows where they are and drops the connection behind it, and the session reads again --
//!   when its next row wakes it, or on a timer if it has none left to come. A tail that fails is
//!   no different: the failure is an item on the stream and the event log is taken up again, so
//!   an `event` table a migration drops for a moment costs a report rather than the capture. A
//!   session whose tail is gone for good reads its aggregate one last time before it ends.
//! - **Once within an incarnation of an aggregate, at least once across one.** opencode deletes a
//!   session's rows when the session is deleted and some of its migrations wipe the table, after
//!   which a session resumed under the same id restarts at `seq` 0; a restored backup rolls its
//!   sequence back. The session notices that the row it resumes at is gone and reads its
//!   aggregate again from the start, so rows a backup still holds are delivered twice. A
//!   duplicate is recoverable and a dropped row is not.
//! - **No session can stall another.** A session that is held but never drained reads nothing and
//!   costs one watermark; the tail and every other session carry on. What a session costs is a
//!   fixed amount -- a position, a wake and the roles of the last few messages it read -- and
//!   none of it grows with the rows it has read: nothing is queued, and the page a session is
//!   being drained through goes with the consumer that stops reading it.
//! - **One bad row never stops capture.** Column values of any storage class decode (a NULL makes
//!   the row an ignored one, with a warning), a session id no `String` holds still reads its own
//!   rows under a lossy name, and malformed JSON surfaces as a [`MessageError`] item on the
//!   session's stream while the reader moves on.
//! - **A part is delivered once it is finished.** opencode upserts a part under its own id as it
//!   is written: a streamed text or reasoning part empty and then whole, a tool call at every
//!   state it passes through. The drafts are skipped like a row that only records a role, so a
//!   consumer keeping the first message of an id keeps the part rather than its draft. A part
//!   opencode stops writing mid-way (killed during a tool call) is never delivered.
//! - **Roles are never guessed.** A session reads every row of its aggregate in `seq` order, so a
//!   message's `message.updated.1` row arrives ahead of the parts it describes. Only the newest
//!   few of those roles are kept, which is all a part ever asks for; one that predates the
//!   session's start or has been dropped since is looked up in opencode's `message` projection
//!   table, and `Role::Other("unknown")` is the last resort.
//! - **A dropped session handle loses nothing and keeps nothing.** Its reader stays where it was,
//!   at the last row a consumer took and holding none of those after it, and the session is
//!   offered again under the same id with the aggregate's next row, resuming at the row it had
//!   reached. A handle let go of mid-row is no different: the reader only moves on to the next
//!   row once the message of this one is in the consumer's hands.
//!
//! # What is captured
//!
//! - **Parts**, each under its own id, with what they need of their message: its role, and for an
//!   assistant's its model, cwd, the user message it answers and the model call it belongs to.
//!   Text opencode injected into a message (`synthetic`) or kept from the model (`ignored`) is
//!   the system's, and a compaction's summary is a [`Content::Summary`].
//! - **Usage**, once per model call, from the call's `step-finish` part (see
//!   [`OpencodeMessage::usage`](Message::usage)). A fork copies every message and part under
//!   fresh ids; the copy names the same turn as its original, so its usage is not counted twice
//!   (see `MessageInfo::turn_of`).
//! - **Failures**: an assistant message whose info reports `error` is a row of its own, under the
//!   message's id.
//! - **The session**: its title, when first seen and each time it changes, its directory and the
//!   session it was spawned from, from its `session.created.1` / `session.updated.1` rows.
//! - **The experimental event system's sessions** (`session.next.*`, written for the
//!   `/api/session` routes of `opencode serve`): prompts, finished text, reasoning and tool
//!   calls, each model call's usage and failure (see `Reader::next`). A row of a type or version
//!   this module does not model is delivered whole, as [`Content::Other`]: the schema
//!   (`packages/schema/src/session-event.ts`) is still moving.
//! - **What the event log does not hold**: a session's messages from before its log began --
//!   opencode's log is younger than its sessions and one of its migrations empties it -- are read
//!   by [`Session::read`] from opencode's `session`/`message`/`part` projection, and
//!   [`Sessions::existing`] lists the sessions only the projection holds (see `Backlog`). The
//!   live tail follows the log alone.
//!
//! A revert's removals (`message.removed.1`, `message.part.removed.1`) are not read: what was
//! captured stands. The reverted turns happened and cost their tokens, captured rows cannot be
//! retracted downstream, and opencode writes one removal per message and part when the next
//! prompt commits the revert, not when it is made.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures::future::BoxFuture;
use futures::{Stream, StreamExt};
use serde::de::Error as _;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteRow};
use sqlx::{Connection, Row, Sqlite, TypeInfo, ValueRef};
use time::OffsetDateTime;
use tokio::sync::{Mutex, MutexGuard, watch};
use typed_builder::TypedBuilder;

use crate::db::sqlite::observe::{
    ObserveConfig, ReplayBehavior, RowAppendedEvent, SqliteObserver, TableSchema, Tailable,
};
use crate::db::{query_as, query_scalar};
use crate::harnesstools::opencode::Opencode;
use crate::harnesstools::session::model::{
    Content, MessageId, Role, StopReason, TitleChange, TitleSource, ToolCallId, ToolResult,
    ToolUse, Usage,
};
use crate::harnesstools::session::{
    Checkpoint, Listener, Message, MessageError, Observable, RuntimeError, Session, SessionId,
    Sessions, WatchError,
};
use crate::os::fs::FdIdentity;
use crate::sync::BlockingPool;
use crate::utils::{env_nonempty, home_dir};

#[derive(Debug, Clone, TypedBuilder)]
pub struct OpencodeSessions {
    #[builder(default, setter(strip_option, into))]
    db: Option<PathBuf>,
    #[builder(default = ReplayBehavior::All)]
    replay: ReplayBehavior,
}

impl OpencodeSessions {
    fn data_dir() -> PathBuf {
        env_nonempty("XDG_DATA_HOME")
            .map_or_else(|| home_dir().join(".local").join("share"), PathBuf::from)
            .join("opencode")
    }

    /// When a database was last written: the newer of its file and `-wal` mtimes. `-shm` is
    /// left out because readers touch it too, our own observer included.
    fn recency(path: &Path) -> Option<SystemTime> {
        let mtime = |p: &Path| std::fs::metadata(p).ok()?.modified().ok();
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        mtime(path).max(mtime(Path::new(&wal)))
    }

    /// opencode never discovers: every release channel writes `opencode.db` and only
    /// pre-release channels write `opencode-<channel>.db`, so the former wins whenever it exists
    /// and the newest channel database is the fallback.
    fn discover(data: &Path) -> Option<PathBuf> {
        let main = data.join("opencode.db");
        if main.is_file() {
            return Some(main);
        }
        std::fs::read_dir(data)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path.file_name().is_some_and(|name| {
                        let name = name.to_string_lossy();
                        name.starts_with("opencode-") && name.ends_with(".db")
                    })
            })
            .max_by_key(|path| Self::recency(path))
    }

    /// Mirrors opencode's own resolution: `OPENCODE_DB` (absolute, or relative to the data dir),
    /// then the channel-independent `opencode.db`, then the newest channel database. The data dir
    /// (and with it the home dir) is only resolved by the branches that need it.
    fn resolve_db(&self) -> Result<PathBuf, RuntimeError> {
        if let Some(db) = &self.db {
            return Ok(db.clone());
        }
        if let Some(env) = env_nonempty("OPENCODE_DB") {
            let path = PathBuf::from(env);
            if path.as_os_str() == OsStr::new(":memory:") {
                return Err(RuntimeError::Io(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "OPENCODE_DB=:memory: keeps opencode's database in memory; there is no file \
                     to observe",
                )));
            }
            return Ok(if path.is_absolute() {
                path
            } else {
                Self::data_dir().join(path)
            });
        }
        let data = Self::data_dir();
        if env_nonempty("OPENCODE_DISABLE_CHANNEL_DB")
            .is_some_and(|v| matches!(v.to_str(), Some("1" | "true")))
        {
            return Ok(data.join("opencode.db"));
        }
        Ok(Self::discover(&data).unwrap_or_else(|| data.join("opencode.db")))
    }
}

impl Sessions for OpencodeSessions {
    type Listener = OpencodeListener;

    fn existing(
        &self,
    ) -> Result<
        impl Stream<Item = Result<OpencodeSession, RuntimeError>> + Send + 'static,
        RuntimeError,
    > {
        let db = self.resolve_db()?;
        if !db.is_file() {
            return Err(RuntimeError::NotFound(db));
        }
        Ok(async_stream::stream! {
            let reads = Arc::new(Reads::new(db.clone()));
            match reads.aggregates().await {
                Some(aggregates) => {
                    // the sessions of the log, then those only opencode's projection still holds
                    // (see `Backlog`): older than the log, or than its last wipe
                    let projected = reads.projected_sessions().await;
                    let logged: HashSet<Aggregate> = aggregates.iter().cloned().collect();
                    for aggregate in aggregates {
                        yield Ok(OpencodeSession::detached(aggregate, Arc::clone(&reads)));
                    }
                    match projected {
                        Some(projected) => {
                            for aggregate in projected {
                                if !logged.contains(&aggregate) {
                                    yield Ok(OpencodeSession::detached(
                                        aggregate,
                                        Arc::clone(&reads),
                                    ));
                                }
                            }
                        }
                        None => yield Err(RuntimeError::Io(io::Error::other(format!(
                            "{}: cannot read opencode's session projection",
                            db.display()
                        )))),
                    }
                }
                // one scan, one answer: a log that cannot be read is not an empty one
                None => yield Err(RuntimeError::Io(io::Error::other(format!(
                    "{}: cannot read opencode's event log",
                    db.display()
                )))),
            }
        })
    }

    fn listener(&self) -> Result<OpencodeListener, RuntimeError> {
        let db = self.resolve_db()?;
        if !db.is_file() {
            return Err(RuntimeError::NotFound(db));
        }
        Ok(OpencodeListener {
            db,
            replay: self.replay,
        })
    }
}

impl Observable for Opencode {
    type Sessions = OpencodeSessions;

    /// Opencode's sessions live in one SQLite database read through async connections, so nothing
    /// runs in `_pool`.
    fn sessions(&self, _pool: BlockingPool) -> OpencodeSessions {
        OpencodeSessions::builder().build()
    }
}

#[derive(Debug, Clone)]
pub struct OpencodeListener {
    db: PathBuf,
    replay: ReplayBehavior,
}

/// Rows a session reads from its aggregate at a time: the most it holds in memory while it is
/// being drained, and all it holds at all -- a session nobody reads holds none.
const PAGE: usize = 64;

/// How long a session waits before reading again after a read failed, when no new row of it wakes
/// it first.
const RETRY: Duration = Duration::from_secs(1);

impl Listener for OpencodeListener {
    type Session = OpencodeSession;

    /// Tails the event log and offers each session on its first forwarded row.
    ///
    /// Only session ids and positions pass through here; the rows themselves are read by the
    /// sessions, so a session that is never drained holds nothing up. Dropping a session handle
    /// loses nothing: the session is offered again, under the same id, with its aggregate's next
    /// row, and resumes at the row it had reached.
    ///
    /// A failure of the tail is reported and then carried on from. Only a database that cannot
    /// be observed at all ends this stream, and with it every session it has offered.
    fn watch(self) -> impl Stream<Item = Result<OpencodeSession, WatchError>> + Send + 'static {
        let db = self.db;
        let replay = self.replay;
        async_stream::stream! {
            let reads = Arc::new(Reads::new(db.clone()));
            let observe = |source| WatchError::Observe { db: db.clone(), source };
            if let Err(err) = reads.require_event_table().await {
                yield Err(err);
                return;
            }
            let mut events = match SqliteObserver::new(&db)
                .append::<EventRow>(ObserveConfig::builder().replay(replay).build())
                .await
            {
                Ok(events) => events,
                Err(err) => {
                    yield Err(observe(err));
                    return;
                }
            };
            let mut sessions: HashMap<Aggregate, Live> = HashMap::new();
            while let Some(next) = events.next().await {
                let row = match next {
                    Ok(RowAppendedEvent(row)) => row,
                    // The tail takes the table up again by itself and the rows it could not read
                    // are still in it, so reporting the failure is all there is to do here --
                    // ending would drop every session's wake and stop the capture for good.
                    Err(err) => {
                        yield Err(observe(err));
                        continue;
                    }
                };
                let Some(event) = row.event() else { continue };
                if EventRow::classify(event.kind).is_none() {
                    continue;
                }
                match sessions.entry(event.aggregate.clone()) {
                    Entry::Occupied(entry) => {
                        if let Some(session) = entry.into_mut().saw(event.aggregate, event.seq) {
                            yield Ok(session);
                        }
                    }
                    // a session exists from its first forwarded kind of row on
                    Entry::Vacant(entry) => {
                        let live = entry.insert(Live::new(&event, Arc::clone(&reads)));
                        yield Ok(live.offer(event.aggregate));
                    }
                }
            }
        }
    }
}

/// A session the tail has offered.
struct Live {
    /// The highest `seq` the tail has seen for this aggregate. The reader goes by its own
    /// watermark, so what this carries is a wake rather than a message -- a watch channel because
    /// it coalesces (a reader behind by a hundred rows is woken once) and because its receiver
    /// count is how the tail knows whether a consumer still holds the session: the handle holds
    /// the only receiver.
    wake: watch::Sender<i64>,
    /// Where the session has read up to. It lives here rather than in the handle so that a
    /// consumer which drops a session and takes it up again resumes where it stopped.
    reader: Arc<Mutex<Reader>>,
    /// Handed to the sessions this one is offered as, so that [`Session::read`] can page the
    /// aggregate from the start without disturbing the tail's own place in it.
    reads: Arc<Reads>,
}

impl Live {
    fn new(event: &Event<'_>, reads: Arc<Reads>) -> Self {
        let anchor = Anchor {
            seq: event.seq,
            id: event.id.to_owned(),
            delivered: false,
        };
        Self {
            wake: watch::Sender::new(event.seq),
            reader: Arc::new(Mutex::new(Reader::new(
                event.aggregate.clone(),
                Arc::clone(&reads),
                anchor,
            ))),
            reads,
        }
    }

    fn offer(&self, id: &Aggregate) -> OpencodeSession {
        OpencodeSession {
            id: id.to_string(),
            aggregate: id.clone(),
            reads: Arc::clone(&self.reads),
            wake: self.wake.subscribe(),
            reader: Arc::clone(&self.reader),
        }
    }

    /// A row of this aggregate: wakes the session's reader and, when no consumer holds the session
    /// any more, hands it back to be offered again.
    fn saw(&self, id: &Aggregate, seq: i64) -> Option<OpencodeSession> {
        self.wake.send_replace(seq);
        (self.wake.receiver_count() == 0).then(|| self.offer(id))
    }
}

/// Where a session resumes in its aggregate: the row it last read, and whether that row still has
/// to be delivered (the row the session was created on, which the tail saw but no page has
/// yielded).
///
/// `seq` counts up contiguously from 0 within one incarnation of an aggregate and names one row
/// for good, so reading each page from the anchor's own `seq` proves the incarnation in the same
/// statement that fetches the rows: a different id at that `seq`, or no row there at all, is a new
/// incarnation (opencode's reset migrations wipe the table and a session resumed afterwards
/// restarts at 0; a restored backup rolls the sequence back to where its copy ends) and the
/// session reads its aggregate again from the start. Ids are only compared for equality:
/// opencode's are not ordered across the 795-day wrap of their time prefix, nor within one
/// incarnation when the writer's clock steps back, so `id <= last` says nothing about which row
/// came first.
#[derive(Debug, Clone)]
struct Anchor {
    seq: i64,
    id: String,
    delivered: bool,
}

/// What a page read at an anchor's `seq` means for the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resume {
    /// Deliver the page from this row on: `1` when its head is the anchor and was delivered
    /// already, `0` when there is nothing to skip.
    From(usize),
    /// The anchor row is gone: the aggregate is a new incarnation, to be read again from `seq` 0.
    Restart,
}

/// Where a session resumes in a page it read at `anchor`'s `seq`, whose head row is `head`.
fn resume(anchor: Option<&Anchor>, head: Option<(i64, &str)>) -> Resume {
    let Some(anchor) = anchor else {
        return Resume::From(0);
    };
    match head {
        Some((seq, id)) if seq == anchor.seq && id == anchor.id => {
            Resume::From(usize::from(anchor.delivered))
        }
        _ => Resume::Restart,
    }
}

/// One session's place in the event log: it pages its aggregate's rows out of the `event` table
/// and turns them into messages.
struct Reader {
    aggregate: Aggregate,
    reads: Arc<Reads>,
    anchor: Option<Anchor>,
    /// What this session has lately learned of its messages from their `message.updated.1` rows.
    infos: Infos,
    /// The title this session last delivered, so that the `session.updated.1` rows opencode
    /// writes on every prompt deliver a title only when it changes.
    title: Option<TitleChange>,
    /// Whether the aggregate may have a row to read right now: set by a wake, kept while pages
    /// come back full.
    ready: bool,
    /// Whether the last page read failed, so that the rows it did not reach are read again even
    /// if no further row ever wakes this session.
    failed: bool,
}

impl Reader {
    /// A reader of the whole aggregate, from its first row.
    fn from_start(aggregate: Aggregate, reads: Arc<Reads>) -> Self {
        Self {
            aggregate,
            reads,
            anchor: None,
            infos: Infos::default(),
            title: None,
            ready: true,
            failed: false,
        }
    }

    fn new(aggregate: Aggregate, reads: Arc<Reads>, anchor: Anchor) -> Self {
        Self {
            aggregate,
            reads,
            anchor: Some(anchor),
            infos: Infos::default(),
            title: None,
            ready: true,
            failed: false,
        }
    }

    /// Read on past the row `from` names, if it is still the row `from` was taken after: a row
    /// that is gone, or another under the same `seq` after a wipe, leaves the aggregate to be
    /// read again from the start.
    async fn seek(&mut self, from: Checkpoint) {
        self.ready = true;
        let Ok(seq) = i64::try_from(from.at) else {
            return;
        };
        let head = self
            .reads
            .page(&self.aggregate, seq)
            .await
            .and_then(|rows| rows.into_iter().next())
            .filter(|row| row.seq == seq && from.names(row.identity().as_bytes()));
        if head.is_none() {
            tracing::debug!(
                session = %self.aggregate,
                seq,
                "the checkpoint no longer names its row; reading the aggregate from the start"
            );
        }
        self.anchor = head.map(|row| Anchor {
            seq,
            id: row.identity().to_owned(),
            delivered: true,
        });
    }

    /// The aggregate has a row this session has not read.
    const fn wake(&mut self) {
        self.ready = true;
    }

    /// Whether the read that ended the last drain failed rather than running out of rows.
    const fn failed(&self) -> bool {
        self.failed
    }

    /// A pass over the rows this session has not delivered yet.
    fn drain(&mut self) -> Drain<'_> {
        Drain {
            reader: self,
            page: Vec::new().into_iter(),
        }
    }

    /// Reads the next page of the aggregate, restarting the session at `seq` 0 when the row it
    /// resumes at is gone. `None` when there is nothing to deliver right now.
    async fn fill(&mut self) -> Option<std::vec::IntoIter<PageRow>> {
        while self.ready {
            let from = self.anchor.as_ref().map_or(0, |anchor| anchor.seq);
            let Some(mut rows) = self.reads.page(&self.aggregate, from).await else {
                self.ready = false;
                self.failed = true;
                return None;
            };
            self.failed = false;
            self.ready = rows.len() == PAGE;
            let head = rows.first().map(|row| (row.seq, row.identity()));
            match resume(self.anchor.as_ref(), head) {
                Resume::From(skip) => {
                    rows.drain(..skip.min(rows.len()));
                    return (!rows.is_empty()).then(|| rows.into_iter());
                }
                Resume::Restart => {
                    tracing::warn!(
                        session = %self.aggregate,
                        seq = from,
                        "the row this session resumes at is gone; reading its aggregate again \
                         from the start"
                    );
                    self.anchor = None;
                    self.ready = true;
                }
            }
        }
        None
    }

    /// The message a row carries, if any. A `message.updated.1` row records what its parts need
    /// to know of their message and is a message itself only when it reports a failed model call;
    /// a draft of a part opencode is still writing is superseded by a later row; a
    /// `session.updated.1` row that leaves the title as it was is one of the many opencode writes
    /// to touch a session; and an unmodelled kind of row is skipped.
    ///
    /// The row's `id` is not read here. It anchors the session, and a NULL one anchors as the
    /// empty string (see [`PageRow::identity`]), so a row whose payload is whole is decoded and
    /// delivered like any other rather than dropped for want of a key its message never needed.
    async fn message(&mut self, row: &PageRow) -> Option<Result<OpencodeMessage, MessageError>> {
        let (Some(kind), Some(data)) = (row.kind.as_deref(), row.data.as_deref()) else {
            tracing::warn!(
                session = %self.aggregate,
                seq = row.seq,
                "ignoring an event row with a NULL column"
            );
            return None;
        };
        let classified = EventRow::classify(kind)?;
        let data = match json(data) {
            Ok(data) => data,
            Err(err) => return Some(Err(MessageError::from(err))),
        };
        match classified {
            EventKind::Message => self.learn(&data).map(Ok),
            EventKind::Part => match OpencodeMessage::split_part(data) {
                Ok((part, _)) if OpencodeMessage::is_draft(&part) => None,
                Ok((part, time)) => {
                    let message_id = part.get("messageID").and_then(Value::as_str);
                    let info = self.info(message_id).await;
                    Some(Ok(OpencodeMessage::part(info, part, time)))
                }
                Err(err) => Some(Err(MessageError::from(err))),
            },
            EventKind::Session => {
                let message = OpencodeMessage::session(data)?;
                let title = message.title();
                if title.is_some() && title == self.title {
                    return None;
                }
                self.title = title;
                Some(Ok(message))
            }
            EventKind::Next => self.next(kind, row.identity(), data).map(Ok),
            EventKind::Unmapped => Some(Ok(OpencodeMessage::raw(kind, data))),
        }
    }

    /// The message a `session.next.*` row carries, the experimental event system's
    /// (`packages/schema/src/session-event.ts`, written by `packages/core/src/session`: the
    /// `/api/session` routes of `opencode serve`). It records a session as events of their own
    /// rather than as messages and parts: a prompt, then per model call (one assistant message
    /// id each) a `step.started`, the finished text, reasoning and tool calls, and a
    /// `step.ended` with its usage or a `step.failed`.
    ///
    /// Only the full values are read: the `*.started`, `tool.input.*` and `tool.progress` rows
    /// a finished value follows, and `prompt.admitted` (a prompt queued, recorded again as
    /// `prompted` once it joins the conversation), are no messages. A row whose type or version
    /// this does not know, or whose payload lacks what it needs, is delivered whole, as
    /// [`OpencodeMessage::raw`].
    fn next(&mut self, kind: &str, event: &str, data: Value) -> Option<OpencodeMessage> {
        let time = epoch_millis(&data["timestamp"]);
        let text = |key: &str| data[key].as_str().map(str::to_owned);
        let assistant = text("assistantMessageID");
        let modelled = match kind {
            "session.next.prompt.admitted.1"
            | "session.next.text.started.1"
            | "session.next.reasoning.started.1"
            | "session.next.tool.input.started.1"
            | "session.next.tool.input.ended.1"
            | "session.next.tool.progress.1"
            | "session.next.compaction.started.1" => return None,
            "session.next.step.started.1" if assistant.is_some() => {
                let id = assistant.unwrap_or_default();
                let info = MessageInfo {
                    role: Role::Assistant,
                    turn: Some(id.clone()),
                    model: data["model"]["id"].as_str().map(str::to_owned),
                    ..MessageInfo::unknown()
                };
                self.infos.insert(&id, info);
                return None;
            }
            "session.next.prompted.1" => text("messageID")
                .zip(data["prompt"]["text"].as_str())
                .map(|(id, prompt)| Next::said(Role::User, id, Content::Text(prompt.to_owned()))),
            "session.next.synthetic.1" | "session.next.context.updated.1" => text("messageID")
                .zip(data["text"].as_str())
                .map(|(id, said)| Next::said(Role::System, id, Content::Text(said.to_owned()))),
            "session.next.compaction.ended.1" => {
                text("messageID").zip(data["text"].as_str()).map(|(id, said)| {
                    Next::said(Role::Assistant, id, Content::Summary(said.to_owned()))
                })
            }
            "session.next.text.ended.1" => {
                assistant.as_ref().zip(data["textID"].as_str()).zip(data["text"].as_str()).map(
                    |((message, part), said)| {
                        let content = Content::Text(said.to_owned());
                        Next::said(Role::Assistant, format!("{message}/{part}"), content)
                    },
                )
            }
            "session.next.reasoning.ended.1" => {
                assistant.as_ref().zip(data["reasoningID"].as_str()).zip(data["text"].as_str()).map(
                    |((message, part), said)| {
                        let content = Content::Reasoning(said.to_owned());
                        Next::said(Role::Assistant, format!("{message}/{part}"), content)
                    },
                )
            }
            "session.next.tool.called.1" => {
                assistant.as_ref().zip(data["callID"].as_str()).map(|(message, call)| {
                    let content = Content::ToolUse(ToolUse {
                        id: ToolCallId::from(call.to_owned()),
                        name: data["tool"].as_str().unwrap_or_default().to_owned(),
                        input: data["input"].clone(),
                    });
                    Next::said(Role::Assistant, format!("{message}/{call}"), content)
                })
            }
            "session.next.tool.success.1" | "session.next.tool.failed.1" => {
                let error = kind == "session.next.tool.failed.1";
                assistant.as_ref().zip(data["callID"].as_str()).map(|(message, call)| {
                    let content = Content::ToolResult(ToolResult {
                        call: ToolCallId::from(call.to_owned()),
                        output: if error {
                            data["error"]["message"].clone()
                        } else {
                            data["content"].clone()
                        },
                        error,
                    });
                    Next::said(Role::Assistant, format!("{message}/{call}/result"), content)
                })
            }
            "session.next.step.ended.2" => assistant.as_ref().map(|message| Next {
                role: Role::Assistant,
                id: format!("{message}/step"),
                content: Vec::new(),
                usage: usage_of(&data["tokens"]),
                stop: data["finish"].as_str().map(OpencodeMessage::stop_reason_of),
            }),
            "session.next.step.failed.2" => assistant.as_ref().map(|message| Next {
                role: Role::Assistant,
                id: format!("{message}/step"),
                content: vec![Content::Error(
                    data["error"]["message"].as_str().unwrap_or("error").to_owned(),
                )],
                usage: None,
                stop: Some(StopReason::Error),
            }),
            _ => None,
        };
        let Some(next) = modelled else {
            tracing::debug!(session = %self.aggregate, event, kind, "delivering a session.next row whole");
            return Some(OpencodeMessage::raw(kind, data));
        };
        // an assistant row belongs to its model call, whose model the call's `step.started`
        // recorded; the call is the turn even when that row is no longer to hand
        let info = (next.role == Role::Assistant).then(|| {
            let id = assistant.unwrap_or_default();
            self.infos.get(&id).unwrap_or_else(|| MessageInfo {
                role: Role::Assistant,
                turn: (!id.is_empty()).then_some(id),
                ..MessageInfo::unknown()
            })
        });
        Some(OpencodeMessage {
            role: next.role.clone(),
            body: Body::Next(next),
            time,
            info,
        })
    }

    /// The messages one message of opencode's projection makes (see [`Backlog`]): its finished
    /// parts, as its `message.part.updated.1` rows would have delivered them, then its failure,
    /// if it reports one, as its last `message.updated.1` row would have.
    fn projected(
        &mut self,
        message_id: &str,
        data: &str,
        parts: Vec<PartProjection>,
    ) -> Vec<Result<OpencodeMessage, MessageError>> {
        let mut info = match json(data) {
            Ok(info) if info.is_object() => info,
            Ok(_) => {
                let err = serde_json::Error::custom("a projected message's data is no object");
                return vec![Err(MessageError::from(err))];
            }
            Err(err) => return vec![Err(MessageError::from(err))],
        };
        // the projection keeps a message's `id` and `sessionID` in columns of their own
        info["id"] = Value::from(message_id);
        let learned = MessageInfo::of(message_id, &info);
        if let Some(learned) = &learned {
            self.infos.insert(message_id, learned.clone());
        }
        let mut out = Vec::with_capacity(parts.len() + 1);
        for row in parts {
            let part = match row.data.as_deref().map(json) {
                Some(Ok(mut part)) if part.is_object() => {
                    part["id"] = row.id.map_or(Value::Null, Value::from);
                    part["messageID"] = Value::from(message_id);
                    part["sessionID"] = Value::from(self.aggregate.to_string());
                    part
                }
                Some(Err(err)) => {
                    out.push(Err(MessageError::from(err)));
                    continue;
                }
                _ => {
                    let err = serde_json::Error::custom("a projected part's data is no object");
                    out.push(Err(MessageError::from(err)));
                    continue;
                }
            };
            // the column is when the part was first written, the envelope `time` of its first row
            let data = serde_json::json!({"part": part, "time": row.time_created});
            match OpencodeMessage::split_part(data) {
                Ok((part, _)) if OpencodeMessage::is_draft(&part) => {}
                Ok((part, time)) => {
                    let info = learned.clone().unwrap_or_else(MessageInfo::unknown);
                    out.push(Ok(OpencodeMessage::part(info, part, time)));
                }
                Err(err) => out.push(Err(MessageError::from(err))),
            }
        }
        if let Some(failure) =
            learned.and_then(|learned| OpencodeMessage::failure(message_id, learned, &info))
        {
            out.push(Ok(failure));
        }
        out
    }

    /// Records what a `message.updated.1` row says of its message, handing back the failure it
    /// reports, if any.
    fn learn(&mut self, data: &Value) -> Option<OpencodeMessage> {
        let info = data.get("info")?;
        let id = info.get("id").and_then(Value::as_str)?;
        let learned = MessageInfo::of(id, info)?;
        self.infos.insert(id, learned.clone());
        OpencodeMessage::failure(id, learned, info)
    }

    /// What is known of the message `message_id`: from its `message.updated.1` row, or opencode's
    /// `message` projection when that row is no longer to hand.
    async fn info(&mut self, message_id: Option<&str>) -> MessageInfo {
        let Some(message_id) = message_id else {
            return MessageInfo::unknown();
        };
        if let Some(info) = self.infos.get(message_id) {
            return info;
        }
        match self.reads.info(message_id).await {
            Some(info) => {
                self.infos.insert(message_id, info.clone());
                info
            }
            None => MessageInfo::unknown(),
        }
    }
}

/// What a part needs to know of the message it belongs to. opencode fixes all of it when it
/// creates the message, so any `message.updated.1` row of a message says it for good.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MessageInfo {
    role: Role,
    /// The model call an assistant message is (see [`MessageInfo::turn_of`]).
    turn: Option<String>,
    /// The user message an assistant message answers (`parentID`).
    parent: Option<String>,
    /// `modelID`, of an assistant message.
    model: Option<String>,
    /// `path.cwd`, of an assistant message.
    cwd: Option<String>,
    /// A compaction summary (`summary: true`), whose text stands in for the conversation before
    /// it.
    summary: bool,
}

impl MessageInfo {
    fn unknown() -> Self {
        Self {
            role: Role::Other("unknown".to_owned()),
            turn: None,
            parent: None,
            model: None,
            cwd: None,
            summary: false,
        }
    }

    /// What `info`, the `Assistant` or `User` info of the message `id`, says. `None` when it
    /// names no role.
    fn of(id: &str, info: &Value) -> Option<Self> {
        let role = OpencodeMessage::role_of(info.get("role")?.as_str()?);
        if role != Role::Assistant {
            return Some(Self {
                role,
                ..Self::unknown()
            });
        }
        let text = |value: &Value| value.as_str().map(str::to_owned);
        Some(Self {
            role,
            turn: Some(Self::turn_of(id, info)),
            parent: text(&info["parentID"]),
            model: text(&info["modelID"]),
            cwd: text(&info["path"]["cwd"]),
            summary: info["summary"].as_bool().unwrap_or_default(),
        })
    }

    /// The model call an assistant message is: when it was created, and with which model.
    ///
    /// Not its id, because that is not the call's: forking a session (`Session.fork`) copies
    /// every message into the new session under a fresh id, with the rest of its info -- the
    /// creation time, the model, the tokens -- as it was, and every part likewise. Keyed on what
    /// the copy keeps, the copy is the same call as its original and its usage is not counted
    /// twice. opencode creates one assistant message per model call and stamps it to the
    /// millisecond, so two calls of one model share this key only when they start in the same
    /// millisecond -- parallel subagents can -- which the usage key of their steps then tells
    /// apart (see [`OpencodeMessage::turn_id`]). A message whose info lacks any of these falls
    /// back to its id.
    fn turn_of(id: &str, info: &Value) -> String {
        let created = epoch_millis(&info["time"]["created"]);
        match (created, info["providerID"].as_str(), info["modelID"].as_str()) {
            (Some(created), Some(provider), Some(model)) => format!("{created}:{provider}/{model}"),
            _ => id.to_owned(),
        }
    }
}

/// What a [`Reader`] knows of its messages, most recently used first.
///
/// Bounded, and small: opencode writes a message's `message.updated.1` row immediately before the
/// parts that carry it, so a part asks after one of the newest messages of all. Keeping the rest
/// would keep an entry per message for as long as the tail runs -- the history of every session
/// anyone ever read, pinned by a tail that never prunes the sessions it has seen. Dropping one
/// costs a lookup rather than an answer: opencode's `message` projection records the same thing
/// durably, and that is where [`Reader::info`] goes when this cannot answer.
#[derive(Debug, Default)]
struct Infos(VecDeque<(String, MessageInfo)>);

impl Infos {
    /// How many messages a session keeps what it knows of.
    const CAP: usize = 8;

    /// What is known of `message`, made the most recently used of those kept.
    fn get(&mut self, message: &str) -> Option<MessageInfo> {
        let at = self.0.iter().position(|(id, _)| id == message)?;
        let kept = self.0.remove(at).expect("position() named an entry");
        let info = kept.1.clone();
        self.0.push_front(kept);
        Some(info)
    }

    /// Records what is known of `message`, dropping the least recently used once [`Self::CAP`]
    /// are kept.
    fn insert(&mut self, message: &str, info: MessageInfo) {
        self.0.retain(|(id, _)| id != message);
        self.0.push_front((message.to_owned(), info));
        self.0.truncate(Self::CAP);
    }
}

/// One pass over a session's rows: from where its [`Reader`] left off to wherever the consumer
/// stops taking them.
///
/// The page being delivered lives here rather than in the reader, so that it goes when the stream
/// reading it does. A page left behind would stay behind for good: only the drain that reads the
/// next one replaces it, and a session no consumer holds never drains again -- up to [`PAGE`]
/// payloads pinned for the life of the tail, per session anyone ever let go of mid-page. Dropping
/// the page costs nothing but a re-read, the anchor naming the last row actually delivered.
struct Drain<'a> {
    reader: &'a mut Reader,
    /// What is left of the page being delivered.
    page: std::vec::IntoIter<PageRow>,
}

impl Drain<'_> {
    /// The session's next message, or `None` when its aggregate has nothing more to read right
    /// now -- a failed read included, since the rows stay in the table and are read again when
    /// the session is woken or retried.
    async fn next(&mut self) -> Option<(Checkpoint, Result<OpencodeMessage, MessageError>)> {
        loop {
            for row in self.page.by_ref() {
                // the anchor only moves on once the message of the row is in hand: building one
                // reads the database (a role lookup), and a stream dropped inside that read has
                // to leave the session on the row it was on.
                let message = self.reader.message(&row).await;
                self.reader.anchor = Some(Anchor {
                    seq: row.seq,
                    id: row.identity().to_owned(),
                    delivered: true,
                });
                if let Some(message) = message {
                    let at = u64::try_from(row.seq).unwrap_or(0);
                    return Some((Checkpoint::new(at, row.identity().as_bytes()), message));
                }
            }
            self.page = self.reader.fill().await?;
        }
    }
}

/// The identity of the file at `path`, or `None` when it cannot be told. Synchronous on purpose:
/// every read checks it, and one `stat` of a local file is cheaper than the round trip to a
/// blocking thread it would take to keep it off this one.
fn file_identity(path: &Path) -> Option<FdIdentity> {
    #[cfg(unix)]
    {
        std::fs::metadata(path).ok().as_ref().map(FdIdentity::from_metadata)
    }
    #[cfg(windows)]
    {
        use crate::os::fs::FdIdentityExt;

        std::fs::File::open(path).ok().and_then(|file| file.identity().ok())
    }
}

/// The open connection and the identity of the file it was opened on.
struct Open {
    conn: SqliteConnection,
    identity: Option<FdIdentity>,
}

/// One read-only connection to the observed database, shared by every session and by the role
/// lookups the event log itself cannot answer. One connection for all of them: opencode's event
/// log holds thousands of sessions and a connection each would exhaust our file descriptors.
///
/// It is opened on first use and reopened whenever the database file underneath it was replaced
/// (opencode resetting its database, a restored backup): a connection to the unlinked file would
/// go on serving the old rows for ever. A failed read drops it, and the next read reconnects.
struct Reads {
    db: PathBuf,
    conn: Mutex<Option<Open>>,
}

impl Reads {
    fn new(db: PathBuf) -> Self {
        Self {
            db,
            conn: Mutex::new(None),
        }
    }

    /// The shared connection, opened on first use. `None` (logged) when the database cannot be
    /// opened; the rows stay in the table, so the caller simply reads again later.
    async fn open(&self) -> Option<MutexGuard<'_, Option<Open>>> {
        let mut guard = self.conn.lock().await;
        let identity = file_identity(&self.db);
        if guard.as_ref().is_some_and(|open| open.identity != identity) {
            tracing::warn!(
                db = %self.db.display(),
                "opencode's database file was replaced; reconnecting"
            );
            *guard = None;
        }
        if guard.is_none() {
            let opts = SqliteConnectOptions::new()
                .filename(&self.db)
                .read_only(true)
                .busy_timeout(Duration::from_secs(5));
            match SqliteConnection::connect_with(&opts).await {
                Ok(conn) => *guard = Some(Open { conn, identity }),
                Err(err) => {
                    tracing::warn!(
                        db = %self.db.display(),
                        %err,
                        "cannot open opencode's database for reads"
                    );
                    return None;
                }
            }
        }
        Some(guard)
    }

    /// Runs one statement against the shared connection, dropping the connection on failure so
    /// that the next read reconnects.
    ///
    /// **Be warned**: the statement runs in a task of its own, and it must. Every session reads
    /// through this one connection, and a caller awaiting a statement holds it: a consumer that
    /// stops polling a session -- a timeout it keeps the stream past, a `select!` branch it does
    /// not come back to -- would suspend the read where it stands and lock every other session
    /// out of the database for as long as it holds the stream. A task of its own runs the
    /// statement to its end whatever the caller does with the handle.
    async fn read<T, F>(self: &Arc<Self>, what: &'static str, run: F) -> Option<T>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&'c mut SqliteConnection) -> BoxFuture<'c, Result<T, sqlx::Error>>
            + Send
            + 'static,
    {
        let reads = Arc::clone(self);
        let read = tokio::spawn(async move {
            let mut guard = reads.open().await?;
            let open = guard.as_mut().expect("open() hands back an open connection");
            match run(&mut open.conn).await {
                Ok(value) => Some(value),
                Err(err) => {
                    tracing::warn!(db = %reads.db.display(), %err, "opencode's {what} failed");
                    *guard = None;
                    None
                }
            }
        });
        match read.await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(%err, "opencode's {what} did not finish");
                None
            }
        }
    }

    /// Fails fast when the `event` table does not exist yet (an opencode older than its event
    /// log, or a database caught mid-migration); the consumer decides when to retry.
    async fn require_event_table(self: &Arc<Self>) -> Result<(), WatchError> {
        let probe = self
            .read("event table probe", |conn| {
                Box::pin(async move {
                    query_scalar::<Sqlite, i64>(
                        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'event'",
                    )
                    .fetch_optional(conn)
                    .await
                })
            })
            .await;
        // a probe that could not run is inconclusive: whatever is wrong with the database, the
        // observer's own connection will report it
        if probe.is_none_or(|found| found.is_some()) {
            return Ok(());
        }
        Err(WatchError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{}: no `event` table (opencode has not created its event log yet)",
                self.db.display()
            ),
        )))
    }

    /// One aggregate's rows from `from` on, the row at `from` included so that a session can check
    /// in the same statement that it is still reading the incarnation it left off in.
    ///
    /// opencode indexes `(aggregate_id, seq)` uniquely, so this is a seek rather than a scan: the
    /// cast that rebuilds a text id sits on the bound value, which leaves the index usable.
    async fn page(self: &Arc<Self>, aggregate: &Aggregate, from: i64) -> Option<Vec<PageRow>> {
        const TEXT: &str = "SELECT id, seq, type, data FROM event WHERE aggregate_id = CAST(?1 AS \
                            TEXT) AND seq >= ?2 ORDER BY seq ASC LIMIT ?3";
        const BLOB: &str = "SELECT id, seq, type, data FROM event WHERE aggregate_id = ?1 AND seq \
                            >= ?2 ORDER BY seq ASC LIMIT ?3";
        let (sql, bytes) = match aggregate {
            Aggregate::Text(bytes) => (TEXT, bytes.clone()),
            Aggregate::Blob(bytes) => (BLOB, bytes.clone()),
        };
        self.read("session page read", move |conn| {
            Box::pin(async move {
                query_as::<Sqlite, PageRow>(sql)
                    .bind(bytes)
                    .bind(from)
                    .bind(i64::try_from(PAGE).expect("PAGE fits an i64"))
                    .fetch_all(conn)
                    .await
            })
        })
        .await
    }

    /// Every aggregate the event log holds a row this module reads for, in the order the rows
    /// were written. The kinds mirror [`EventRow::classify`]: an aggregate whose every row is
    /// one this module ignores is no session to a consumer.
    async fn aggregates(self: &Arc<Self>) -> Option<Vec<Aggregate>> {
        const SQL: &str = "SELECT aggregate_id, min(rowid) AS first FROM event WHERE type LIKE \
                           'message.updated.%' OR type LIKE 'message.part.updated.%' OR type LIKE \
                           'session.created.%' OR type LIKE 'session.updated.%' OR type LIKE \
                           'session.next.%' GROUP BY aggregate_id ORDER BY first ASC";
        let rows: Vec<AggregateRow> = self
            .read("aggregate scan", |conn| {
                Box::pin(async move { query_as::<Sqlite, AggregateRow>(SQL).fetch_all(conn).await })
            })
            .await?;
        Some(rows.into_iter().filter_map(|row| row.aggregate).collect())
    }

    /// Every session opencode's `session` projection holds, oldest first: the sessions whose
    /// event log is gone are among them (see [`Backlog`]). No projection, no sessions.
    async fn projected_sessions(self: &Arc<Self>) -> Option<Vec<Aggregate>> {
        let rows: Vec<AggregateRow> = self
            .read("session scan", |conn| {
                Box::pin(async move {
                    if !projected(conn, &["session"]).await? {
                        return Ok(Vec::new());
                    }
                    query_as::<Sqlite, AggregateRow>(
                        "SELECT id AS aggregate_id FROM session ORDER BY time_created, id",
                    )
                    .fetch_all(conn)
                    .await
                })
            })
            .await?;
        Some(rows.into_iter().filter_map(|row| row.aggregate).collect())
    }

    /// What opencode's projection holds of a session that its event log does not (see
    /// [`Backlog`]).
    async fn backlog(self: &Arc<Self>, aggregate: &Aggregate) -> Option<Backlog> {
        // opencode's session ids are TEXT; a BLOB id names no session of the projection
        let Aggregate::Text(bytes) = aggregate else {
            return Some(Backlog::default());
        };
        let bytes = bytes.clone();
        self.read("projection backlog", move |conn| {
            Box::pin(async move {
                if !projected(conn, &["session", "message", "part"]).await? {
                    return Ok(Backlog::default());
                }
                let (created, updated) = query_as::<Sqlite, (bool, bool)>(
                    "SELECT EXISTS (SELECT 1 FROM event WHERE aggregate_id = CAST(?1 AS TEXT) AND \
                     type LIKE 'session.created.%'), EXISTS (SELECT 1 FROM event WHERE \
                     aggregate_id = CAST(?1 AS TEXT) AND type LIKE 'session.updated.%')",
                )
                .bind(bytes.clone())
                .fetch_one(&mut *conn)
                .await?;
                if created {
                    return Ok(Backlog::default());
                }
                // the log's own `session.updated` rows say what the session is, in the order it
                // changed: the projection's row is only its latest state, and delivered ahead of
                // them it would be the first of its title, and a later change back to it a
                // duplicate
                let session = if updated {
                    None
                } else {
                    query_as::<Sqlite, SessionRow>(
                        "SELECT id, title, parent_id, directory, time_created, time_updated FROM \
                         session WHERE id = CAST(?1 AS TEXT)",
                    )
                    .bind(bytes.clone())
                    .fetch_optional(&mut *conn)
                    .await?
                };
                // `json_valid` first: one malformed payload would fail `json_extract`, and with it
                // the whole statement
                let logged: HashSet<String> = query_as::<Sqlite, IdRow>(
                    "SELECT DISTINCT CASE WHEN json_valid(data) THEN json_extract(data, \
                     '$.info.id') END AS id FROM event WHERE aggregate_id = CAST(?1 AS TEXT) AND \
                     type LIKE 'message.updated.%'",
                )
                .bind(bytes.clone())
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .filter_map(|row| row.id)
                .collect();
                let messages = query_as::<Sqlite, IdRow>(
                    "SELECT id FROM message WHERE session_id = CAST(?1 AS TEXT) ORDER BY \
                     time_created, id",
                )
                .bind(bytes)
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .filter_map(|row| row.id)
                .filter(|id| !logged.contains(id))
                .collect();
                Ok(Backlog {
                    session: session.map(SessionRow::info),
                    messages,
                })
            })
        })
        .await
    }

    /// One message of opencode's projection, with its parts in the order opencode reads them
    /// (`MessageV2.page`). `Ok(None)` for a message deleted since the backlog listed it.
    async fn projected_message(
        self: &Arc<Self>,
        message_id: &str,
    ) -> Option<Option<(String, Vec<PartProjection>)>> {
        let id = message_id.to_owned();
        self.read("projected message read", move |conn| {
            Box::pin(async move {
                let Some(row) =
                    query_as::<Sqlite, DataRow>("SELECT data FROM message WHERE id = ?1")
                        .bind(id.clone())
                        .fetch_optional(&mut *conn)
                        .await?
                else {
                    return Ok(None);
                };
                let parts = query_as::<Sqlite, PartProjection>(
                    "SELECT id, time_created, data FROM part WHERE message_id = ?1 ORDER BY id",
                )
                .bind(id)
                .fetch_all(&mut *conn)
                .await?;
                Ok(Some((row.data.unwrap_or_default(), parts)))
            })
        })
        .await
    }

    /// What opencode's `message` projection records of `message_id` (its `data` is the message
    /// info minus `id` and `sessionID`), for parts whose `message.updated.1` row predates the
    /// session's start.
    async fn info(self: &Arc<Self>, message_id: &str) -> Option<MessageInfo> {
        let id = message_id.to_owned();
        let data = self
            .read("message info lookup", move |conn| {
                Box::pin(async move {
                    query_scalar::<Sqlite, Option<Vec<u8>>>(
                        "SELECT data FROM message WHERE id = ?1",
                    )
                    .bind(id)
                    .fetch_optional(conn)
                    .await
                })
            })
            .await?
            .flatten()?;
        let info = json(&String::from_utf8_lossy(&data)).ok()?;
        MessageInfo::of(message_id, &info)
    }
}

pub struct OpencodeSession {
    id: String,
    aggregate: Aggregate,
    reads: Arc<Reads>,
    /// The tail's wake for this session; dropping it tells the tail that no consumer holds the
    /// session any more.
    wake: watch::Receiver<i64>,
    reader: Arc<Mutex<Reader>>,
}

impl OpencodeSession {
    /// A session of the event log as it stands, with no tail behind it: its wake is closed, so
    /// [`messages`](Session::messages) reads the aggregate out and ends.
    fn detached(aggregate: Aggregate, reads: Arc<Reads>) -> Self {
        let (closed, wake) = watch::channel(0);
        drop(closed);
        Self {
            id: aggregate.to_string(),
            reader: Arc::new(Mutex::new(Reader::from_start(aggregate.clone(), Arc::clone(&reads)))),
            aggregate,
            reads,
            wake,
        }
    }
}

impl std::fmt::Debug for OpencodeSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpencodeSession").field("id", &self.id).finish_non_exhaustive()
    }
}

impl Session for OpencodeSession {
    type Message = OpencodeMessage;

    fn id(&self) -> SessionId {
        SessionId::from(self.id.clone())
    }

    /// The session as it stands, from the start, ending when it runs out of rows: what opencode's
    /// projection holds of it from before its event log began (see `Backlog`), then the
    /// aggregate's rows. Independent of [`messages`](Self::messages): it reads a place in the log
    /// of its own, so a session being tailed is undisturbed by it.
    fn read(&self) -> impl Stream<Item = Result<OpencodeMessage, MessageError>> + Send + 'static {
        let reads = Arc::clone(&self.reads);
        let mut reader = Reader::from_start(self.aggregate.clone(), Arc::clone(&reads));
        let aggregate = self.aggregate.clone();
        async_stream::stream! {
            let mut incomplete = false;
            match reads.backlog(&aggregate).await {
                Some(backlog) => {
                    if let Some(message) = backlog.session.and_then(|info| {
                        OpencodeMessage::session(serde_json::json!({"info": info}))
                    }) {
                        yield Ok(message);
                    }
                    for id in backlog.messages {
                        match reads.projected_message(&id).await {
                            Some(Some((data, parts))) => {
                                for message in reader.projected(&id, &data, parts) {
                                    yield message;
                                }
                            }
                            Some(None) => {}
                            None => incomplete = true,
                        }
                    }
                }
                None => incomplete = true,
            }
            let mut drain = reader.drain();
            while let Some((_, message)) = drain.next().await {
                yield message;
            }
            // A drain also ends on a failed read; a one-shot read says so rather than passing
            // for the whole session.
            if incomplete || reader.failed() {
                yield Err(MessageError::Incomplete);
            }
        }
    }

    /// A checkpoint is a row's `seq`, which opencode assigns contiguously from 0 within one
    /// incarnation of an aggregate and never reuses within it, and a digest of the row's id.
    fn messages_from(
        self,
        from: Option<Checkpoint>,
    ) -> impl Stream<Item = Result<(Checkpoint, OpencodeMessage), MessageError>> + Send + 'static
    {
        let Self {
            mut wake, reader, ..
        } = self;
        async_stream::stream! {
            // whether the drain at the top of the loop is this session's last: the tail has gone
            // and nothing can wake the session again, so it reads out the rows already in the
            // table -- those a failed read did not reach among them -- rather than abandon them
            let mut last = false;
            // a checkpoint resumes the session past the row it names
            if let Some(from) = from {
                reader.lock().await.seek(from).await;
            }
            loop {
                let failed = {
                    // the reader is locked for as long as this stream drains it; the only other
                    // taker is the handle this session is offered as again, which cannot exist
                    // before this one is dropped
                    let mut reader = reader.lock().await;
                    // a fresh handle reads at once: the row it was offered on is already in the
                    // table, and its wake was subscribed to after the tail sent it
                    reader.wake();
                    let mut drain = reader.drain();
                    while let Some((at, message)) = drain.next().await {
                        yield message.map(|message| (at, message));
                    }
                    reader.failed()
                };
                if last {
                    break;
                }
                // a read that failed left rows behind, so it is tried again even if the session
                // never writes another row; the tail having gone (an error here) is the only
                // thing that can say no further row will ever be recorded for it
                let woken = if failed {
                    tokio::select! {
                        changed = wake.changed() => changed,
                        () = tokio::time::sleep(RETRY) => Ok(()),
                    }
                } else {
                    wake.changed().await
                };
                last = woken.is_err();
            }
        }
    }
}

/// Whether every one of `tables` is there to read. opencode has kept its `session`, `message` and
/// `part` projections since it moved to SQLite, before it kept an event log, so a database with
/// an `event` table has them; one without is a database this module was not given by opencode,
/// and has no projection to read rather than a broken one.
async fn projected(conn: &mut SqliteConnection, tables: &[&str]) -> Result<bool, sqlx::Error> {
    let listed: Vec<String> = query_as::<Sqlite, IdRow>(
        "SELECT name AS id FROM sqlite_master WHERE type = 'table' AND name IN ('session', \
         'message', 'part')",
    )
    .fetch_all(conn)
    .await?
    .into_iter()
    .filter_map(|row| row.id)
    .collect();
    Ok(tables.iter().all(|table| listed.iter().any(|name| name == table)))
}

/// What opencode's projection holds of a session that its event log does not.
///
/// opencode records a session twice: as the durable rows of its event log, and in the `session`,
/// `message` and `part` tables its projectors keep from those rows, which are what opencode
/// itself reads. The projection holds the whole of every session; the event log only what was
/// written since it began. opencode kept no event log before 1.3 and wrote one unconditionally
/// only from 1.16; its 1.17.10 migration (`20260622170816_reset_v2_session_state`) empties it;
/// and sessions migrated from its JSON storage never had one. A session with no `session.created`
/// row therefore predates its log, and what the projection holds of it -- messages the log has no
/// `message.updated` row for -- is read from the projection, ahead of its log.
///
/// A session whose log has its `session.created` row is read from the log alone: the projection
/// holds nothing it does not.
#[derive(Debug, Default)]
struct Backlog {
    /// The session's info as the projection holds it -- its title, directory and parent -- when
    /// the log has no `session.updated` row to say it.
    session: Option<Value>,
    /// The messages the projection holds and the log does not, in opencode's order.
    messages: Vec<String>,
}

/// A row of opencode's `session` projection: what a [`Body::Session`] reads of it.
struct SessionRow {
    id: Option<String>,
    title: Option<String>,
    parent_id: Option<String>,
    directory: Option<String>,
    created: Option<i64>,
    updated: Option<i64>,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for SessionRow {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: text(row, "id")?,
            title: text(row, "title")?,
            parent_id: text(row, "parent_id")?,
            directory: text(row, "directory")?,
            created: row.try_get_unchecked("time_created")?,
            updated: row.try_get_unchecked("time_updated")?,
        })
    }
}

impl SessionRow {
    /// The row as the session info a `session.updated.1` row would carry.
    fn info(self) -> Value {
        let mut info = serde_json::json!({
            "id": self.id,
            "title": self.title,
            "directory": self.directory,
            "time": {"created": self.created, "updated": self.updated},
        });
        if let Some(parent) = self.parent_id {
            info["parentID"] = Value::from(parent);
        }
        info
    }
}

/// A single text column, `id`, of any storage class.
struct IdRow {
    id: Option<String>,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for IdRow {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: text(row, "id")?,
        })
    }
}

/// A single text column, `data`, of any storage class.
struct DataRow {
    data: Option<String>,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for DataRow {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            data: text(row, "data")?,
        })
    }
}

/// A row of opencode's `part` projection: the part minus its `id`, `messageID` and `sessionID`,
/// and when it was first written.
struct PartProjection {
    id: Option<String>,
    time_created: Option<i64>,
    data: Option<String>,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for PartProjection {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: text(row, "id")?,
            time_created: row.try_get_unchecked("time_created")?,
            data: text(row, "data")?,
        })
    }
}

/// One row of the aggregate scan: the ids the event log holds rows for.
struct AggregateRow {
    aggregate: Option<Aggregate>,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for AggregateRow {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            aggregate: aggregate(row, "aggregate_id")?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EventKind {
    /// `message.updated.1`: a message's info.
    Message,
    /// `message.part.updated.1`: one part of a message, as it was upserted.
    Part,
    /// `session.created.1` / `session.updated.1`: the session's info.
    Session,
    /// `session.next.*`: a row of the experimental event system (see [`Reader::next`]).
    Next,
    /// A durable event this module does not model.
    Unmapped,
}

/// A column decoded without type checks: SQLite stores whatever a writer put in a column, and a
/// checked `String` decode of a single NULL, BLOB or non-UTF-8 value would fail the whole page and
/// wedge the reader on that row forever. Non-text values decode lossily.
fn text(row: &SqliteRow, column: &str) -> Result<Option<String>, sqlx::Error> {
    let bytes: Option<Vec<u8>> = row.try_get_unchecked(column)?;
    Ok(bytes.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
}

/// A session's `aggregate_id`, held as the column holds it so that a page read can bind a value
/// SQLite matches it against again.
///
/// A lossy decode cannot be bound back: `=` compares storage class as well as bytes, and neither a
/// replacement character nor text bound for a BLOB names the rows it came from. A session keyed on
/// one is offered and then reads none of its own rows, forever. Two ids that differ only outside
/// UTF-8 are two sessions here and collapse only where they are shown, a [`SessionId`] being a
/// `String`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Aggregate {
    /// A TEXT value, as its bytes: SQLite never checks that text is UTF-8, so a `String` cannot
    /// hold every id a page read has to ask for.
    Text(Vec<u8>),
    /// A value of any other storage class. `aggregate_id` is declared `TEXT`, whose affinity
    /// converts a number on the way in, so in practice this is a BLOB.
    Blob(Vec<u8>),
}

impl std::fmt::Display for Aggregate {
    /// The id a consumer sees, lossily: a [`SessionId`] is a `String`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::Text(bytes) | Self::Blob(bytes)) = self;
        f.write_str(&String::from_utf8_lossy(bytes))
    }
}

/// A row's `aggregate_id`, with the storage class read before the bytes: decoding a value converts
/// it, and the class is what tells text a `String` cannot hold from a BLOB that only looks like it.
fn aggregate(row: &SqliteRow, column: &str) -> Result<Option<Aggregate>, sqlx::Error> {
    let class = match row.try_get_raw(column)?.type_info().name() {
        "TEXT" => Aggregate::Text,
        _ => Aggregate::Blob,
    };
    let bytes: Option<Vec<u8>> = row.try_get_unchecked(column)?;
    Ok(bytes.map(class))
}

/// What the tail reads of an `event` row: enough to place it and to tell whether a session wants
/// it. The payload stays in the table until the session it belongs to reads it.
#[derive(Clone)]
struct EventRow {
    rowid: i64,
    seq: i64,
    id: Option<String>,
    aggregate_id: Option<Aggregate>,
    kind: Option<String>,
}

/// An [`EventRow`] the tail can place.
struct Event<'a> {
    /// The row's `id`, or the empty string when it is NULL: a session's anchor only ever compares
    /// ids for equality, so a NULL id still anchors -- only another NULL-id row in its place goes
    /// unnoticed.
    id: &'a str,
    aggregate: &'a Aggregate,
    seq: i64,
    kind: &'a str,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for EventRow {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            rowid: row.try_get_unchecked("rowid")?,
            seq: row.try_get_unchecked("seq")?,
            id: text(row, "id")?,
            aggregate_id: aggregate(row, "aggregate_id")?,
            kind: text(row, "type")?,
        })
    }
}

impl EventRow {
    /// What a row of `kind` is to a session, or `None` for one no session reads: a revert's
    /// removals (`message.removed.1`, `message.part.removed.1`; see the module docs) and a
    /// deletion among them.
    fn classify(kind: &str) -> Option<EventKind> {
        match kind {
            "message.updated.1" => Some(EventKind::Message),
            "message.part.updated.1" => Some(EventKind::Part),
            "session.created.1" | "session.updated.1" => Some(EventKind::Session),
            k if k.starts_with("session.next.") => Some(EventKind::Next),
            k if k.starts_with("message.updated.")
                || k.starts_with("message.part.updated.")
                || k.starts_with("session.created.")
                || k.starts_with("session.updated.") =>
            {
                Some(EventKind::Unmapped)
            }
            _ => None,
        }
    }

    /// The row's place in the log, or `None` (logged) when it names no session or no type: SQLite
    /// allows a NULL there and a hand-edited database is still a database. Such a row is invisible
    /// to the sessions too, so this is the only place that can report it.
    fn event(&self) -> Option<Event<'_>> {
        let (Some(aggregate), Some(kind)) = (self.aggregate_id.as_ref(), self.kind.as_deref())
        else {
            tracing::warn!(rowid = self.rowid, "ignoring an event row with a NULL column");
            return None;
        };
        Some(Event {
            id: self.id.as_deref().unwrap_or_default(),
            aggregate,
            seq: self.seq,
            kind,
        })
    }
}

impl TableSchema for EventRow {
    const TABLE: &'static str = "event";
    const COLUMNS: &'static [&'static str] = &["rowid", "id", "aggregate_id", "seq", "type"];
}

impl Tailable for EventRow {
    type Cursor = i64;

    fn cursor(&self) -> i64 {
        self.rowid
    }

    /// The `id` is a never-reused primary key. A NULL id still anchors, as the empty string: a
    /// deleted or replaced row is then still noticed, only another NULL-id row in its place is not.
    fn identity(&self) -> Option<impl Eq> {
        Some(self.id.as_deref().unwrap_or_default())
    }
}

/// One row of a session's own page of the `event` table, decoded like [`EventRow`] and carrying
/// the payload.
#[derive(Clone, Debug)]
struct PageRow {
    seq: i64,
    id: Option<String>,
    kind: Option<String>,
    data: Option<String>,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for PageRow {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            seq: row.try_get_unchecked("seq")?,
            id: text(row, "id")?,
            kind: text(row, "type")?,
            data: text(row, "data")?,
        })
    }
}

impl PageRow {
    /// What the row anchors at: its `id`, or the empty string when that is NULL (see [`Event`]).
    fn identity(&self) -> &str {
        self.id.as_deref().unwrap_or_default()
    }
}

/// `message.part.updated.1` → `message.part.updated`: opencode versions durable event types with
/// a numeric suffix.
fn unversioned(kind: &str) -> &str {
    match kind.rsplit_once('.') {
        Some((base, version))
            if !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit()) =>
        {
            base
        }
        _ => kind,
    }
}

/// A JSON payload opencode wrote, read as `JSON.parse` would (see [`crate::json::js`]): opencode
/// cuts text by UTF-16 code units -- a shell tool keeps the last 30,000 of its output, the read
/// tool the first 2,000 of a long line -- so an emoji at the cut leaves a lone surrogate behind.
fn json(text: &str) -> Result<Value, serde_json::Error> {
    crate::json::js::from_slice(text.as_bytes())
}

/// opencode's clocks are `Date.now()` millisecond epochs: integers in practice, declared as finite
/// numbers.
#[allow(clippy::cast_possible_truncation, reason = "millisecond epochs fit i64 by a wide margin")]
fn epoch_millis(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_f64().map(|ms| ms.round() as i64))
}

/// A token count: a finite number in opencode's schema, an integer in practice. A negative one
/// (a provider's accounting gone wrong) counts as none.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to be non-negative; token counts fit u64 by a wide margin"
)]
fn tokens(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_f64().map(|n| n.max(0.0).round() as u64))
}

/// One message of an opencode session.
///
/// Most are parts (`message.part.updated.1`): opencode stores a message as its info plus parts,
/// and each part is delivered once finished, under its own id. What a part needs of its message
/// -- role, model, cwd, the user message it answers -- comes from the message's info. A step's
/// usage rides its `step-finish` part (see [`Message::usage`]).
///
/// The rest are rows of their own: a failed assistant message, under the message's id (a model
/// call that fails before a single part is written leaves nothing else behind), and the
/// session's title and parent, from its `session.created.1` / `session.updated.1` rows.
#[derive(Debug, Clone)]
pub struct OpencodeMessage {
    role: Role,
    body: Body,
    time: Option<i64>,
    /// The message a part or failure belongs to.
    info: Option<MessageInfo>,
}

#[derive(Debug, Clone)]
enum Body {
    /// A part of a message.
    Part(Value),
    /// A message whose info reports `error`: its id and that info.
    Failure {
        id: String,
        info: Value,
    },
    /// A session's info.
    Session(Value),
    /// A `session.next.*` row (see [`Reader::next`]).
    Next(Next),
    /// A durable event this module does not model, whole.
    Raw(Value),
}

/// What a `session.next.*` row says, under the id it is delivered as: the message's own id for a
/// prompt, a summary or injected text; for a model call's rows, its assistant message id and the
/// part of the call the row is (`<message>/<textID>`, `<message>/<callID>`,
/// `<message>/<callID>/result`, `<message>/step`), since the call's rows share its message id.
#[derive(Debug, Clone)]
struct Next {
    role: Role,
    id: String,
    content: Vec<Content>,
    usage: Option<Usage>,
    stop: Option<StopReason>,
}

impl Next {
    fn said(role: Role, id: String, content: Content) -> Self {
        Self {
            role,
            id,
            content: vec![content],
            usage: None,
            stop: None,
        }
    }
}

/// A model call's usage from its token counts (a `step-finish` part's or a
/// `session.next.step.ended` row's `tokens`), `None` when there are none.
///
/// opencode's `input` already leaves out the cached tokens, and its `output` the reasoning ones
/// (`Session.getUsage`; the experimental runner's `nonCachedInputTokens` and
/// `visibleOutputTokens`), which are added back here: reasoning is billed as output.
fn usage_of(counts: &Value) -> Option<Usage> {
    if !counts.is_object() {
        return None;
    }
    let reasoning = tokens(&counts["reasoning"]);
    let output = match (tokens(&counts["output"]), reasoning) {
        (None, None) => None,
        (output, reasoning) => {
            Some(output.unwrap_or_default().saturating_add(reasoning.unwrap_or_default()))
        }
    };
    Some(Usage {
        input: tokens(&counts["input"]),
        output,
        cache_read: tokens(&counts["cache"]["read"]),
        cache_write: tokens(&counts["cache"]["write"]),
        reasoning,
    })
}

impl OpencodeMessage {
    /// A finished part of the message `info` describes.
    ///
    /// A `synthetic` text part is one opencode wrote into the message itself -- a file an `@`
    /// mention read, an MCP resource, a plan-mode reminder, a subagent's result, the prompt that
    /// resumes a session after compaction -- and an `ignored` one is context a client showed the
    /// user and kept from the model. Neither was typed by anyone, so both are the harness's
    /// ([`Role::System`]), whichever message they sit in. So is a `compaction` part: the marker
    /// opencode writes as the whole of a user message it creates to ask for a compaction
    /// (`SessionCompaction.create`, on `/compact` or when the context overflows).
    fn part(info: MessageInfo, part: Value, time: Option<i64>) -> Self {
        let injected = part["type"] == "compaction"
            || (part["type"] == "text"
                && (part["synthetic"].as_bool() == Some(true)
                    || part["ignored"].as_bool() == Some(true)));
        Self {
            role: if injected {
                Role::System
            } else {
                info.role.clone()
            },
            body: Body::Part(part),
            time,
            info: Some(info),
        }
    }

    /// The failure `info`, the message `id`'s info, reports, if it reports one: `info.error`, set
    /// when a model call is aborted or fails (`processor.ts` `halt`) and when a finished one is
    /// refused (`prompt.ts`, a `content-filter` finish).
    fn failure(id: &str, learned: MessageInfo, info: &Value) -> Option<Self> {
        if learned.role != Role::Assistant || !info["error"].is_object() {
            return None;
        }
        let time = &info["time"];
        Some(Self {
            role: Role::Assistant,
            time: epoch_millis(&time["completed"]).or_else(|| epoch_millis(&time["created"])),
            body: Body::Failure {
                id: id.to_owned(),
                info: info.clone(),
            },
            info: Some(learned),
        })
    }

    /// A session's `session.created.1` or `session.updated.1` payload, as a row carrying its
    /// title and the session it was spawned from. `None` for a payload with no info.
    fn session(mut data: Value) -> Option<Self> {
        let info = data.get_mut("info").filter(|info| info.is_object()).map(Value::take)?;
        let time = &info["time"];
        Some(Self {
            role: Role::System,
            time: epoch_millis(&time["updated"]).or_else(|| epoch_millis(&time["created"])),
            body: Body::Session(info),
            info: None,
        })
    }

    /// A durable event this module does not model (`session.next.*`, or a newer version of a
    /// modelled type), surfaced whole as [`Content::Other`] under `Role::Other(<unversioned
    /// type>)`: these payloads carry no role, and `session.next.prompted` for one is the user's.
    fn raw(kind: &str, data: Value) -> Self {
        let time = data
            .get("time")
            .or_else(|| data.get("timestamp"))
            .or_else(|| data.get("info")?.get("time")?.get("created"))
            .and_then(epoch_millis);
        Self {
            role: Role::Other(unversioned(kind).to_owned()),
            body: Body::Raw(data),
            time,
            info: None,
        }
    }

    /// The part this message is, if it is one.
    const fn as_part(&self) -> Option<&Value> {
        match &self.body {
            Body::Part(part) => Some(part),
            _ => None,
        }
    }

    /// The part this message is, if it is a `step-finish`: the end of one model call.
    fn step_finish(&self) -> Option<&Value> {
        self.as_part().filter(|part| part["type"] == "step-finish")
    }

    /// The assistant message this one belongs to, if any.
    fn assistant(&self) -> Option<&MessageInfo> {
        self.info.as_ref().filter(|info| info.role == Role::Assistant)
    }

    /// What an opencode error (`{name, data: {message, ..}}`) says happened.
    fn error_text(error: &Value) -> String {
        match error["data"]["message"].as_str() {
            Some(message) if !message.is_empty() => message.to_owned(),
            _ => error["name"].as_str().unwrap_or("error").to_owned(),
        }
    }

    /// The stop reason of a step's `finish` (the AI SDK's finish reasons).
    fn stop_reason_of(finish: &str) -> StopReason {
        match finish {
            "stop" => StopReason::EndTurn,
            "length" => StopReason::MaxTokens,
            "tool-calls" => StopReason::ToolUse,
            "content-filter" => StopReason::Refusal,
            "error" => StopReason::Error,
            other => StopReason::Other(other.to_owned()),
        }
    }

    /// What a part says. A `step-finish` says nothing: it carries its step's usage and why the
    /// step ended, which are no conversation.
    fn part_content(&self, part: &Value) -> Vec<Content> {
        match part["type"].as_str() {
            Some("text") => {
                let text = part["text"].as_str().unwrap_or_default().to_owned();
                if self.assistant().is_some_and(|info| info.summary) {
                    vec![Content::Summary(text)]
                } else {
                    vec![Content::Text(text)]
                }
            }
            Some("step-finish") => Vec::new(),
            // an API error the call is retried after
            Some("retry") => vec![Content::Error(Self::error_text(&part["error"]))],
            Some("reasoning") => {
                vec![Content::Reasoning(part["text"].as_str().unwrap_or_default().to_owned())]
            }
            Some("tool") => {
                let call = ToolCallId::from(part["callID"].as_str().unwrap_or_default().to_owned());
                let state = &part["state"];
                let mut content = vec![Content::ToolUse(ToolUse {
                    id: call.clone(),
                    name: part["tool"].as_str().unwrap_or_default().to_owned(),
                    input: state["input"].clone(),
                })];
                match state["status"].as_str() {
                    Some("completed") => content.push(Content::ToolResult(ToolResult {
                        call,
                        output: state["output"].clone(),
                        error: false,
                    })),
                    Some("error") => content.push(Content::ToolResult(ToolResult {
                        call,
                        output: state["error"].clone(),
                        error: true,
                    })),
                    _ => {}
                }
                content
            }
            _ => vec![Content::Other(part.clone())],
        }
    }

    /// Splits a `message.part.updated.1` payload into its part and the part's timestamp. The
    /// part's own clock (`time.start` on text and reasoning parts, `time.created` on retries,
    /// `state.time.start` on tool parts) beats the envelope's `time`: that is the upsert time, and
    /// opencode re-upserts old parts (streaming revisions, compaction pruning of tool output).
    fn split_part(mut data: Value) -> Result<(Value, Option<i64>), serde_json::Error> {
        let part =
            data.get_mut("part").filter(|part| part.is_object()).map(Value::take).ok_or_else(
                || serde_json::Error::custom("message.part.updated payload has no `part` object"),
            )?;
        let time = part
            .get("time")
            .and_then(|time| time.get("start").or_else(|| time.get("created")))
            .or_else(|| part.get("state")?.get("time")?.get("start"))
            .or_else(|| data.get("time"))
            .and_then(epoch_millis);
        Ok((part, time))
    }

    /// Whether `part` is one opencode is still writing and will upsert again: a text or reasoning
    /// part streaming in (`time.start` without `time.end`), or a tool call `pending` or `running`.
    ///
    /// A status this module does not know counts as finished, since a draft delivered costs the
    /// part that replaces it and a finished part skipped is never delivered at all.
    fn is_draft(part: &Value) -> bool {
        match part["type"].as_str() {
            Some("text" | "reasoning") => {
                !part["time"]["start"].is_null() && part["time"]["end"].is_null()
            }
            Some("tool") => matches!(part["state"]["status"].as_str(), Some("pending" | "running")),
            // every other kind of part is written once
            _ => false,
        }
    }

    fn role_of(role: &str) -> Role {
        match role {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "system" => Role::System,
            "tool" => Role::Tool,
            other => Role::Other(other.to_owned()),
        }
    }
}

impl Message for OpencodeMessage {
    /// A part's id; a failed message's id; for a session's info, its id and title, so that a
    /// title is delivered once however many rows repeat it.
    fn id(&self) -> Option<MessageId> {
        let id = match &self.body {
            Body::Part(value) | Body::Raw(value) => value["id"].as_str()?.to_owned(),
            Body::Failure { id, .. } | Body::Next(Next { id, .. }) => id.clone(),
            Body::Session(info) => {
                format!("{}:title:{}", info["id"].as_str()?, info["title"].as_str()?)
            }
        };
        Some(MessageId::from(id))
    }

    fn role(&self) -> Role {
        self.role.clone()
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.time.and_then(|ms| {
            OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()
        })
    }

    fn content(&self) -> Vec<Content> {
        match &self.body {
            Body::Part(part) => self.part_content(part),
            Body::Failure { info, .. } => vec![Content::Error(Self::error_text(&info["error"]))],
            Body::Next(next) => next.content.clone(),
            Body::Session(_) => Vec::new(),
            Body::Raw(data) => vec![Content::Other(data.clone())],
        }
    }

    fn model(&self) -> Option<String> {
        self.assistant()?.model.clone()
    }

    /// A step's usage, from its `step-finish` part: one per model call, which is what opencode
    /// reports usage for. The message's own `tokens` are not used, as opencode overwrites them
    /// with each step's (`processor.ts`, `step-finish`) and a message of several steps would
    /// report its last one only.
    ///
    /// opencode's `input` already leaves out the cached tokens, and its `output` the reasoning
    /// ones (`Session.getUsage`), which are added back here: reasoning is billed as output (see
    /// `usage_of`). An experimental `session.next.step.ended` row carries its call's usage
    /// likewise.
    fn usage(&self) -> Option<Usage> {
        match &self.body {
            Body::Next(next) => next.usage,
            _ => usage_of(&self.step_finish()?["tokens"]),
        }
    }

    fn stop_reason(&self) -> Option<StopReason> {
        match &self.body {
            Body::Part(_) => self.step_finish()?["reason"].as_str().map(Self::stop_reason_of),
            // the names of `SessionV1.Assistant.error` (`packages/schema/src/v1/session.ts`)
            Body::Failure { info, .. } => Some(match info["error"]["name"].as_str() {
                Some("MessageAbortedError") => StopReason::Aborted,
                // `prompt.ts`: a `content-filter` finish is surfaced as this error
                Some("ContentFilterError") => StopReason::Refusal,
                Some("MessageOutputLengthError") => StopReason::MaxTokens,
                _ => StopReason::Error,
            }),
            Body::Next(next) => next.stop.clone(),
            Body::Session(_) | Body::Raw(_) => None,
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        match &self.body {
            Body::Session(info) => info["directory"].as_str().map(PathBuf::from),
            _ => self.assistant()?.cwd.as_deref().map(PathBuf::from),
        }
    }

    /// The user message an assistant message's rows answer.
    fn parent_id(&self) -> Option<MessageId> {
        self.assistant()?.parent.clone().map(MessageId::from)
    }

    fn parent_session(&self) -> Option<SessionId> {
        match &self.body {
            Body::Session(info) => {
                info["parentID"].as_str().map(|id| SessionId::from(id.to_owned()))
            }
            _ => None,
        }
    }

    /// The model call a row of an assistant message belongs to (see `MessageInfo::turn_of`).
    ///
    /// A `step-finish` part is one model call on its own, whose usage the key has to tell apart
    /// from every other call's: it is its message's key and the counts it reports, which a fork's
    /// copy of it keeps and which two calls that start in the same millisecond, or two steps of
    /// one message, do not share.
    fn turn_id(&self) -> Option<String> {
        let turn = self.assistant().and_then(|info| info.turn.clone());
        let Some(step) = self.step_finish() else {
            return turn;
        };
        let turn = turn.or_else(|| step["messageID"].as_str().map(str::to_owned))?;
        let counts = &step["tokens"];
        let count = |value: &Value| tokens(value).unwrap_or_default();
        Some(format!(
            "{turn}#{}/{}/{}/{}/{}",
            count(&counts["input"]),
            count(&counts["output"]),
            count(&counts["reasoning"]),
            count(&counts["cache"]["read"]),
            count(&counts["cache"]["write"]),
        ))
    }

    /// opencode records no difference between a title it generated and one the user set, so
    /// every title is ranked as generated and the newest wins.
    fn title(&self) -> Option<TitleChange> {
        match &self.body {
            Body::Session(info) => {
                info["title"].as_str().map(|text| TitleChange::new(TitleSource::Generated, text))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::task::Poll;

    use futures::StreamExt;
    use futures::stream::BoxStream;
    use rstest::rstest;

    use super::*;
    #[cfg(unix)]
    use crate::db::sqlite::Journaling;
    use crate::db::sqlite::Sqlite;
    use crate::db::{query, query_as};
    use crate::harnesstools::session::model::{Content, Role};
    use crate::harnesstools::session::{CaptureError, Message, SessionEvent, Sessions};

    fn part_message(part: Value) -> OpencodeMessage {
        let info = MessageInfo {
            role: Role::Assistant,
            ..MessageInfo::unknown()
        };
        OpencodeMessage::part(info, part, Some(1_700_000_000_000))
    }

    #[rstest]
    fn normalizes_a_text_part() {
        let m = part_message(serde_json::json!({"id": "prt_1", "type": "text", "text": "hi"}));
        assert_eq!(m.role(), Role::Assistant);
        assert_eq!(m.content(), vec![Content::Text("hi".into())]);
        assert_eq!(m.id(), Some(MessageId::from("prt_1".to_owned())));
        assert_eq!(m.timestamp().unwrap().unix_timestamp(), 1_700_000_000);
    }

    #[rstest]
    fn normalizes_a_completed_tool_part() {
        let m = part_message(serde_json::json!({
            "type": "tool",
            "callID": "call_1",
            "tool": "bash",
            "state": {"status": "completed", "input": {"command": "ls"}, "output": "files"},
        }));
        let content = m.content();
        assert!(matches!(
            content.as_slice(),
            [Content::ToolUse(u), Content::ToolResult(r)]
                if u.name == "bash"
                    && u.id.as_ref() == "call_1"
                    && u.input == serde_json::json!({"command": "ls"})
                    && r.call.as_ref() == "call_1"
                    && !r.error
                    && r.output == Value::String("files".into())
        ));
    }

    #[rstest]
    fn preserves_a_structured_tool_output() {
        let m = part_message(serde_json::json!({
            "type": "tool",
            "callID": "call_1",
            "tool": "bash",
            "state": {
                "status": "completed",
                "input": {},
                "output": {"stdout": "files", "exit": 0},
            },
        }));
        assert!(matches!(
            m.content().as_slice(),
            [Content::ToolUse(_), Content::ToolResult(r)]
                if r.output == serde_json::json!({"stdout": "files", "exit": 0})
        ));
    }

    #[rstest]
    fn normalizes_an_errored_tool_part() {
        let m = part_message(serde_json::json!({
            "type": "tool",
            "callID": "call_1",
            "tool": "bash",
            "state": {"status": "error", "input": {}, "error": "boom"},
        }));
        assert!(matches!(
            m.content().as_slice(),
            [Content::ToolUse(_), Content::ToolResult(r)]
                if r.error && r.output == Value::String("boom".into())
        ));
    }

    #[rstest]
    fn falls_back_to_other_for_an_unknown_part_type() {
        let raw = serde_json::json!({"id": "prt_1", "type": "step-start", "step": 1});
        assert_eq!(part_message(raw.clone()).content(), vec![Content::Other(raw)]);
    }

    #[rstest]
    #[case::streaming_text(serde_json::json!({"type": "text", "time": {"start": 1}}), true)]
    #[case::finished_text(serde_json::json!({"type": "text", "time": {"start": 1, "end": 2}}), false)]
    #[case::a_prompt_written_once(serde_json::json!({"type": "text", "text": "hi"}), false)]
    #[case::streaming_reasoning(serde_json::json!({"type": "reasoning", "time": {"start": 1}}), true)]
    #[case::finished_reasoning(
        serde_json::json!({"type": "reasoning", "time": {"start": 1, "end": 2}}),
        false
    )]
    #[case::a_pending_call(serde_json::json!({"type": "tool", "state": {"status": "pending"}}), true)]
    #[case::a_running_call(serde_json::json!({"type": "tool", "state": {"status": "running"}}), true)]
    #[case::a_completed_call(
        serde_json::json!({"type": "tool", "state": {"status": "completed"}}),
        false
    )]
    #[case::a_failed_call(serde_json::json!({"type": "tool", "state": {"status": "error"}}), false)]
    #[case::a_status_this_module_does_not_know(
        serde_json::json!({"type": "tool", "state": {"status": "cancelled"}}),
        false
    )]
    #[case::a_step_written_once(serde_json::json!({"type": "step-finish"}), false)]
    fn only_a_part_opencode_is_still_writing_is_a_draft(#[case] part: Value, #[case] draft: bool) {
        assert_eq!(OpencodeMessage::is_draft(&part), draft);
    }

    #[rstest]
    #[case::the_row_a_session_was_offered_on_is_delivered_first(
        Some((5, "f", false)), Some((5, "f")), Resume::From(0))]
    #[case::the_rows_after_the_last_one_delivered_go_on(
        Some((2, "c", true)), Some((2, "c")), Resume::From(1))]
    #[case::an_id_that_sorts_below_the_ones_before_it_is_still_the_same_row(
        Some((2, "b", true)), Some((2, "b")), Resume::From(1))]
    #[case::a_wipe_restarts_the_aggregate_under_the_same_seqs(
        Some((1, "y", true)), Some((1, "a")), Resume::Restart)]
    #[case::a_restored_backup_ends_below_the_row_resumed_at(
        Some((3, "d", true)), None, Resume::Restart)]
    #[case::a_null_id_still_anchors(Some((4, "", true)), Some((4, "")), Resume::From(1))]
    #[case::a_new_incarnation_starts_at_the_first_row(None, Some((0, "a")), Resume::From(0))]
    #[case::an_aggregate_with_no_rows_delivers_nothing(None, None, Resume::From(0))]
    fn a_session_resumes_only_in_the_incarnation_it_left_off_in(
        #[case] anchor: Option<(i64, &str, bool)>,
        #[case] head: Option<(i64, &str)>,
        #[case] expected: Resume,
    ) {
        let anchor = anchor.map(|(seq, id, delivered)| Anchor {
            seq,
            id: id.to_owned(),
            delivered,
        });
        assert_eq!(resume(anchor.as_ref(), head), expected);
    }

    #[rstest]
    #[case::text_part_clock(
        serde_json::json!({"part": {"type": "text", "time": {"start": 1000}}, "time": 9000}),
        Some(1000)
    )]
    #[case::tool_state_clock(
        serde_json::json!({"part": {"type": "tool", "state": {"time": {"start": 2000}}}, "time": 9000}),
        Some(2000)
    )]
    #[case::retry_clock(
        serde_json::json!({"part": {"type": "retry", "time": {"created": 3000}}, "time": 9000}),
        Some(3000)
    )]
    #[case::envelope_fallback(serde_json::json!({"part": {"type": "step-start"}, "time": 9000}), Some(9000))]
    #[case::no_clock(serde_json::json!({"part": {"type": "step-start"}}), None)]
    fn a_part_keeps_its_own_clock(#[case] data: Value, #[case] time: Option<i64>) {
        let (part, got) = OpencodeMessage::split_part(data).unwrap();
        assert!(part.is_object());
        assert_eq!(got, time);
    }

    #[rstest]
    #[case::string(serde_json::json!("nope"))]
    #[case::part_not_an_object(serde_json::json!({"part": [1, 2]}))]
    #[case::no_part(serde_json::json!({"time": 1}))]
    fn a_shapeless_part_payload_is_an_error(#[case] data: Value) {
        assert!(OpencodeMessage::split_part(data).is_err());
    }

    /// opencode's `event` table (a rowid table: `id` is a TEXT primary key) with the unique
    /// `(aggregate_id, seq)` index a session pages its rows by, and its `message` projection, in
    /// WAL mode like the real database.
    async fn event_db(path: &Path) -> Sqlite {
        with_schema(Sqlite::builder(path.as_os_str()).open().await.unwrap()).await
    }

    /// The event log itself, as a migration that wipes it leaves it behind.
    const EVENT_DDL: [&str; 2] = [
        "CREATE TABLE event (id TEXT PRIMARY KEY, aggregate_id TEXT NOT NULL, seq INTEGER NOT \
         NULL, type TEXT NOT NULL, data TEXT NOT NULL)",
        "CREATE UNIQUE INDEX event_aggregate_seq_idx ON event (aggregate_id, seq)",
    ];

    async fn with_schema(sqlite: Sqlite) -> Sqlite {
        let message = "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, \
                       time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT \
                       NOT NULL)";
        let mut conn = sqlite.pool().acquire().await.unwrap();
        for ddl in EVENT_DDL.into_iter().chain([message]) {
            query::<sqlx::Sqlite>(ddl).execute(&mut *conn).await.unwrap();
        }
        sqlite
    }

    /// Appends like opencode does: `seq` continues the aggregate's sequence, or restarts at 0
    /// when its rows are gone.
    async fn insert_event(sqlite: &Sqlite, id: &str, aggregate: &str, kind: &str, data: &str) {
        query::<sqlx::Sqlite>(
            "INSERT INTO event (id, aggregate_id, seq, type, data) VALUES (?1, ?2, (SELECT \
             coalesce(max(seq), -1) + 1 FROM event WHERE aggregate_id = ?2), ?3, ?4)",
        )
        .bind(id)
        .bind(aggregate)
        .bind(kind)
        .bind(data)
        .execute(&mut *sqlite.pool().acquire().await.unwrap())
        .await
        .unwrap();
    }

    async fn execute(sqlite: &Sqlite, sql: &'static str) {
        query::<sqlx::Sqlite>(sql)
            .execute(&mut *sqlite.pool().acquire().await.unwrap())
            .await
            .unwrap();
    }

    /// `(rowid, seq)` of an event: the positions the tail and a session's pages go by.
    async fn position(sqlite: &Sqlite, id: &str) -> (i64, i64) {
        query_as::<sqlx::Sqlite, (i64, i64)>("SELECT rowid, seq FROM event WHERE id = ?1")
            .bind(id)
            .fetch_one(&mut *sqlite.pool().acquire().await.unwrap())
            .await
            .unwrap()
    }

    fn role_event(message: &str, role: &str) -> String {
        serde_json::json!({"info": {"id": message, "role": role}}).to_string()
    }

    fn text_event(part: &str, message: &str, text: &str) -> String {
        serde_json::json!({
            "part": {"id": part, "messageID": message, "type": "text", "text": text},
            "time": 1_700_000_000_000i64,
        })
        .to_string()
    }

    /// A `message.updated.1` row carrying `message`'s role.
    async fn role_row(db: &Sqlite, id: &str, session: &str, message: &str, role: &str) {
        insert_event(db, id, session, "message.updated.1", &role_event(message, role)).await;
    }

    /// The `message` projection row opencode keeps alongside a message's `message.updated.1`
    /// rows: its `data` is the message info minus `id` and `sessionID`.
    async fn message_row(db: &Sqlite, message: &str, session: &str, role: &str) {
        query::<sqlx::Sqlite>(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, \
             ?2, 0, 0, ?3)",
        )
        .bind(message)
        .bind(session)
        .bind(serde_json::json!({"role": role, "time": {"created": 0}}).to_string())
        .execute(&mut *db.pool().acquire().await.unwrap())
        .await
        .unwrap();
    }

    /// A `message.part.updated.1` row with a text part of `message`.
    async fn text_row(db: &Sqlite, id: &str, session: &str, part: &str, message: &str, text: &str) {
        insert_event(db, id, session, "message.part.updated.1", &text_event(part, message, text))
            .await;
    }

    /// A `message.part.updated.1` row with `message`'s `call_1` tool part in `state`.
    async fn tool_row(
        db: &Sqlite,
        id: &str,
        session: &str,
        part: &str,
        message: &str,
        state: Value,
    ) {
        let data = serde_json::json!({
            "part": {
                "id": part,
                "messageID": message,
                "type": "tool",
                "callID": "call_1",
                "tool": "bash",
                "state": state,
            },
            "time": 1_700_000_000_000i64,
        });
        insert_event(db, id, session, "message.part.updated.1", &data.to_string()).await;
    }

    /// opencode ids from either side of the wrap of their 48-bit time prefix (2026-08-14, then
    /// every 795 days): every id from before it sorts above every id from after it.
    fn pre_wrap(n: u32) -> String {
        format!("evt_fffffffff{n:03x}aaaaaaaaaaaaaa")
    }

    fn post_wrap(n: u32) -> String {
        format!("evt_000000000{n:03x}aaaaaaaaaaaaaa")
    }

    type Events = BoxStream<'static, Result<SessionEvent<OpencodeMessage>, CaptureError>>;

    fn events(path: &Path, replay: ReplayBehavior) -> Events {
        OpencodeSessions::builder()
            .db(path)
            .replay(replay)
            .build()
            .listener()
            .unwrap()
            .events(|_| async { None })
            .boxed()
    }

    /// The next `n` items, failing instead of hanging when the tail never delivers them.
    async fn next_n<S: Stream + Unpin + Send>(stream: &mut S, n: usize) -> Vec<S::Item> {
        tokio::time::timeout(Duration::from_secs(10), stream.by_ref().take(n).collect())
            .await
            .unwrap_or_else(|_| panic!("the stream did not deliver {n} items within 10s"))
    }

    /// One line per event, the way a consumer would log it: `<session> <role> <text>` or
    /// `<session> <error kind>`.
    fn describe(event: &Result<SessionEvent<OpencodeMessage>, CaptureError>) -> String {
        match event {
            Ok(SessionEvent {
                session, message, ..
            }) => {
                let content = message
                    .content()
                    .into_iter()
                    .map(|content| match content {
                        Content::Text(text) => text,
                        other => format!("{other:?}"),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{session} {:?} {content}", message.role())
            }
            Err(CaptureError::Message {
                session,
                source: MessageError::Json(_),
            }) => format!("{session} json error"),
            Err(CaptureError::Message { session, .. }) => format!("{session} error"),
            Err(CaptureError::Watch(err)) => format!("watch error: {err}"),
        }
    }

    async fn described(stream: &mut Events, n: usize) -> Vec<String> {
        next_n(stream, n).await.iter().map(describe).collect()
    }

    /// The messages among the next `n` events of a capture.
    async fn captured(stream: &mut Events, n: usize) -> Vec<OpencodeMessage> {
        next_n(stream, n).await.into_iter().map(|event| event.unwrap().message).collect()
    }

    /// `ReplayBehavior::FromNow` anchors where the table ends when the stream is *first polled*, so a row
    /// committed before that poll is legitimately skipped: keeps appending rows for `aggregate`
    /// until the stream yields, and hands back that first item.
    async fn first_while_appending<S: Stream + Unpin + Send>(
        stream: &mut S,
        sqlite: &Sqlite,
        aggregate: &str,
        kind: &str,
        data: impl Fn(usize) -> String + Send + Sync,
    ) -> S::Item {
        let appends = async {
            for i in 0..500 {
                insert_event(sqlite, &format!("live{i:04}"), aggregate, kind, &data(i)).await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::select! {
            biased;
            item = stream.next() => item.expect("the stream ended"),
            () = appends => panic!("no live row was delivered within 10s"),
        }
    }

    #[rstest]
    fn listener_reports_not_found_for_a_missing_db() {
        let sessions =
            OpencodeSessions::builder().db(PathBuf::from("/no/such/opencode.db")).build();
        assert!(matches!(sessions.listener(), Err(RuntimeError::NotFound(_))));
    }

    #[rstest]
    fn discover_prefers_opencode_db_then_the_newest_channel_db() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        for name in [
            "opencode.db",
            "opencode-dev.db",
            "opencode-old.db",
            "opencode-old.db-shm",
            "opencode-dev.db-wal",
            "opencode2.db",
            "notopencode.db",
            "opencode-x.txt",
        ] {
            std::fs::write(base.join(name), b"").unwrap();
        }
        std::fs::create_dir(base.join("opencode-dir.db")).unwrap();
        let set_mtime = |name: &str, secs: u64| {
            std::fs::OpenOptions::new()
                .write(true)
                .open(base.join(name))
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        };
        set_mtime("opencode.db", 1);
        set_mtime("opencode-old.db", 2000);
        set_mtime("opencode-old.db-shm", 9000);
        set_mtime("opencode-dev.db", 1000);
        set_mtime("opencode-dev.db-wal", 3000);
        set_mtime("opencode2.db", 9000);

        assert_eq!(OpencodeSessions::discover(base), Some(base.join("opencode.db")));

        std::fs::remove_file(base.join("opencode.db")).unwrap();
        assert_eq!(OpencodeSessions::discover(base), Some(base.join("opencode-dev.db")));

        std::fs::remove_dir(base.join("opencode-dir.db")).unwrap();
        std::fs::create_dir(base.join("opencode.db")).unwrap();
        assert_eq!(OpencodeSessions::discover(base), Some(base.join("opencode-dev.db")));
    }

    #[rstest]
    #[tokio::test]
    async fn a_db_without_an_event_table_is_reported_as_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        drop(Sqlite::builder(path.as_os_str()).open().await.unwrap());

        let listener = OpencodeSessions::builder().db(path).build().listener().unwrap();
        let mut stream = listener.watch().boxed();
        assert!(matches!(
            stream.next().await,
            Some(Err(WatchError::Io(err))) if err.kind() == io::ErrorKind::NotFound
        ));
        assert!(stream.next().await.is_none());
    }

    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    async fn events_demultiplex_sessions_on_one_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "assistant").await;
        role_row(&db, "e2", "ses_2", "msg_2", "user").await;
        text_row(&db, "e3", "ses_1", "p1", "msg_1", "hello").await;
        text_row(&db, "e4", "ses_2", "p2", "msg_2", "world").await;

        let mut got = described(&mut events(&path, ReplayBehavior::All), 2).await;
        got.sort();
        assert_eq!(got, ["ses_1 Assistant hello", "ses_2 User world"]);
    }

    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    async fn a_session_drains_past_one_page_on_one_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "seed", "ses_1", "msg_1", "assistant").await;
        let parts = PAGE + 8;
        for i in 0..parts {
            text_row(
                &db,
                &format!("p{i:03}"),
                "ses_1",
                &format!("prt_{i}"),
                "msg_1",
                &i.to_string(),
            )
            .await;
        }

        let got = described(&mut events(&path, ReplayBehavior::All), parts).await;
        let expected: Vec<String> = (0..parts).map(|i| format!("ses_1 Assistant {i}")).collect();
        assert_eq!(got, expected);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_from_now_skips_preexisting_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_pre", "m1", "user").await;

        let mut stream = events(&path, ReplayBehavior::FromNow);
        let first =
            first_while_appending(&mut stream, &db, "ses_new", "message.part.updated.1", |i| {
                text_event(&format!("prt_{i}"), "m2", "after")
            })
            .await;
        assert_eq!(first.unwrap().session, SessionId::from("ses_new".to_owned()));
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_all_backfills_preexisting_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_pre", "m1", "user").await;
        text_row(&db, "e2", "ses_pre", "prt_1", "m1", "before").await;

        assert_eq!(described(&mut events(&path, ReplayBehavior::All), 1).await, [
            "ses_pre User before"
        ]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rows_written_after_a_session_delete_are_delivered_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        // ses_Y started before the id wrap and went on after it
        role_row(&db, &pre_wrap(1), "ses_Y", "msg_y1", "user").await;
        text_row(&db, &pre_wrap(2), "ses_Y", "prt_y1", "msg_y1", "one").await;
        text_row(&db, &post_wrap(3), "ses_Y", "prt_y2", "msg_y1", "two").await;
        role_row(&db, &post_wrap(4), "ses_X", "msg_x1", "user").await;
        text_row(&db, &post_wrap(5), "ses_X", "prt_x1", "msg_x1", "gone").await;

        let mut stream = events(&path, ReplayBehavior::All);
        let mut before = described(&mut stream, 3).await;
        before.sort();
        assert_eq!(before, ["ses_X User gone", "ses_Y User one", "ses_Y User two"]);

        // deleting the newest session frees the highest rowids, which ses_Y's next rows take
        execute(&db, "DELETE FROM event WHERE aggregate_id = 'ses_X'").await;
        role_row(&db, &post_wrap(6), "ses_Y", "msg_y2", "assistant").await;
        text_row(&db, &post_wrap(7), "ses_Y", "prt_y3", "msg_y2", "three").await;
        assert_eq!(position(&db, &post_wrap(6)).await, (4, 3), "rowids were not recycled");

        assert_eq!(described(&mut stream, 1).await, ["ses_Y Assistant three"]);
        text_row(&db, &post_wrap(8), "ses_Y", "prt_y4", "msg_y2", "four").await;
        assert_eq!(described(&mut stream, 1).await, ["ses_Y Assistant four"]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_rewind_re_delivers_nothing_of_a_session_whose_ids_stepped_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        // ses_X's writer's clock stepped back before its third row (a time correction, or the
        // session moving to another opencode process): the id sorts below the two before it
        // while seq goes on, so the newest delivered id is not the highest one
        role_row(&db, &post_wrap(5), "ses_X", "msg_x", "user").await;
        text_row(&db, &post_wrap(6), "ses_X", "prt_x1", "msg_x", "one").await;
        text_row(&db, &post_wrap(2), "ses_X", "prt_x2", "msg_x", "two").await;
        role_row(&db, &post_wrap(8), "ses_Z", "msg_z", "user").await;
        text_row(&db, &post_wrap(9), "ses_Z", "prt_z", "msg_z", "gone").await;

        let mut stream = events(&path, ReplayBehavior::All);
        let mut before = described(&mut stream, 3).await;
        before.sort();
        assert_eq!(before, ["ses_X User one", "ses_X User two", "ses_Z User gone"]);

        // deleting the newest session takes the tail's anchor row with it: the tail rewinds and
        // ses_X's rows come by again, but a session reads by its own watermark and nothing it
        // already delivered is yielded twice
        execute(&db, "DELETE FROM event WHERE aggregate_id = 'ses_Z'").await;
        text_row(&db, &post_wrap(10), "ses_X", "prt_x3", "msg_x", "three").await;
        assert_eq!(position(&db, &post_wrap(10)).await, (4, 3), "rowids were not recycled");

        assert_eq!(described(&mut stream, 1).await, ["ses_X User three"]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rows_written_after_an_event_log_wipe_are_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, &pre_wrap(1), "ses_A", "msg_a1", "user").await;
        text_row(&db, &pre_wrap(2), "ses_A", "prt_a1", "msg_a1", "one").await;

        let mut stream = events(&path, ReplayBehavior::All);
        assert_eq!(described(&mut stream, 1).await, ["ses_A User one"]);

        // opencode's reset migrations: rowids and the aggregate's seq both start over, and the
        // session resumed afterwards writes ids that sort below its old ones
        execute(&db, "DELETE FROM event").await;
        role_row(&db, &post_wrap(3), "ses_A", "msg_a2", "assistant").await;
        text_row(&db, &post_wrap(4), "ses_A", "prt_a2", "msg_a2", "two").await;
        assert_eq!(position(&db, &post_wrap(3)).await, (1, 0));

        assert_eq!(described(&mut stream, 1).await, ["ses_A Assistant two"]);
    }

    /// The `event` table is gone for longer than one poll -- a reset migration caught between its
    /// `DROP` and its `CREATE`, rather than inside a single tick -- so the tail meets a query
    /// error it cannot read past. It reports it and takes the table up again: the sessions
    /// written once the table is back are captured all the same.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn capture_resumes_after_the_event_table_is_gone_for_a_while() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_A", "msg_a", "user").await;
        text_row(&db, "e2", "ses_A", "prt_a", "msg_a", "before").await;

        let mut stream = events(&path, ReplayBehavior::All);
        assert_eq!(described(&mut stream, 1).await, ["ses_A User before"]);

        // one connection for the whole migration, as opencode's own runs: a pooled connection
        // that has not read the schema since the `DROP` resolves `CREATE` against the table it
        // still has cached, and fails with an `already exists` that names nothing on disk
        let mut migration = db.pool().acquire().await.unwrap();
        query::<sqlx::Sqlite>("DROP TABLE event").execute(&mut *migration).await.unwrap();
        let reported = next_n(&mut stream, 1).await.pop().expect("the capture ended");
        assert!(matches!(reported, Err(CaptureError::Watch(WatchError::Observe { .. }))));

        for ddl in EVENT_DDL {
            query::<sqlx::Sqlite>(ddl).execute(&mut *migration).await.unwrap();
        }
        drop(migration);
        role_row(&db, "e3", "ses_B", "msg_b", "assistant").await;
        text_row(&db, "e4", "ses_B", "prt_b", "msg_b", "after").await;

        let mut captured = Vec::new();
        while captured.is_empty() {
            let line = described(&mut stream, 1).await.pop().expect("the capture ended");
            // the polls that still met no table report it too, and the tail retries after each
            if !line.starts_with("watch error") {
                captured.push(line);
            }
        }
        assert_eq!(captured, ["ses_B Assistant after"]);
    }

    #[cfg(unix)]
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rows_written_after_a_restored_backup_are_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "a0", "ses_A", "msg_a", "user").await;
        for (id, text) in [("a1", "one"), ("a2", "two"), ("a3", "three")] {
            text_row(&db, id, "ses_A", &format!("prt_{id}"), "msg_a", text).await;
        }

        let mut stream = events(&path, ReplayBehavior::All);
        assert_eq!(described(&mut stream, 3).await, [
            "ses_A User one",
            "ses_A User two",
            "ses_A User three"
        ]);

        // the database is put back from a backup taken after `one`: the rows and the aggregate's
        // sequence roll back together, so the resumed session writes new rows at the seqs of
        // `two` and `three`
        db.pool().close().await;
        drop(db);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(dir.path().join(format!("opencode.db{suffix}")));
        }
        let backup = dir.path().join("backup.db");
        let restored = with_schema(
            Sqlite::builder(backup.as_os_str())
                .journal(Some(Journaling::Delete))
                .open()
                .await
                .unwrap(),
        )
        .await;
        role_row(&restored, "a0", "ses_A", "msg_a", "user").await;
        text_row(&restored, "a1", "ses_A", "prt_a1", "msg_a", "one").await;
        restored.pool().close().await;
        std::fs::rename(&backup, &path).unwrap();
        let db = Sqlite::builder(path.as_os_str()).open().await.unwrap();
        text_row(&db, "b2", "ses_A", "prt_b2", "msg_a", "two again").await;
        text_row(&db, "b3", "ses_A", "prt_b3", "msg_a", "three again").await;
        assert_eq!(position(&db, "b2").await, (3, 2));

        // the row the session resumed at went with the rows the backup does not hold, so it reads
        // its aggregate again from the start: what the backup still holds is delivered a second
        // time, and the rows written since are delivered at all
        assert_eq!(described(&mut stream, 3).await, [
            "ses_A User one",
            "ses_A User two again",
            "ses_A User three again"
        ]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_ignored_first_row_spawns_no_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        // a revert's removal and a deletion are rows no session reads
        let removed = r#"{"sessionID":"ses_old","messageID":"msg_0"}"#;
        insert_event(&db, "e1", "ses_old", "message.removed.1", removed).await;
        insert_event(&db, "e2", "ses_gone", "session.deleted.1", r#"{"sessionID":"ses_gone"}"#)
            .await;
        role_row(&db, "e3", "ses_1", "msg_1", "user").await;
        text_row(&db, "e4", "ses_1", "prt_1", "msg_1", "hi").await;

        let mut stream = watch(&path);
        assert_eq!(offered(&mut stream).await.id(), SessionId::from("ses_1".to_owned()));
    }

    type Offers = BoxStream<'static, Result<OpencodeSession, WatchError>>;

    fn watch(path: &Path) -> Offers {
        OpencodeSessions::builder().db(path).build().listener().unwrap().watch().boxed()
    }

    /// The next session `watch()` offers, failing instead of hanging.
    async fn offered(stream: &mut Offers) -> OpencodeSession {
        next_n(stream, 1).await.pop().expect("watch() ended").unwrap()
    }

    /// The first `n` messages of `session`, with `stream` driven alongside so that rows written
    /// while they are read are noticed.
    async fn delivered(
        stream: Offers,
        session: OpencodeSession,
        n: usize,
    ) -> Vec<(Role, Vec<Content>)> {
        let messages = session.messages().map(|message| Some(message.unwrap()));
        let driven = futures::stream::select(stream.map(|_| None), messages);
        tokio::time::timeout(
            Duration::from_secs(10),
            driven.filter_map(futures::future::ready).take(n).collect::<Vec<_>>(),
        )
        .await
        .expect("the session did not deliver its messages within 10s")
        .iter()
        .map(|m| (m.role(), m.content()))
        .collect()
    }

    /// The session's next message; the handle is dropped afterwards.
    async fn take_one(session: OpencodeSession) -> Vec<Content> {
        let mut messages = session.messages().boxed();
        next_n(&mut messages, 1).await.pop().unwrap().unwrap().content()
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dropped_session_handle_is_offered_again_with_nothing_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "user").await;
        text_row(&db, "e2", "ses_1", "prt_1", "msg_1", "one").await;

        let mut stream = watch(&path);
        // offered on its role row and dropped before anything was read from it
        let first = offered(&mut stream).await;
        assert_eq!(first.id(), SessionId::from("ses_1".to_owned()));
        drop(first);
        // its next row offers it again, and it still starts at its first row
        let second = offered(&mut stream).await;
        assert_eq!(second.id(), SessionId::from("ses_1".to_owned()));
        assert_eq!(take_one(second).await, vec![Content::Text("one".into())]);

        // dropped after reading: the next row offers it once more, resuming where it stopped
        text_row(&db, "e3", "ses_1", "prt_2", "msg_1", "two").await;
        let third = offered(&mut stream).await;
        assert_eq!(third.id(), SessionId::from("ses_1".to_owned()));
        assert_eq!(delivered(stream, third, 1).await, vec![(Role::User, vec![Content::Text(
            "two".into()
        )])]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dropped_offer_waits_for_the_session_s_next_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "user").await;
        text_row(&db, "e2", "ses_1", "prt_1", "msg_1", "one").await;
        role_row(&db, "e3", "ses_2", "msg_2", "user").await;
        text_row(&db, "e4", "ses_2", "prt_2", "msg_2", "two").await;

        // a consumer that drops every offer: each session is offered on its role row and again on
        // its part, and then there is no row left to offer either of them on
        let mut stream = watch(&path);
        let mut ids = Vec::new();
        for _ in 0..4 {
            ids.push(offered(&mut stream).await.id().to_string());
        }
        assert_eq!(ids, ["ses_1", "ses_1", "ses_2", "ses_2"]);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), stream.next()).await.is_err(),
            "watch() offered a session without a row of it"
        );

        // the next row offers the session again, with the rows that waited still ahead of it
        text_row(&db, "e5", "ses_1", "prt_3", "msg_1", "three").await;
        let again = offered(&mut stream).await;
        assert_eq!(again.id(), SessionId::from("ses_1".to_owned()));
        assert_eq!(delivered(stream, again, 2).await, vec![
            (Role::User, vec![Content::Text("one".into())]),
            (Role::User, vec![Content::Text("three".into())]),
        ]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_taken_up_again_resumes_where_it_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "user").await;
        for (id, text) in [("e2", "one"), ("e3", "two"), ("e4", "three")] {
            text_row(&db, id, "ses_1", &format!("prt_{id}"), "msg_1", text).await;
        }

        let mut stream = watch(&path);
        drop(offered(&mut stream).await);
        // a consumer that reads one message per offer and lets go: each of the session's next
        // rows offers it again, and it never repeats a message nor skips one
        let mut texts = Vec::new();
        for _ in 0..3 {
            texts.push(take_one(offered(&mut stream).await).await);
        }
        assert_eq!(texts, [
            vec![Content::Text("one".into())],
            vec![Content::Text("two".into())],
            vec![Content::Text("three".into())],
        ]);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), stream.next()).await.is_err(),
            "watch() offered a session without a row of it"
        );
    }

    /// A checkpoint names its row by id as well as `seq`: it resumes the session past that row
    /// while the row is there, and once a wipe has put other rows under the same seqs the session
    /// reads its aggregate again from the start.
    #[rstest]
    #[case::the_row_is_still_there(false, ["two", "three"])]
    #[case::a_wipe_reused_its_seq(true, ["three", "four"])]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_checkpoint_resumes_only_past_the_row_it_was_taken_after(
        #[case] wipe: bool,
        #[case] expected: [&str; 2],
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "user").await;
        text_row(&db, "e2", "ses_1", "prt_e2", "msg_1", "one").await;
        text_row(&db, "e3", "ses_1", "prt_e3", "msg_1", "two").await;

        let mut stream = watch(&path);
        let mut first = offered(&mut stream).await.messages_from(None).boxed();
        let (after_one, _) = next_n(&mut first, 1).await.pop().unwrap().unwrap();
        drop(first);

        if wipe {
            execute(&db, "DELETE FROM event").await;
            role_row(&db, "f1", "ses_1", "msg_2", "user").await;
            text_row(&db, "f2", "ses_1", "prt_f2", "msg_2", "three").await;
            text_row(&db, "f3", "ses_1", "prt_f3", "msg_2", "four").await;
        } else {
            text_row(&db, "e4", "ses_1", "prt_e4", "msg_1", "three").await;
        }
        let mut resumed = offered(&mut stream).await.messages_from(Some(after_one)).boxed();
        let contents: Vec<Vec<Content>> = next_n(&mut resumed, 2)
            .await
            .into_iter()
            .map(|item| item.unwrap().1.content())
            .collect();
        assert_eq!(contents, expected.map(|text| vec![Content::Text(text.into())]));
    }

    /// What `stream` delivers before it goes quiet, or before it has had to wait `waits` times,
    /// whichever comes first. Dropping it at a wait is what a consumer that lets go of a session
    /// mid-row looks like from in here: a task that is aborted, a `select!` that loses its race,
    /// a shutdown.
    async fn until_wait<S: Stream + Unpin>(stream: &mut S, waits: Option<usize>) -> Vec<S::Item> {
        let mut got = Vec::new();
        let mut waited = 0;
        let waiting = futures::future::poll_fn(|cx| {
            loop {
                match stream.poll_next_unpin(cx) {
                    Poll::Ready(Some(item)) => got.push(item),
                    Poll::Ready(None) => return Poll::Ready(()),
                    Poll::Pending => {
                        waited += 1;
                        return if waits == Some(waited) {
                            Poll::Ready(())
                        } else {
                            Poll::Pending
                        };
                    }
                }
            }
        });
        // short of its tail going, a session's stream does not end: with nothing left to read it
        // waits for a row, and these tests are done writing them
        let _ = tokio::time::timeout(Duration::from_secs(1), waiting).await;
        got
    }

    /// The text of each message: every row this test writes carries exactly one text part.
    fn texts(messages: Vec<Result<OpencodeMessage, MessageError>>) -> Vec<String> {
        messages
            .into_iter()
            .map(|message| match message.unwrap().content().as_slice() {
                [Content::Text(text)] => text.clone(),
                other => panic!("expected a single text part, got {other:?}"),
            })
            .collect()
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_dropped_while_it_waits_loses_no_row(
        #[values(0, 1)] taken: usize,
        #[values(1, 2, 3, 4, 5, 6)] waits: usize,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        // no `message.updated.1` row and no `message` projection, so every part's role is looked
        // up in the database: the reader waits there holding a row no consumer has yet
        text_row(&db, "e0", "ses_1", "prt_0", "msg_1", "zero").await;
        text_row(&db, "e1", "ses_1", "prt_1", "msg_1", "one").await;

        let mut stream = watch(&path);
        let mut messages = offered(&mut stream).await.messages().boxed();
        // with a message already taken the page holds the next row, so the first wait after it is
        // that row's role lookup: the reader is in the middle of a row when the stream goes
        let mut before = texts(next_n(&mut messages, taken).await);
        before.extend(texts(until_wait(&mut messages, Some(waits)).await));
        drop(messages);

        // the session's next row offers it again, and the new handle carries on from the last row
        // a consumer actually has rather than from wherever the dropped one had read to
        text_row(&db, "e2", "ses_1", "prt_2", "msg_1", "two").await;
        let mut again = offered(&mut stream).await.messages().boxed().take(3 - before.len());
        let after = texts(until_wait(&mut again, None).await);
        assert_eq!([before, after].concat(), ["zero", "one", "two"]);
    }

    /// A session nobody reads holds none of its rows: the page a dropped stream was being drained
    /// through goes with it, and what it had not delivered is read out of the table again. Told
    /// apart by the payloads, the weight a page carries: rewritten in the table while no consumer
    /// holds the session, they come back rewritten rather than as the dropped page had them.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_nobody_reads_holds_none_of_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "user").await;
        for (id, text) in [("e2", "one"), ("e3", "two"), ("e4", "three")] {
            text_row(&db, id, "ses_1", &format!("prt_{id}"), "msg_1", text).await;
        }

        // one message of a page of four rows taken, and the session let go of with the rest of
        // them read but undelivered
        let mut stream = watch(&path);
        let mut messages = offered(&mut stream).await.messages().boxed();
        assert_eq!(texts(next_n(&mut messages, 1).await), ["one"]);
        drop(messages);

        execute(&db, r#"UPDATE event SET data = replace(data, '"text":"', '"text":"re-read ')"#)
            .await;
        text_row(&db, "e5", "ses_1", "prt_e5", "msg_1", "four").await;

        let mut again = offered(&mut stream).await.messages().boxed();
        assert_eq!(texts(next_n(&mut again, 3).await), ["re-read two", "re-read three", "four"]);
    }

    /// A session outlives its tail: with the tail gone nothing can wake it again, so it reads
    /// what is left of its aggregate before it ends rather than abandoning rows in the table.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_whose_tail_is_gone_reads_its_aggregate_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "user").await;
        text_row(&db, "e2", "ses_1", "prt_1", "msg_1", "one").await;

        let mut stream = watch(&path);
        let mut messages = offered(&mut stream).await.messages().boxed();
        assert_eq!(texts(next_n(&mut messages, 1).await), ["one"]);

        // the consumer lets go of the tail, and a row of the session reaches the table after
        // it: there is nothing left to wake the session for that row
        drop(stream);
        text_row(&db, "e3", "ses_1", "prt_2", "msg_1", "two").await;

        assert_eq!(texts(until_wait(&mut messages, None).await), ["two"]);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), messages.next()).await.unwrap().is_none(),
            "a session that has read its aggregate out and has no tail left must end"
        );
    }

    /// `sessions` sessions `ses_0000`.. of a user role row and `parts` text parts each, in one
    /// statement.
    async fn seed_sessions(db: &Sqlite, sessions: usize, parts: usize) {
        query::<sqlx::Sqlite>(
            r#"INSERT INTO event (id, aggregate_id, seq, type, data)
            WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < ?1)
            SELECT printf('e%06d', i), printf('ses_%04d', i / ?2), i % ?2,
                CASE WHEN i % ?2 = 0 THEN 'message.updated.1' ELSE 'message.part.updated.1' END,
                CASE WHEN i % ?2 = 0
                    THEN printf('{"info":{"id":"msg_%04d","role":"user"}}', i / ?2)
                    ELSE printf('{"part":{"id":"prt_%06d","messageID":"msg_%04d","type":"text","text":"x"},"time":1700000000000}', i, i / ?2)
                END
            FROM n"#,
        )
        .bind(i64::try_from(sessions * (parts + 1)).unwrap())
        .bind(i64::try_from(parts + 1).unwrap())
        .execute(&mut *db.pool().acquire().await.unwrap())
        .await
        .unwrap();
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_that_is_never_drained_stalls_no_other() {
        // sessions of more rows than a page each, so a demux that copied rows into memory would
        // park on the first of them and never reach the rest of the table
        const STALLED: usize = 4;
        const PARTS: usize = PAGE + 3;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        seed_sessions(&db, STALLED, PARTS).await;
        role_row(&db, "z1", "ses_z", "msg_z", "user").await;
        text_row(&db, "z2", "ses_z", "prt_z", "msg_z", "after").await;

        let mut stream = watch(&path);
        let held: Vec<OpencodeSession> =
            next_n(&mut stream, STALLED).await.into_iter().map(Result::unwrap).collect();
        let ids: Vec<String> = held.iter().map(|session| session.id().to_string()).collect();
        assert_eq!(ids, (0..STALLED).map(|i| format!("ses_{i:04}")).collect::<Vec<_>>());

        // held and never read, yet the session written after them is offered and delivers
        let last = offered(&mut stream).await;
        assert_eq!(last.id(), SessionId::from("ses_z".to_owned()));
        assert_eq!(delivered(stream, last, 1).await, vec![(Role::User, vec![Content::Text(
            "after".into()
        )])]);

        // and the rows of a session that was held all along waited for it in the table
        let mut messages = held.into_iter().next().unwrap().messages().boxed();
        let parts = next_n(&mut messages, PARTS).await;
        assert_eq!(parts.len(), PARTS);
        assert!(parts.iter().all(|m| m.as_ref().unwrap().role() == Role::User));
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_left_mid_read_stalls_no_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        for session in ["ses_a", "ses_b"] {
            role_row(&db, &format!("{session}1"), session, "msg_1", "user").await;
            text_row(&db, &format!("{session}2"), session, "prt_1", "msg_1", "hello").await;
        }

        let mut stream = watch(&path);
        let a = offered(&mut stream).await;
        let b = offered(&mut stream).await;

        // one poll suspends A inside its page read; the stream is then kept, never polled again
        let mut a_messages = a.messages().boxed();
        assert!(futures::poll!(a_messages.next()).is_pending(), "A read its page in one poll");

        assert_eq!(delivered(stream, b, 1).await, vec![(Role::User, vec![Content::Text(
            "hello".into()
        )])]);
    }

    /// opencode upserts a part under its own id as it is written: a streamed text part empty and
    /// then whole, a tool call at every state it passes through. A consumer keeps the first
    /// message of an id, so a draft delivered ahead of its part would stand in for it.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_part_is_delivered_only_once_it_is_finished() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "assistant").await;
        for (id, time, text) in [
            ("e2", serde_json::json!({"start": 1}), ""),
            ("e3", serde_json::json!({"start": 1, "end": 2}), "hello"),
        ] {
            let part = serde_json::json!({
                "id": "prt_1",
                "messageID": "msg_1",
                "type": "text",
                "text": text,
                "time": time,
            });
            let data = serde_json::json!({"part": part}).to_string();
            insert_event(&db, id, "ses_1", "message.part.updated.1", &data).await;
        }
        for (id, state) in [
            ("e4", serde_json::json!({"status": "pending"})),
            ("e5", serde_json::json!({"status": "running", "input": {"command": "ls"}})),
            (
                "e6",
                serde_json::json!({
                    "status": "completed",
                    "input": {"command": "ls"},
                    "output": "files",
                }),
            ),
        ] {
            tool_row(&db, id, "ses_1", "prt_2", "msg_1", state).await;
        }

        let messages = captured(&mut events(&path, ReplayBehavior::All), 2).await;

        let content: Vec<Content> = messages.iter().flat_map(Message::content).collect();
        assert_eq!(content, vec![
            Content::Text("hello".into()),
            Content::ToolUse(ToolUse {
                id: ToolCallId::from("call_1".to_owned()),
                name: "bash".to_owned(),
                input: serde_json::json!({"command": "ls"}),
            }),
            Content::ToolResult(ToolResult {
                call: ToolCallId::from("call_1".to_owned()),
                output: Value::String("files".to_owned()),
                error: false,
            }),
        ]);
    }

    #[rstest]
    #[case::from_the_projection(true, Role::User)]
    #[case::unknown_without_it(false, Role::Other("unknown".to_owned()))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_part_whose_role_row_predates_the_tail_takes_its_role_from_the_projection(
        #[case] projected: bool,
        #[case] role: Role,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "user").await;
        if projected {
            message_row(&db, "msg_1", "ses_1", "user").await;
        }

        // the tail starts between the message's role row and its first part
        let mut stream = events(&path, ReplayBehavior::FromNow);
        let first =
            first_while_appending(&mut stream, &db, "ses_1", "message.part.updated.1", |i| {
                text_event(&format!("prt_{i}"), "msg_1", "hi")
            })
            .await;
        assert_eq!(describe(&first), format!("ses_1 {role:?} hi"));
    }

    /// A session's roles do not pile up with the messages it reads: it keeps the newest few, and
    /// one it has dropped since costs a lookup in the projection rather than the role itself.
    ///
    /// Written `interleaved`, each role row comes where opencode writes it -- immediately ahead
    /// of its own part -- and none is ever dropped before it is asked for. Written `up_front`,
    /// every role row precedes every part, so all but the last few are gone by the time the parts
    /// that carry them are read.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_keeps_the_roles_of_its_newest_messages_only(
        #[values(false, true)] up_front: bool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        // alternated, so that a role read back from the wrong message shows
        let messages: Vec<(String, &str)> = (0..Infos::CAP * 3)
            .map(|i| (format!("msg_{i:02}"), ["user", "assistant"][i % 2]))
            .collect();
        for (i, (message, role)) in messages.iter().enumerate() {
            role_row(&db, &format!("r{i:02}"), "ses_1", message, role).await;
            message_row(&db, message, "ses_1", role).await;
            if !up_front {
                text_row(&db, &format!("p{i:02}"), "ses_1", &format!("prt_{i:02}"), message, "hi")
                    .await;
            }
        }
        if up_front {
            for (i, (message, _)) in messages.iter().enumerate() {
                text_row(&db, &format!("p{i:02}"), "ses_1", &format!("prt_{i:02}"), message, "hi")
                    .await;
            }
        }

        let mut stream = watch(&path);
        let session = offered(&mut stream).await;
        let reader = Arc::clone(&session.reader);
        let mut parts = session.messages().boxed();
        let read = next_n(&mut parts, messages.len()).await;
        // the reader is locked for as long as its stream lives
        drop(parts);

        let roles: Vec<Role> = read.into_iter().map(|part| part.unwrap().role()).collect();
        let written: Vec<Role> =
            messages.iter().map(|(_, role)| OpencodeMessage::role_of(role)).collect();
        assert_eq!(roles, written);
        let kept = reader.lock().await.infos.0.len();
        assert!(
            kept <= Infos::CAP,
            "a session that read {} messages kept {kept} of their roles",
            messages.len()
        );
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_bad_row_is_reported_and_the_tail_continues() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "user").await;
        // valid JSON of the wrong shape, and a BLOB
        insert_event(&db, "e2", "ses_1", "message.part.updated.1", "[]").await;
        execute(
            &db,
            "INSERT INTO event (id, aggregate_id, seq, type, data) VALUES ('e3', 'ses_1', 2, \
             'message.part.updated.1', X'FFFE')",
        )
        .await;
        // a NULL primary key (legal on a TEXT one) anchors as the empty string and costs the row
        // nothing: its payload is read like any other's, reported when it is bad and delivered
        // when it is whole
        execute(
            &db,
            "INSERT INTO event (id, aggregate_id, seq, type, data) VALUES (NULL, 'ses_1', 3, \
             'message.part.updated.1', '{}')",
        )
        .await;
        query::<sqlx::Sqlite>(
            "INSERT INTO event (id, aggregate_id, seq, type, data) VALUES (NULL, 'ses_1', 4, \
             'message.part.updated.1', ?1)",
        )
        .bind(text_event("prt_0", "msg_1", "kept"))
        .execute(&mut *db.pool().acquire().await.unwrap())
        .await
        .unwrap();
        text_row(&db, "e5", "ses_1", "prt_1", "msg_1", "ok").await;

        assert_eq!(described(&mut events(&path, ReplayBehavior::All), 5).await, [
            "ses_1 json error",
            "ses_1 json error",
            "ses_1 json error",
            "ses_1 User kept",
            "ses_1 User ok"
        ]);
    }

    /// An `aggregate_id` no `String` round-trips.
    const NOT_UTF8: &[u8] = &[0xFF, 0xFE];

    #[rstest]
    #[case::a_blob(
        "INSERT INTO event (id, aggregate_id, seq, type, data) VALUES (?1, ?2, ?3, ?4, ?5)"
    )]
    #[case::text_sqlite_never_checked(
        "INSERT INTO event (id, aggregate_id, seq, type, data) VALUES (?1, CAST(?2 AS TEXT), ?3, \
         ?4, ?5)"
    )]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_whose_id_is_not_utf8_reads_its_own_rows(#[case] insert: &'static str) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_ok", "msg_1", "user").await;
        for (seq, kind, data) in [
            (0, "message.updated.1", role_event("msg_2", "assistant")),
            (1, "message.part.updated.1", text_event("prt_1", "msg_2", "one")),
            (2, "message.part.updated.1", text_event("prt_2", "msg_2", "two")),
        ] {
            query::<sqlx::Sqlite>(insert)
                .bind(format!("b{seq}"))
                .bind(NOT_UTF8)
                .bind(i64::from(seq))
                .bind(kind)
                .bind(data)
                .execute(&mut *db.pool().acquire().await.unwrap())
                .await
                .unwrap();
        }
        text_row(&db, "e2", "ses_ok", "prt_ok", "msg_1", "fine").await;

        let mut got = described(&mut events(&path, ReplayBehavior::All), 3).await;
        got.sort();
        let lossy = String::from_utf8_lossy(NOT_UTF8);
        let mut expected = vec![
            format!("{lossy} Assistant one"),
            format!("{lossy} Assistant two"),
            "ses_ok User fine".to_owned(),
        ];
        expected.sort();
        assert_eq!(got, expected);
    }

    /// opencode 1.18.32 keeps a shell tool's last 30,000 code units of output; with an emoji at
    /// the cut its payload holds a lone low surrogate, both in the running tool's rows and in the
    /// finished one.
    #[rstest]
    #[tokio::test]
    async fn a_part_with_a_lone_surrogate_is_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "evt_1", "ses_a", "msg_a", "assistant").await;
        let tool = |status: &str| {
            serde_json::json!({
                "part": {
                    "id": "prt_1", "messageID": "msg_a", "type": "tool", "callID": "call_1",
                    "tool": "bash",
                    "state": {
                        "status": status, "input": {"command": "cat emoji"},
                        "output": "...LONEa", "metadata": {"output": "...LONEa"},
                    },
                },
                "time": 1_700_000_000_000i64,
            })
            .to_string()
            .replace("LONE", r"\ude00")
        };
        insert_event(&db, "evt_2", "ses_a", "message.part.updated.1", &tool("running")).await;
        insert_event(&db, "evt_3", "ses_a", "message.part.updated.1", &tool("completed")).await;

        let messages: Vec<_> = read_all(&path).await.into_iter().map(|(_, m)| m).collect();
        let contents: Vec<Content> = messages.iter().flat_map(Message::content).collect();
        assert_eq!(contents, vec![
            Content::ToolUse(ToolUse {
                id: ToolCallId::from("call_1".to_owned()),
                name: "bash".into(),
                input: serde_json::json!({"command": "cat emoji"}),
            }),
            Content::ToolResult(ToolResult {
                call: ToolCallId::from("call_1".to_owned()),
                output: Value::from("...\u{fffd}a"),
                error: false,
            }),
        ]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_backfill_delivers_every_session_and_part() {
        const SESSIONS: usize = 3_000;
        const PARTS: usize = 9;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        seed_sessions(&db, SESSIONS, PARTS).await;

        let mut stream = events(&path, ReplayBehavior::All);
        let mut seen: HashSet<String> = HashSet::new();
        let mut delivered = 0;
        while delivered < SESSIONS * PARTS {
            let event = tokio::time::timeout(Duration::from_secs(30), stream.next())
                .await
                .expect("the backfill stalled")
                .expect("the stream ended")
                .unwrap();
            seen.insert(event.session.to_string());
            delivered += 1;
        }
        assert_eq!(seen.len(), SESSIONS);
    }

    async fn load_fixture(db: &Sqlite, jsonl: &str) {
        for line in jsonl.lines().filter(|l| !l.trim().is_empty()) {
            let row: Value = serde_json::from_str(line).expect("fixture line parses");
            insert_event(
                db,
                row["id"].as_str().unwrap(),
                row["aggregate_id"].as_str().unwrap(),
                row["type"].as_str().unwrap(),
                &row["data"].to_string(),
            )
            .await;
        }
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reconstructs_two_interleaved_sessions_from_a_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        load_fixture(&db, include_str!("../../../tests/fixtures/opencode/session1.jsonl")).await;

        let mut by_session: HashMap<String, Vec<OpencodeMessage>> = HashMap::new();
        for event in next_n(&mut events(&path, ReplayBehavior::All), 6).await {
            let event = event.unwrap();
            by_session.entry(event.session.to_string()).or_default().push(event.message);
        }

        assert_eq!(
            by_session.keys().cloned().collect::<HashSet<_>>(),
            HashSet::from(["ses_A".to_owned(), "ses_B".to_owned()])
        );

        // prtA2's streaming draft and prtA3's pending and running calls are skipped
        let a = &by_session["ses_A"];
        assert_eq!(a.len(), 3, "expected prtA1, prtA2 and prtA3 once each");
        assert!(a.iter().all(|m| m.timestamp().is_some()), "a part is missing its timestamp");

        let a1 = a
            .iter()
            .find(|m| m.id() == Some(MessageId::from("prtA1".to_owned())))
            .expect("prtA1 missing");
        assert_eq!(a1.role(), Role::User);
        assert_eq!(a1.content(), vec![Content::Text("hello from A".into())]);

        let a2 = a
            .iter()
            .find(|m| m.id() == Some(MessageId::from("prtA2".to_owned())))
            .expect("prtA2 missing");
        assert_eq!(a2.role(), Role::Assistant);
        assert_eq!(a2.content(), vec![Content::Text("final answer A".into())]);

        let tool = a
            .iter()
            .find_map(|m| {
                m.content().into_iter().find_map(|c| match c {
                    Content::ToolResult(r) => Some(r),
                    _ => None,
                })
            })
            .expect("no completed tool result");
        assert_eq!(tool.output, serde_json::json!({"stdout": "listing", "exit": 0}));

        // a session.created.1 with no info and message.part.removed.1 yield nothing; the unmapped
        // version surfaces whole, dated and named by its type; the session.next prompt is the
        // user's
        let b = &by_session["ses_B"];
        let [b1, future, prompted] = b.as_slice() else {
            panic!("expected exactly three messages for ses_B, got {}", b.len());
        };
        assert_eq!(b1.role(), Role::User);
        assert_eq!(b1.content(), vec![Content::Text("hi from B".into())]);
        assert_eq!(future.role(), Role::Other("message.part.updated".to_owned()));
        assert_eq!(future.timestamp().unwrap().unix_timestamp(), 1_700_000_006);
        assert!(matches!(
            future.content().as_slice(),
            [Content::Other(v)] if v["part"]["kind"] == "future"
        ));
        assert_eq!(prompted.role(), Role::User);
        assert_eq!(prompted.id(), Some(MessageId::from("msgB2".to_owned())));
        assert_eq!(prompted.timestamp().unwrap().unix_timestamp(), 1_700_000_011);
        assert_eq!(prompted.content(), vec![Content::Text("second prompt from B".into())]);
    }

    /// What opencode's durable events capture, end to end. Payloads follow opencode's durable
    /// event schemas (`packages/schema/src/v1/session.ts`: `session.created`/`session.updated`
    /// carry the whole `SessionInfo`, `message.updated` the whole `Assistant`/`User` info,
    /// `message.part.updated` `{sessionID, part, time}`) and the order `session/prompt.ts` and
    /// `session/processor.ts` write them in.
    mod captured {
        use super::*;
        use crate::harnesstools::session::model::{StopReason, Usage};

        const SES: &str = "ses_R";

        /// Every message the event log yields, read out by a one-shot backfill.
        async fn backfill(path: &Path) -> Vec<OpencodeMessage> {
            let sessions: Vec<OpencodeSession> = OpencodeSessions::builder()
                .db(path)
                .build()
                .existing()
                .unwrap()
                .map(Result::unwrap)
                .collect()
                .await;
            let mut out = Vec::new();
            for session in sessions {
                let messages: Vec<_> = session.messages().collect().await;
                out.extend(messages.into_iter().map(Result::unwrap));
            }
            out
        }

        fn find<'a>(messages: &'a [OpencodeMessage], id: &str) -> &'a OpencodeMessage {
            messages
                .iter()
                .find(|m| m.id() == Some(MessageId::from(id.to_owned())))
                .unwrap_or_else(|| panic!("{id} was not delivered"))
        }

        fn session_info(title: &str, parent: Option<&str>) -> Value {
            let mut info = serde_json::json!({
                "id": SES, "slug": "s", "projectID": "p", "directory": "/work/proj",
                "title": title, "version": "1.0.0",
                "time": {"created": 1_700_000_000_000i64, "updated": 1_700_000_000_000i64},
            });
            if let Some(parent) = parent {
                info["parentID"] = Value::from(parent);
            }
            info
        }

        async fn session_row(db: &Sqlite, id: &str, kind: &str, info: Value) {
            let data = serde_json::json!({"sessionID": info["id"], "info": info});
            insert_event(db, id, info["id"].as_str().unwrap(), kind, &data.to_string()).await;
        }

        /// An assistant message's info as opencode's processor writes it at `step-finish`
        /// (`processor.ts`: `assistantMessage.tokens = usage.tokens; finish = reason`).
        fn assistant_info(tokens_in: u64, error: Option<Value>) -> Value {
            let mut info = serde_json::json!({
                "id": "msg_a1", "sessionID": SES, "role": "assistant",
                "time": {"created": 1_700_000_001_000i64, "completed": 1_700_000_009_000i64},
                "parentID": "msg_u1", "modelID": "claude-sonnet-4", "providerID": "anthropic",
                "mode": "build", "agent": "build",
                "path": {"cwd": "/work/proj/sub", "root": "/work/proj"},
                "cost": 0.0123,
                "tokens": {"input": tokens_in, "output": 250, "reasoning": 40,
                           "cache": {"read": 900, "write": 30}},
                "finish": "stop",
            });
            if let Some(error) = error {
                info["error"] = error;
            }
            info
        }

        async fn message_updated(db: &Sqlite, id: &str, info: &Value) {
            let data = serde_json::json!({"sessionID": info["sessionID"], "info": info});
            let session = info["sessionID"].as_str().unwrap();
            insert_event(db, id, session, "message.updated.1", &data.to_string()).await;
        }

        async fn part_row(db: &Sqlite, id: &str, part: Value) {
            let session = part["sessionID"].as_str().unwrap().to_owned();
            let data = serde_json::json!({"sessionID": session, "time": 1_700_000_008_000i64,
                                          "part": part});
            insert_event(db, id, &session, "message.part.updated.1", &data.to_string()).await;
        }

        fn step_finish(session: &str, id: &str, message: &str, input: u64) -> Value {
            serde_json::json!({
                "id": id, "sessionID": session, "messageID": message, "type": "step-finish",
                "reason": "stop", "cost": 0.0123,
                "tokens": {"input": input, "output": 250, "reasoning": 40,
                           "cache": {"read": 900, "write": 30}}})
        }

        /// One assistant turn as opencode writes it: the session, the user's prompt, the
        /// assistant message (zero tokens when created, filled in at step-finish), a finished
        /// text part, a step-finish part, and the message again once completed.
        async fn seed_turn(db: &Sqlite) {
            session_row(db, "evt_00", "session.created.1", session_info("New session", None)).await;
            let user = serde_json::json!({
                "id": "msg_u1", "sessionID": SES, "role": "user",
                "time": {"created": 1_700_000_000_500i64}, "agent": "build",
                "model": {"providerID": "anthropic", "modelID": "claude-sonnet-4"}});
            message_updated(db, "evt_01", &user).await;
            text_row(db, "evt_02", SES, "prt_u1", "msg_u1", "hello").await;
            message_updated(db, "evt_03", &assistant_info(0, None)).await;
            let text = serde_json::json!({"sessionID": SES, "time": 1_700_000_005_000i64, "part": {
                "id": "prt_a1", "sessionID": SES, "messageID": "msg_a1", "type": "text",
                "text": "hi there",
                "time": {"start": 1_700_000_002_000i64, "end": 1_700_000_005_000i64}}});
            insert_event(db, "evt_04", SES, "message.part.updated.1", &text.to_string()).await;
            part_row(db, "evt_05", step_finish(SES, "prt_a2", "msg_a1", 1200)).await;
            message_updated(db, "evt_06", &assistant_info(1200, None)).await;
        }

        /// A one-shot read whose store fails partway ends in an error, not in a stream that
        /// passes for the whole session.
        #[rstest]
        #[tokio::test]
        async fn a_read_that_fails_partway_says_so() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = event_db(&path).await;
            seed_turn(&db).await;
            let sessions: Vec<OpencodeSession> = OpencodeSessions::builder()
                .db(&path)
                .build()
                .existing()
                .unwrap()
                .map(Result::unwrap)
                .collect()
                .await;
            let [session] = <[_; 1]>::try_from(sessions).unwrap();
            query::<sqlx::Sqlite>("DROP TABLE event")
                .execute(&mut *db.pool().acquire().await.unwrap())
                .await
                .unwrap();

            let read: Vec<_> = session.read().collect().await;
            assert!(
                matches!(read.last(), Some(Err(MessageError::Incomplete))),
                "a failed read ended the stream silently: {read:?}"
            );
        }

        async fn seeded() -> (tempfile::TempDir, Vec<OpencodeMessage>) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = event_db(&path).await;
            seed_turn(&db).await;
            let messages = backfill(&path).await;
            (dir, messages)
        }

        /// The usage opencode reports on the message and again on its step-finish part is
        /// captured once, from the step. opencode's `output` leaves the reasoning tokens out
        /// (`Session.getUsage`: `output: outputTokens - reasoningTokens`), and they are billed
        /// as output (ccusage and tokscale both add them back), so 250 + 40.
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_turns_usage_is_captured_once_from_its_step() {
            let (_dir, messages) = seeded().await;
            let usage: Vec<(Option<MessageId>, Usage)> =
                messages.iter().filter_map(|m| Some((m.id(), m.usage()?))).collect();
            assert_eq!(usage, vec![(Some(MessageId::from("prt_a2".to_owned())), Usage {
                input: Some(1200),
                output: Some(290),
                cache_read: Some(900),
                cache_write: Some(30),
                reasoning: Some(40),
            })]);
            let step = find(&messages, "prt_a2");
            assert!(step.content().is_empty());
            assert_eq!(step.stop_reason(), Some(StopReason::EndTurn));
            assert_eq!(
                step.turn_id().as_deref(),
                Some("1700000001000:anthropic/claude-sonnet-4#1200/250/40/900/30")
            );
            assert_eq!(step.parent_id(), Some(MessageId::from("msg_u1".to_owned())));
        }

        /// Every row of an assistant message says which model wrote it, where, in which call,
        /// and which user message it answers; a user's row says none of it.
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn an_assistant_part_carries_its_messages_context() {
            let (_dir, messages) = seeded().await;
            let reply = find(&messages, "prt_a1");
            assert_eq!(reply.role(), Role::Assistant);
            assert_eq!(reply.model().as_deref(), Some("claude-sonnet-4"));
            assert_eq!(reply.cwd(), Some(PathBuf::from("/work/proj/sub")));
            assert_eq!(reply.parent_id(), Some(MessageId::from("msg_u1".to_owned())));
            assert_eq!(reply.turn_id().as_deref(), Some("1700000001000:anthropic/claude-sonnet-4"));
            assert_eq!(reply.stop_reason(), None);
            assert_eq!(reply.usage(), None);

            let prompt = find(&messages, "prt_u1");
            assert_eq!(prompt.role(), Role::User);
            assert_eq!(
                (prompt.model(), prompt.cwd(), prompt.parent_id(), prompt.turn_id()),
                (None, None, None, None)
            );
        }

        /// A message's info, read back from opencode's `message` projection when its
        /// `message.updated.1` row is not in the part of the log a session reads, says the same.
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_parts_context_is_looked_up_when_its_message_row_is_not_to_hand() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = event_db(&path).await;
            let mut info = assistant_info(1200, None);
            let data = {
                let object = info.as_object_mut().unwrap();
                object.remove("id");
                object.remove("sessionID");
                info.to_string()
            };
            query::<sqlx::Sqlite>(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES \
                 ('msg_a1', ?1, 0, 0, ?2)",
            )
            .bind(SES)
            .bind(data)
            .execute(&mut *db.pool().acquire().await.unwrap())
            .await
            .unwrap();
            part_row(&db, "evt_00", step_finish(SES, "prt_a2", "msg_a1", 1200)).await;

            let messages = backfill(&path).await;
            let step = find(&messages, "prt_a2");
            assert_eq!(step.role(), Role::Assistant);
            assert_eq!(step.model().as_deref(), Some("claude-sonnet-4"));
            assert_eq!(
                step.turn_id().as_deref(),
                Some("1700000001000:anthropic/claude-sonnet-4#1200/250/40/900/30")
            );
        }

        /// `Session.fork` copies every message and part into the new session under fresh ids
        /// and with everything else -- times, model, tokens -- as it was: the copy of a step is
        /// the same model call as its original, and names the same turn so its usage is counted
        /// once.
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_forked_copy_of_a_step_is_the_same_turn_as_its_original() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = event_db(&path).await;
            seed_turn(&db).await;
            let mut fork = session_info("New session (fork #1)", None);
            fork["id"] = Value::from("ses_F");
            session_row(&db, "evt_f0", "session.created.1", fork).await;
            let user = serde_json::json!({
                "id": "msg_fu1", "sessionID": "ses_F", "role": "user",
                "time": {"created": 1_700_000_000_500i64}, "agent": "build",
                "model": {"providerID": "anthropic", "modelID": "claude-sonnet-4"}});
            message_updated(&db, "evt_f1", &user).await;
            text_row(&db, "evt_f2", "ses_F", "prt_fu1", "msg_fu1", "hello").await;
            let mut copy = assistant_info(1200, None);
            copy["id"] = Value::from("msg_fa1");
            copy["sessionID"] = Value::from("ses_F");
            copy["parentID"] = Value::from("msg_fu1");
            message_updated(&db, "evt_f3", &copy).await;
            part_row(&db, "evt_f4", step_finish("ses_F", "prt_fa2", "msg_fa1", 1200)).await;

            let messages = backfill(&path).await;
            let (original, copy) = (find(&messages, "prt_a2"), find(&messages, "prt_fa2"));
            assert!(original.usage().is_some());
            assert_eq!(copy.usage(), original.usage());
            assert_eq!(copy.turn_id(), original.turn_id());
            assert_eq!(copy.parent_id(), Some(MessageId::from("msg_fu1".to_owned())));
        }

        /// A message of several steps -- a retried stream, an older opencode that ran many steps
        /// per message -- reports each step's usage on its own step-finish, and each is a model
        /// call of its own.
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn each_step_of_a_message_is_a_turn_of_its_own() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = event_db(&path).await;
            message_updated(&db, "evt_00", &assistant_info(0, None)).await;
            part_row(&db, "evt_01", step_finish(SES, "prt_s1", "msg_a1", 1000)).await;
            part_row(&db, "evt_02", step_finish(SES, "prt_s2", "msg_a1", 2000)).await;

            let messages = backfill(&path).await;
            let turns: Vec<String> = messages.iter().filter_map(Message::turn_id).collect();
            assert_eq!(turns, [
                "1700000001000:anthropic/claude-sonnet-4#1000/250/40/900/30",
                "1700000001000:anthropic/claude-sonnet-4#2000/250/40/900/30",
            ]);
            let inputs: Vec<Option<u64>> =
                messages.iter().filter_map(|m| Some(m.usage()?.input)).collect();
            assert_eq!(inputs, [Some(1000), Some(2000)]);
        }

        /// A model call that fails or is aborted exists only as its message's info with
        /// `error`, if it fails before a single part is written: that is a row of its own.
        #[rstest]
        #[case::aborted(
            serde_json::json!({"name": "MessageAbortedError", "data": {"message": "aborted"}}),
            StopReason::Aborted,
            "aborted"
        )]
        #[case::api(
            serde_json::json!({
                "name": "APIError", "data": {"message": "overloaded", "isRetryable": true}
            }),
            StopReason::Error,
            "overloaded"
        )]
        #[case::output_length(
            serde_json::json!({"name": "MessageOutputLengthError", "data": {}}),
            StopReason::MaxTokens,
            "MessageOutputLengthError"
        )]
        #[case::content_filter(
            serde_json::json!({"name": "ContentFilterError", "data": {
                "message": "The response was blocked by the provider's content filter"}}),
            StopReason::Refusal,
            "The response was blocked by the provider's content filter"
        )]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_failed_assistant_turn_is_captured(
            #[case] error: Value,
            #[case] reason: StopReason,
            #[case] text: &str,
        ) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = event_db(&path).await;
            role_row(&db, "evt_00", SES, "msg_u1", "user").await;
            text_row(&db, "evt_01", SES, "prt_u1", "msg_u1", "hello").await;
            message_updated(&db, "evt_02", &assistant_info(0, None)).await;
            message_updated(&db, "evt_03", &assistant_info(0, Some(error))).await;

            let messages = backfill(&path).await;
            assert_eq!(messages.len(), 2, "the prompt and the failure");
            let failed = find(&messages, "msg_a1");
            assert_eq!(failed.role(), Role::Assistant);
            assert_eq!(failed.content(), vec![Content::Error(text.to_owned())]);
            assert_eq!(failed.stop_reason(), Some(reason));
            assert_eq!(failed.timestamp().unwrap().unix_timestamp(), 1_700_000_009);
            assert_eq!(failed.model().as_deref(), Some("claude-sonnet-4"));
            assert_eq!(failed.parent_id(), Some(MessageId::from("msg_u1".to_owned())));
            assert_eq!(
                failed.turn_id().as_deref(),
                Some("1700000001000:anthropic/claude-sonnet-4")
            );
            assert_eq!(failed.usage(), None);
        }

        /// opencode writes `session.updated` on every prompt (`touch`) and whenever anything of
        /// the session changes; its title is delivered when it is first seen and each time it
        /// changes.
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn the_session_title_is_captured_as_it_changes() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = event_db(&path).await;
            seed_turn(&db).await;
            let touched = session_info("New session", None);
            session_row(&db, "evt_07", "session.updated.1", touched).await;
            // ensureTitle -> setTitle
            let titled = session_info("Fix the flaky test", None);
            session_row(&db, "evt_08", "session.updated.1", titled.clone()).await;
            session_row(&db, "evt_09", "session.updated.1", titled).await;

            let messages = backfill(&path).await;
            let titles: Vec<(Option<MessageId>, String)> =
                messages.iter().filter_map(|m| Some((m.id(), m.title()?.text?))).collect();
            assert_eq!(titles, [
                (Some(MessageId::from("ses_R:title:New session".to_owned())), "New session".into()),
                (
                    Some(MessageId::from("ses_R:title:Fix the flaky test".to_owned())),
                    "Fix the flaky test".into()
                ),
            ]);
            let session = find(&messages, "ses_R:title:New session");
            assert_eq!(session.role(), Role::System);
            assert!(session.content().is_empty());
            assert_eq!(session.cwd(), Some(PathBuf::from("/work/proj")));
        }

        /// A task-tool subagent session is created with `parentID` (`tool/task.ts`).
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_child_session_names_its_parent() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = event_db(&path).await;
            let created = session_info("sub", Some("ses_parent"));
            session_row(&db, "evt_00", "session.created.1", created).await;
            role_row(&db, "evt_01", SES, "msg_u1", "user").await;
            text_row(&db, "evt_02", SES, "prt_u1", "msg_u1", "do the subtask").await;

            let messages = backfill(&path).await;
            let parents: Vec<SessionId> =
                messages.iter().filter_map(Message::parent_session).collect();
            assert_eq!(parents, [SessionId::from("ses_parent".to_owned())]);
        }

        /// `@file` in a prompt makes opencode run the Read tool itself and store its output as a
        /// `synthetic` text part of the *user* message (`session/prompt.ts`); ACP content meant
        /// for the user alone is stored `ignored` (`acp/content.ts`). Neither was typed.
        #[rstest]
        #[case::synthetic("synthetic")]
        #[case::ignored("ignored")]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn text_the_harness_injected_is_the_systems(#[case] flag: &str) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = event_db(&path).await;
            role_row(&db, "evt_00", SES, "msg_u1", "user").await;
            text_row(&db, "evt_01", SES, "prt_u1", "msg_u1", "look at @config.env").await;
            let mut injected = serde_json::json!({
                "id": "prt_u2", "sessionID": SES, "messageID": "msg_u1", "type": "text",
                "text": "<file>\n00001| DB_HOST=prod.internal\n</file>"});
            injected[flag] = Value::Bool(true);
            part_row(&db, "evt_02", injected).await;

            let messages = backfill(&path).await;
            assert_eq!(find(&messages, "prt_u1").role(), Role::User);
            let injected = find(&messages, "prt_u2");
            assert_eq!(injected.role(), Role::System);
            assert!(matches!(injected.content().as_slice(), [Content::Text(_)]));
        }

        /// A compaction writes its summary as the text of an assistant message marked `summary`
        /// (`session/compaction.ts`).
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_compaction_summary_is_a_summary() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = event_db(&path).await;
            let mut info = assistant_info(0, None);
            info["summary"] = Value::Bool(true);
            info["mode"] = Value::from("compaction");
            message_updated(&db, "evt_00", &info).await;
            text_row(&db, "evt_01", SES, "prt_a1", "msg_a1", "we fixed the test").await;

            let messages = backfill(&path).await;
            assert_eq!(find(&messages, "prt_a1").content(), vec![Content::Summary(
                "we fixed the test".into()
            )]);
        }

        /// `/compact` and a context overflow ask for a compaction with a user message opencode
        /// writes itself, whose only part is a `compaction` marker (`SessionCompaction.create`).
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_compaction_request_is_the_systems() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = event_db(&path).await;
            role_row(&db, "evt_00", SES, "msg_u1", "user").await;
            // as opencode 1.18.32 wrote it for `POST /session/:id/summarize`
            let marker = serde_json::json!({
                "id": "prt_0d14b98e70014t9fK6vIkrdBD4", "sessionID": SES, "messageID": "msg_u1",
                "type": "compaction", "auto": false});
            part_row(&db, "evt_01", marker).await;

            let messages = backfill(&path).await;
            assert_eq!(find(&messages, "prt_0d14b98e70014t9fK6vIkrdBD4").role(), Role::System);
        }

        /// `/undo` (`session/revert.ts`) removes messages and parts with durable
        /// `message.removed.1` / `message.part.removed.1` rows, written when the next prompt
        /// commits the revert. They are left unread, and what was captured stands: the reverted
        /// turns did happen and did cost their tokens; captured rows are immutable, so nothing
        /// downstream could retract them; and no content says "retracted", so a marker would be
        /// made-up text, one per removed message and part, dated at the next prompt rather than
        /// the undo.
        #[rstest]
        #[case::part(
            "message.part.removed.1",
            serde_json::json!({"sessionID": SES, "messageID": "msg_u1", "partID": "prt_u1"})
        )]
        #[case::message(
            "message.removed.1",
            serde_json::json!({"sessionID": SES, "messageID": "msg_u1"})
        )]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_revert_leaves_what_was_captured_as_it_stands(
            #[case] kind: &str,
            #[case] data: Value,
        ) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = event_db(&path).await;
            role_row(&db, "evt_00", SES, "msg_u1", "user").await;
            text_row(&db, "evt_01", SES, "prt_u1", "msg_u1", "oops").await;
            insert_event(&db, "evt_02", SES, kind, &data.to_string()).await;

            let messages = backfill(&path).await;
            let delivered: Vec<(Option<MessageId>, Vec<Content>)> =
                messages.iter().map(|m| (m.id(), m.content())).collect();
            assert_eq!(delivered, [(Some(MessageId::from("prt_u1".to_owned())), vec![
                Content::Text("oops".into())
            ])]);
        }
    }

    /// Every message a one-shot backfill yields: each session `existing` lists, `read` out.
    async fn read_all(path: &Path) -> Vec<(String, OpencodeMessage)> {
        let sessions: Vec<OpencodeSession> = OpencodeSessions::builder()
            .db(path)
            .build()
            .existing()
            .unwrap()
            .map(Result::unwrap)
            .collect()
            .await;
        let mut out = Vec::new();
        for session in sessions {
            let id = session.id().to_string();
            let messages: Vec<_> = session.read().collect().await;
            out.extend(messages.into_iter().map(|m| (id.clone(), m.unwrap())));
        }
        out
    }

    fn ids(messages: &[(String, OpencodeMessage)]) -> Vec<String> {
        messages.iter().filter_map(|(_, m)| m.id()).map(String::from).collect()
    }

    /// Sessions older than opencode's event log, read from its projection. The fixture is a
    /// session opencode 1.18.32 wrote, whose event log was then emptied as its 1.17.10 migration
    /// empties it, and which opencode continued afterwards (`opencode run --session`): its
    /// `session`, `message` and `part` rows and the log's rows since, paths redacted.
    mod projected {
        use super::*;
        use crate::harnesstools::session::model::{StopReason, Usage};

        const FIXTURE: &str = include_str!("../../../tests/fixtures/opencode/projection.json");

        /// opencode's `session` and `part` projections, as far as this module reads them, beside
        /// the event log and `message` projection [`event_db`] creates.
        async fn projection_db(path: &Path) -> Sqlite {
            let db = event_db(path).await;
            execute(
                &db,
                "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT \
                 NULL, title TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER \
                 NOT NULL)",
            )
            .await;
            execute(
                &db,
                "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id \
                 TEXT NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, \
                 data TEXT NOT NULL)",
            )
            .await;
            db
        }

        async fn session_projection(db: &Sqlite, row: &Value) {
            query::<sqlx::Sqlite>(
                "INSERT INTO session (id, parent_id, directory, title, time_created, \
                 time_updated) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(row["id"].as_str())
            .bind(row["parent_id"].as_str())
            .bind(row["directory"].as_str())
            .bind(row["title"].as_str())
            .bind(row["time_created"].as_i64())
            .bind(row["time_updated"].as_i64())
            .execute(&mut *db.pool().acquire().await.unwrap())
            .await
            .unwrap();
        }

        async fn message_projection(db: &Sqlite, session: &str, row: &Value) {
            query::<sqlx::Sqlite>(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES \
                 (?1, ?2, ?3, ?3, ?4)",
            )
            .bind(row["id"].as_str())
            .bind(session)
            .bind(row["time_created"].as_i64())
            .bind(row["data"].to_string())
            .execute(&mut *db.pool().acquire().await.unwrap())
            .await
            .unwrap();
        }

        async fn part_projection(db: &Sqlite, session: &str, row: &Value) {
            query::<sqlx::Sqlite>(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) \
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            )
            .bind(row["id"].as_str())
            .bind(row["message_id"].as_str())
            .bind(session)
            .bind(row["time_created"].as_i64())
            .bind(row["data"].to_string())
            .execute(&mut *db.pool().acquire().await.unwrap())
            .await
            .unwrap();
        }

        /// The fixture's projection rows, and its log rows unless `logged` is false.
        async fn load(db: &Sqlite, logged: bool) {
            let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
            let session = fixture["session"]["id"].as_str().unwrap();
            session_projection(db, &fixture["session"]).await;
            for row in fixture["messages"].as_array().unwrap() {
                message_projection(db, session, row).await;
            }
            for row in fixture["parts"].as_array().unwrap() {
                part_projection(db, session, row).await;
            }
            if logged {
                for row in fixture["events"].as_array().unwrap() {
                    let (id, kind) = (row["id"].as_str().unwrap(), row["type"].as_str().unwrap());
                    insert_event(db, id, session, kind, &row["data"].to_string()).await;
                }
            }
        }

        fn find<'a>(messages: &'a [(String, OpencodeMessage)], id: &str) -> &'a OpencodeMessage {
            messages
                .iter()
                .map(|(_, m)| m)
                .find(|m| m.id() == Some(MessageId::from(id.to_owned())))
                .unwrap_or_else(|| panic!("{id} was not delivered"))
        }

        /// What the log has no `message.updated` row for is read from the projection, ahead of
        /// the log and just as the log would have delivered it; what the log does have is read
        /// from the log alone; the title is the session's as it stands, once.
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_session_older_than_its_log_is_read_from_the_projection() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = projection_db(&path).await;
            load(&db, true).await;

            let messages = read_all(&path).await;
            assert_eq!(ids(&messages), [
                // from the projection
                "prt_0d148d4bf001JaxA9TwaMgU5vE",
                "prt_0d148db28001TznIjdIVqXAvJ3",
                "prt_0d148db2c001S3pTr5s7fXT3RP",
                "prt_0d148db35001Zcq9788cC4lx83",
                "prt_0d149b05c0015TIlARpqWlDXNK",
                "msg_0d149b2d5001gkfmc9bUMDXLJ5",
                // from the log, which opencode continued after the wipe
                "prt_0d154c3f0001VPSmfCQt4r6LWN",
                "ses_W:title:Mock title number 1",
                "prt_0d154c996001Lr5h4i3jpsGN60",
                "prt_0d154c99a001dtKmLh8SYe2pad",
                "prt_0d154c9a5001gvwUYJAY42itXy",
            ]);

            let title = find(&messages, "ses_W:title:Mock title number 1");
            assert_eq!(title.cwd(), Some(PathBuf::from("/work/proj")));
            assert_eq!(title.timestamp().unwrap().unix_timestamp(), 1_790_218_388);

            let prompt = find(&messages, "prt_0d148d4bf001JaxA9TwaMgU5vE");
            assert_eq!(prompt.role(), Role::User);
            assert_eq!(prompt.content(), vec![Content::Text("\"hello there\"".into())]);
            assert_eq!(prompt.timestamp().unwrap().unix_timestamp(), 1_790_217_606);

            let reply = find(&messages, "prt_0d148db2c001S3pTr5s7fXT3RP");
            assert_eq!(reply.role(), Role::Assistant);
            assert_eq!(reply.model().as_deref(), Some("mock-model"));
            assert_eq!(reply.cwd(), Some(PathBuf::from("/work/proj")));
            assert_eq!(
                reply.parent_id(),
                Some(MessageId::from("msg_0d148d4b9001WQdGo06CVdqA3u".to_owned()))
            );

            let step = find(&messages, "prt_0d148db35001Zcq9788cC4lx83");
            assert_eq!(
                step.usage(),
                Some(Usage {
                    input: Some(802),
                    output: Some(52),
                    cache_read: Some(200),
                    cache_write: Some(0),
                    reasoning: Some(10),
                })
            );
            assert_eq!(step.stop_reason(), Some(StopReason::EndTurn));
            assert_eq!(
                step.turn_id().as_deref(),
                Some("1790217606979:mock/mock-model#802/42/10/200/0")
            );

            let failed = find(&messages, "msg_0d149b2d5001gkfmc9bUMDXLJ5");
            assert_eq!(failed.content(), vec![Content::Error("mock bad request".into())]);
            assert_eq!(failed.stop_reason(), Some(StopReason::Error));
        }

        /// The projection holds a session's title as it stands, the log each title it has had
        /// since the wipe: the log's say which came last, where the projection's, delivered ahead
        /// of them, would make an older title the session's.
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_title_the_log_changed_since_the_wipe_is_the_last() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = projection_db(&path).await;
            load(&db, true).await;
            execute(&db, "UPDATE session SET title = 'Renamed'").await;
            let renamed = serde_json::json!({"sessionID": "ses_W", "info": {
                "id": "ses_W", "title": "Renamed", "directory": "/work/proj",
                "time": {"created": 1_790_217_606_267i64, "updated": 1_790_218_600_000i64}}});
            insert_event(&db, "evt_renamed", "ses_W", "session.updated.1", &renamed.to_string())
                .await;

            let titles: Vec<String> =
                read_all(&path).await.iter().filter_map(|(_, m)| m.title()?.text).collect();
            assert_eq!(titles, ["Mock title number 1", "Renamed"]);
        }

        /// A session whose log was emptied and never written again is still a session: it is
        /// listed, and read from the projection alone, a subagent's naming its parent.
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_session_only_the_projection_holds_is_listed_and_read() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = projection_db(&path).await;
            load(&db, false).await;
            session_projection(
                &db,
                &serde_json::json!({
                    "id": "ses_child", "parent_id": "ses_W", "directory": "/work/proj",
                    "title": "mock subtask (@general subagent)",
                    "time_created": 1_790_217_674_726i64, "time_updated": 1_790_217_674_898i64,
                }),
            )
            .await;

            let messages = read_all(&path).await;
            let sessions: Vec<&str> = messages.iter().map(|(s, _)| s.as_str()).collect();
            assert_eq!(sessions.first(), Some(&"ses_W"));
            assert_eq!(sessions.last(), Some(&"ses_child"));
            assert_eq!(
                messages.len(),
                12,
                "ses_W's title, nine parts and failure; ses_child's title"
            );
            let (_, child) = messages.last().unwrap();
            assert_eq!(child.parent_session(), Some(SessionId::from("ses_W".to_owned())));
            assert_eq!(
                child.title().and_then(|t| t.text).as_deref(),
                Some("mock subtask (@general subagent)")
            );
        }

        /// A session whose log holds its `session.created` row is read from the log alone, even
        /// where the projection holds the same messages.
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_session_whose_log_holds_its_start_is_read_from_the_log_alone() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = projection_db(&path).await;
            let created = serde_json::json!({"sessionID": "ses_W", "info": {
                "id": "ses_W", "title": "New session", "directory": "/work/proj",
                "time": {"created": 1_790_217_606_267i64, "updated": 1_790_217_606_267i64}}});
            insert_event(&db, "evt_created", "ses_W", "session.created.1", &created.to_string())
                .await;
            load(&db, true).await;

            let messages = read_all(&path).await;
            let delivered = ids(&messages);
            assert_eq!(delivered.first().map(String::as_str), Some("ses_W:title:New session"));
            assert!(
                !delivered.contains(&"prt_0d148d4bf001JaxA9TwaMgU5vE".to_owned()),
                "the projection was read for a session the log holds from its start"
            );
        }

        /// A part opencode never finished writing -- a stream a retry abandoned, a process that
        /// died mid-answer -- is left in the projection as a draft, and is no more delivered from
        /// there than from the log.
        #[rstest]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_projected_draft_is_not_delivered() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("opencode.db");
            let db = projection_db(&path).await;
            load(&db, false).await;
            part_projection(&db, "ses_W", &serde_json::json!({
                "id": "prt_0d148db2d001abandonedDraft1", "message_id": "msg_0d148d743001qhQZH5hIpZVrlb",
                "time_created": 1_790_217_607_981i64,
                "data": {"type": "text", "text": "half an ans", "time": {"start": 1_790_217_607_981i64}},
            }))
            .await;

            let messages = read_all(&path).await;
            assert!(!ids(&messages).contains(&"prt_0d148db2d001abandonedDraft1".to_owned()));
            assert_eq!(messages.len(), 11, "the title, nine parts and a failure");
        }
    }

    /// The experimental event system's rows (`session.next.*`), as opencode 1.18.32 wrote them
    /// for two prompts to `POST /api/session/:id/prompt`: one answered with reasoning, a tool
    /// call and a second model call, one the provider refused with HTTP 400.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_session_of_the_experimental_event_system_is_read() {
        use crate::harnesstools::session::model::{StopReason, Usage};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        load_fixture(&db, include_str!("../../../tests/fixtures/opencode/session_next.jsonl"))
            .await;

        let messages: Vec<OpencodeMessage> =
            read_all(&path).await.into_iter().map(|(_, m)| m).collect();
        let first = "msg_0d14e8cc0001K0TEI9JoV4l4pr";
        let second = "msg_0d14e8d380015hjKD5i0Ef3Mxa";
        let failed = "msg_0d1598e09001440ss0zBu2x76T";
        let summary: Vec<(String, Role, Option<String>)> = messages
            .iter()
            .map(|m| (m.id().map(String::from).unwrap_or_default(), m.role(), m.turn_id()))
            .collect();
        let turn = |id: &str| Some(id.to_owned());
        assert_eq!(summary, [
            ("msg_0d14e8c60001TZVSAQxcC0zepK".to_owned(), Role::User, None),
            ("msg_0d14e8ca8001bv5Yyq3BwLdvdJ".to_owned(), Role::System, None),
            (format!("{first}/reasoning-0"), Role::Assistant, turn(first)),
            (format!("{first}/call_2"), Role::Assistant, turn(first)),
            (format!("{first}/text-0"), Role::Assistant, turn(first)),
            (format!("{first}/call_2/result"), Role::Assistant, turn(first)),
            (format!("{first}/step"), Role::Assistant, turn(first)),
            (format!("{second}/text-0"), Role::Assistant, turn(second)),
            (format!("{second}/step"), Role::Assistant, turn(second)),
            ("msg_0d1598db5001GmuaZBhAHyx1g7".to_owned(), Role::User, None),
            (format!("{failed}/step"), Role::Assistant, turn(failed)),
        ]);

        assert_eq!(messages[0].content(), vec![Content::Text("v2 THINK TOOL go".into())]);
        assert_eq!(messages[0].timestamp().unwrap().unix_timestamp(), 1_790_217_981);
        assert_eq!(messages[2].content(), vec![Content::Reasoning(
            "Let me think about it.".into()
        )]);
        assert_eq!(messages[2].model().as_deref(), Some("mock-model"));
        assert!(matches!(
            messages[3].content().as_slice(),
            [Content::ToolUse(u)] if u.name == "bash" && u.id.as_ref() == "call_2"
        ));
        assert!(matches!(
            messages[5].content().as_slice(),
            [Content::ToolResult(r)] if r.call.as_ref() == "call_2" && !r.error
        ));
        let usage: Vec<(Option<Usage>, Option<StopReason>)> = messages
            .iter()
            .filter(|m| m.usage().is_some())
            .map(|m| (m.usage(), m.stop_reason()))
            .collect();
        let used = |input, output| Usage {
            input: Some(input),
            output: Some(output),
            cache_read: Some(200),
            cache_write: Some(0),
            reasoning: Some(10),
        };
        assert_eq!(usage, [
            (Some(used(802, 52)), Some(StopReason::ToolUse)),
            (Some(used(803, 53)), Some(StopReason::EndTurn)),
        ]);
        let refused = messages.last().unwrap();
        assert_eq!(refused.stop_reason(), Some(StopReason::Error));
        assert!(matches!(
            refused.content().as_slice(),
            [Content::Error(e)] if e.contains("mock bad request")
        ));
    }

    #[rstest]
    #[case::stop("stop", StopReason::EndTurn)]
    #[case::length("length", StopReason::MaxTokens)]
    #[case::tool_calls("tool-calls", StopReason::ToolUse)]
    #[case::content_filter("content-filter", StopReason::Refusal)]
    #[case::error("error", StopReason::Error)]
    #[case::unknown("unknown", StopReason::Other("unknown".into()))]
    fn a_step_says_why_it_ended(#[case] reason: &str, #[case] expected: StopReason) {
        let step = part_message(serde_json::json!({
            "id": "prt_1", "type": "step-finish", "reason": reason,
            "tokens": {"input": 1, "output": 2, "reasoning": 0, "cache": {"read": 0, "write": 0}},
        }));
        assert_eq!(step.stop_reason(), Some(expected));
    }

    #[rstest]
    #[case::whole(
        serde_json::json!({"input": 10, "output": 5, "reasoning": 2, "cache": {"read": 3, "write": 1}}),
        Some(Usage { input: Some(10), output: Some(7), cache_read: Some(3), cache_write: Some(1), reasoning: Some(2) })
    )]
    #[case::fractional_and_negative(
        serde_json::json!({"input": 10.4, "output": -5, "reasoning": 2, "cache": {"read": 3}}),
        Some(Usage { input: Some(10), output: Some(2), cache_read: Some(3), cache_write: None, reasoning: Some(2) })
    )]
    #[case::no_output_at_all(
        serde_json::json!({"input": 10}),
        Some(Usage { input: Some(10), output: None, cache_read: None, cache_write: None, reasoning: None })
    )]
    #[case::no_tokens(Value::Null, None)]
    fn a_steps_usage_counts_reasoning_as_output(
        #[case] tokens: Value,
        #[case] expected: Option<Usage>,
    ) {
        let step = part_message(serde_json::json!({
            "id": "prt_1", "type": "step-finish", "reason": "stop", "tokens": tokens,
        }));
        assert_eq!(step.usage(), expected);
    }

    #[rstest]
    fn a_retry_reports_the_error_it_retries_after() {
        let retry = part_message(serde_json::json!({
            "id": "prt_1", "type": "retry", "attempt": 1, "time": {"created": 1},
            "error": {"name": "APIError", "data": {"message": "overloaded", "isRetryable": true}},
        }));
        assert_eq!(retry.content(), vec![Content::Error("overloaded".into())]);
        assert_eq!(retry.stop_reason(), None);
    }
}
