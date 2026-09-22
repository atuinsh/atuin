//! Live capture of opencode sessions from its SQLite event log.
//!
//! opencode appends every durable session event to the `event` table of one global database
//! (`opencode.db` in its data directory). [`OpencodeListener::watch`] tails that table with
//! [`SqliteObserver`] and demultiplexes rows by `aggregate_id` (the session id) into per-session
//! [`OpencodeSession`] streams.
//!
//! # Guarantees
//!
//! - **At least once, never lost.** opencode deletes a session's rows when the session is deleted
//!   and some of its migrations wipe the table, after which SQLite recycles the freed rowids. The
//!   tail notices (see [`Tailable::identity`]) and rewinds to the start of the table, and it
//!   starts over when the file is replaced, so `watch()` sees rows again. It skips a row only
//!   when it has processed one with the same `seq` and id for that session (see `Seen`); every
//!   other row is forwarded, since a duplicate is recoverable and a dropped row is not.
//! - **One bad row never stops capture.** Column values of any storage class decode (a NULL makes
//!   the row an ignored one, with a warning); malformed JSON surfaces as a [`MessageError`] item
//!   on the session's stream and the tail moves on.
//! - **Roles are never guessed.** `message.updated.1` rows record each message's role; a part
//!   whose message predates the tail is looked up in opencode's `message` projection table, and
//!   `Role::Other("unknown")` is the last resort.
//! - **Bounded memory.** At most `INFLIGHT_CAP` rows are in flight across all sessions and
//!   `DEMUX_CAP` per session. `watch()` parks until consumers drain, so it must be polled
//!   concurrently with the sessions' `messages()` streams (as [`Listener::events`] does); a
//!   session that is held but never drained parks the whole demux.
//! - **A dropped session handle loses nothing.** Its undelivered rows stay queued, still counted
//!   by both caps, and the session is offered again under the same id: at once while rows wait
//!   for it, otherwise with its next row. Every offer answers progress, a row forwarded or one
//!   taken from the handle just dropped, so a re-offer dropped unread waits for the session's
//!   next row rather than spinning `watch()`. A `watch()` consumer therefore takes every offer,
//!   re-offers included, and drains it (to ignore a session, drain it and discard); one that
//!   drops re-offers unread leaves their rows holding the caps, and once those are full nothing
//!   can be forwarded.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures::{Stream, StreamExt};
use serde::de::Error as _;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteRow};
use sqlx::{Connection, Row, Sqlite};
use time::OffsetDateTime;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use typed_builder::TypedBuilder;

use crate::db::query_scalar;
use crate::db::sqlite::observe::{
    Appended, ObserveConfig, Replay, SqliteObserver, TableSchema, Tailable,
};
use crate::harnesstools::opencode::Opencode;
use crate::harnesstools::session::model::{
    Content, MessageId, Role, ToolCallId, ToolResult, ToolUse,
};
use crate::harnesstools::session::{
    Listener, Message, MessageError, Observable, RuntimeError, Session, SessionId, Sessions,
    WatchError,
};
use crate::utils::{env_nonempty, home_dir};

#[derive(Debug, Clone, TypedBuilder)]
pub struct OpencodeSessions {
    #[builder(default, setter(strip_option, into))]
    db: Option<PathBuf>,
    #[builder(default = Replay::All)]
    replay: Replay,
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

    fn sessions(&self) -> OpencodeSessions {
        OpencodeSessions::builder().build()
    }
}

#[derive(Debug, Clone)]
pub struct OpencodeListener {
    db: PathBuf,
    replay: Replay,
}

/// Rows queued per session before `watch()` parks for that session's consumer.
const DEMUX_CAP: usize = 64;
/// Rows queued across all sessions before `watch()` parks for any consumer.
const INFLIGHT_CAP: usize = 1024;

impl Listener for OpencodeListener {
    type Session = OpencodeSession;

    /// Tails the event log and offers each session on its first forwarded row.
    ///
    /// Poll it concurrently with the offered sessions' [`messages`](Session::messages) streams
    /// (as [`Listener::events`] does): once a session's queue or the global in-flight cap is full
    /// this stream parks until a consumer drains. Dropping a session handle loses nothing: its
    /// rows stay queued and the session is offered again, under the same id, as soon as a row
    /// waits for it; a re-offer dropped unread is offered again with the session's next row. Take
    /// re-offers as well, and drain them: until then their rows hold the caps.
    fn watch(self) -> impl Stream<Item = Result<OpencodeSession, WatchError>> + Send + 'static {
        let db = self.db;
        let replay = self.replay;
        async_stream::try_stream! {
            let mut projection = Projection::new(db.clone());
            projection.require_event_table().await?;
            let observe = |source| WatchError::Observe { db: db.clone(), source };
            let mut events = SqliteObserver::new(&db)
                .append::<EventRow>(ObserveConfig::builder().replay(replay).build())
                .await
                .map_err(observe)?;
            let inflight = Arc::new(Semaphore::new(INFLIGHT_CAP));
            let (gone_tx, mut gone) = mpsc::unbounded_channel::<String>();
            let mut sessions: HashMap<String, Live> = HashMap::new();
            loop {
                // a dropped handle is answered wherever this stream waits, so a session whose
                // queued rows hold the caps is offered again even when no row can be forwarded
                let next = tokio::select! {
                    biased;
                    Some(id) = gone.recv() => {
                        if let Some(live) = sessions.get_mut(&id)
                            && let Some(session) = live.dropped(&id)
                        {
                            yield session;
                        }
                        continue;
                    }
                    next = events.next() => next,
                };
                let Some(next) = next else { break };
                let Appended(row) = next.map_err(observe)?;
                let Some(event) = row.event() else { continue };
                let kind = EventRow::classify(event.kind);
                // the session is let go of before waiting: a drop answered meanwhile may be any
                // session's
                let (message, slots) = {
                    let live = match sessions.entry(event.aggregate.to_owned()) {
                        Entry::Occupied(entry) => entry.into_mut(),
                        // a session exists from its first forwarded kind of row on
                        Entry::Vacant(entry) if kind.is_some() => {
                            let live = entry.insert(Live::new(gone_tx.clone()));
                            yield live.offer(event.aggregate);
                            live
                        }
                        Entry::Vacant(_) => continue,
                    };
                    let Some(kind) = kind else { continue };
                    if !live.seen.admit(event.seq, event.id) {
                        tracing::debug!(
                            session = event.aggregate,
                            seq = event.seq,
                            id = event.id,
                            "skipping a replayed event row"
                        );
                        continue;
                    }
                    let message = match serde_json::from_str::<Value>(event.data) {
                        Err(err) => Err(MessageError::from(err)),
                        Ok(data) => match kind {
                            EventKind::Role => {
                                live.learn(&data);
                                continue;
                            }
                            EventKind::Part => match OpencodeMessage::split_part(data) {
                                Ok((part, time)) => {
                                    let message_id =
                                        part.get("messageID").and_then(Value::as_str);
                                    let role = live.role(message_id, &mut projection).await;
                                    Ok(OpencodeMessage::part(role, part, time))
                                }
                                Err(err) => Err(MessageError::from(err)),
                            },
                            EventKind::Unmapped => Ok(OpencodeMessage::raw(event.kind, data)),
                        },
                    };
                    (message, live.slots.clone())
                };
                let permits = loop {
                    tokio::select! {
                        biased;
                        Some(id) = gone.recv() => {
                            if let Some(live) = sessions.get_mut(&id)
                                && let Some(session) = live.dropped(&id)
                            {
                                yield session;
                            }
                        }
                        permits = Permits::acquire(&slots, &inflight) => break permits,
                    }
                };
                let live = sessions
                    .get_mut(event.aggregate)
                    .expect("a forwarded row's session is live");
                if let Some(session) = live.push(event.aggregate, message, permits) {
                    yield session;
                }
            }
        }
    }
}

/// A row on its way from `watch()` to a session's `messages()` stream. The permits are released
/// once the consumer has taken the message.
struct Item {
    message: Result<OpencodeMessage, MessageError>,
    _permits: Permits,
}

/// A queued row's slot in its session's queue and its share of the in-flight cap.
struct Permits {
    _slot: OwnedSemaphorePermit,
    _inflight: OwnedSemaphorePermit,
}

impl Permits {
    async fn acquire(slots: &Arc<Semaphore>, inflight: &Arc<Semaphore>) -> Self {
        let slot = slots.clone().acquire_owned().await.expect("session slots are never closed");
        let inflight =
            inflight.clone().acquire_owned().await.expect("the in-flight cap is never closed");
        Self {
            _slot: slot,
            _inflight: inflight,
        }
    }
}

/// A session `watch()` has offered: its queue, the roles of its messages and the rows it has
/// processed.
struct Live {
    tx: flume::Sender<Item>,
    /// `watch()`'s own receiver keeps the queue connected while no consumer holds a handle, so a
    /// dropped handle leaves its undelivered rows where they are. The channel is unbounded because
    /// `slots` already caps it and a full bounded channel would block the sender's thread.
    rx: flume::Receiver<Item>,
    slots: Arc<Semaphore>,
    /// Whether a consumer holds a handle, as far as `watch()` has heard: `false` from a
    /// [`Handle`]'s drop until the next offer.
    held: bool,
    /// While the last offer answered a drop: the rows queued since it was made, so the next
    /// drop can tell whether the consumer took any.
    reoffered: Option<usize>,
    gone: mpsc::UnboundedSender<String>,
    roles: HashMap<String, Role>,
    seen: Seen,
}

impl Live {
    fn new(gone: mpsc::UnboundedSender<String>) -> Self {
        let (tx, rx) = flume::unbounded();
        Self {
            tx,
            rx,
            slots: Arc::new(Semaphore::new(DEMUX_CAP)),
            held: false,
            reoffered: None,
            gone,
            roles: HashMap::new(),
            seen: Seen::default(),
        }
    }

    fn offer(&mut self, id: &str) -> OpencodeSession {
        self.held = true;
        self.reoffered = None;
        OpencodeSession {
            events: Handle {
                rx: self.rx.clone(),
                gone: self.gone.clone(),
                id: id.to_owned(),
            },
        }
    }

    /// The consumer dropped its handle: the session to offer again when rows wait for it. With
    /// an empty queue the next [`push`](Self::push) offers it instead, and so it does when the
    /// dropped handle was itself such a re-offer that nothing was taken from: answering that at
    /// once would only be dropped again, and a consumer dropping every offer would spin `watch()`
    /// without a row being read. Every offer thus answers progress, a row forwarded or taken.
    fn dropped(&mut self, id: &str) -> Option<OpencodeSession> {
        self.held = false;
        let queued = self.rx.len();
        if queued == 0 {
            return None;
        }
        if self.reoffered.is_some_and(|since| queued >= since) {
            tracing::warn!(
                session = id,
                queued,
                "session handle dropped unread again; its rows wait for its next row"
            );
            return None;
        }
        let session = self.offer(id);
        self.reoffered = Some(queued);
        Some(session)
    }

    /// Queues a forwarded row and, when no consumer holds the session, hands it back to offer.
    fn push(
        &mut self,
        id: &str,
        message: Result<OpencodeMessage, MessageError>,
        permits: Permits,
    ) -> Option<OpencodeSession> {
        self.tx
            .send(Item {
                message,
                _permits: permits,
            })
            .expect("watch() keeps a receiver for every live session");
        if let Some(since) = &mut self.reoffered {
            *since += 1;
        }
        (!self.held).then(|| self.offer(id))
    }

    fn learn(&mut self, data: &Value) {
        let info = data.get("info");
        let id = info.and_then(|info| info.get("id")).and_then(Value::as_str);
        let role = info.and_then(|info| info.get("role")).and_then(Value::as_str);
        if let (Some(id), Some(role)) = (id, role) {
            self.roles.insert(id.to_owned(), OpencodeMessage::role_of(role));
        }
    }

    async fn role(&mut self, message_id: Option<&str>, projection: &mut Projection) -> Role {
        let unknown = || Role::Other("unknown".to_owned());
        let Some(message_id) = message_id else {
            return unknown();
        };
        if let Some(role) = self.roles.get(message_id) {
            return role.clone();
        }
        match projection.role(message_id).await {
            Some(role) => {
                self.roles.insert(message_id.to_owned(), role.clone());
                role
            }
            None => unknown(),
        }
    }
}

/// The rows of one aggregate `watch()` has processed: the id of the last row seen at each `seq`.
///
/// `seq` counts up contiguously from 0 within one incarnation of the aggregate and names one row
/// for good, so a row is a replay (the tail rewound, or the file was replaced) exactly when the id
/// remembered at its `seq` is its own. A different id there is a new incarnation: opencode's reset
/// migrations wipe the table and a session resumed afterwards restarts at 0, a restored backup
/// rolls the sequence back to where its copy ends. Ids are only compared for equality: opencode's
/// are not ordered across the 795-day wrap of their time prefix, nor within one incarnation when
/// the writer's clock steps back, so `id <= last` says nothing about which row came first. One
/// id per row is the price of the proof: a range of `seq` with the ids at its ends cannot tell a
/// replayed interior row from a new one written where it was.
#[derive(Default)]
struct Seen {
    ids: HashMap<i64, Box<str>>,
}

impl Seen {
    /// Whether `(seq, id)` is a row not processed yet, recording it if so.
    fn admit(&mut self, seq: i64, id: &str) -> bool {
        if self.ids.get(&seq).is_some_and(|known| known.as_ref() == id) {
            return false;
        }
        self.ids.insert(seq, id.into());
        true
    }
}

/// The consumer's end of a session queue. Dropping it tells `watch()` to offer the session again.
#[derive(Debug)]
struct Handle {
    rx: flume::Receiver<Item>,
    gone: mpsc::UnboundedSender<String>,
    id: String,
}

impl Drop for Handle {
    fn drop(&mut self) {
        // undeliverable only once `watch()` has ended, when there is nothing to offer
        let _ = self.gone.send(std::mem::take(&mut self.id));
    }
}

/// A read-only connection to the observed database for what the tail itself cannot answer.
struct Projection {
    db: PathBuf,
    conn: Option<SqliteConnection>,
}

impl Projection {
    const fn new(db: PathBuf) -> Self {
        Self { db, conn: None }
    }

    async fn connect(&mut self) -> Option<&mut SqliteConnection> {
        if self.conn.is_none() {
            let opts = SqliteConnectOptions::new()
                .filename(&self.db)
                .read_only(true)
                .busy_timeout(Duration::from_secs(5));
            match SqliteConnection::connect_with(&opts).await {
                Ok(conn) => self.conn = Some(conn),
                Err(err) => tracing::warn!(
                    db = %self.db.display(),
                    %err,
                    "cannot open opencode's database for lookups"
                ),
            }
        }
        self.conn.as_mut()
    }

    /// Fails fast when the `event` table does not exist yet (an opencode older than its event
    /// log, or a database caught mid-migration); the consumer decides when to retry.
    async fn require_event_table(&mut self) -> Result<(), WatchError> {
        let Some(conn) = self.connect().await else {
            return Ok(());
        };
        let probe = query_scalar::<Sqlite, i64>(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'event'",
        );
        match probe.fetch_optional(conn).await {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(WatchError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "{}: no `event` table (opencode has not created its event log yet)",
                    self.db.display()
                ),
            ))),
            // whatever is wrong with the database, the observer's connection will report it
            Err(_) => {
                self.conn = None;
                Ok(())
            }
        }
    }

    /// The role opencode's `message` projection records for `message_id` (its `data` is the
    /// message info minus `id` and `sessionID`), for parts whose `message.updated.1` this tail
    /// never saw.
    async fn role(&mut self, message_id: &str) -> Option<Role> {
        let conn = self.connect().await?;
        let lookup =
            query_scalar::<Sqlite, Option<Vec<u8>>>("SELECT data FROM message WHERE id = ?1")
                .bind(message_id)
                .fetch_optional(conn)
                .await;
        let data = match lookup {
            Ok(data) => data.flatten()?,
            Err(err) => {
                tracing::warn!(db = %self.db.display(), %err, "message role lookup failed");
                self.conn = None;
                return None;
            }
        };
        let info: Value = serde_json::from_slice(&data).ok()?;
        info.get("role").and_then(Value::as_str).map(OpencodeMessage::role_of)
    }
}

#[derive(Debug)]
pub struct OpencodeSession {
    events: Handle,
}

impl Session for OpencodeSession {
    type Message = OpencodeMessage;

    fn id(&self) -> SessionId {
        SessionId::from(self.events.id.clone())
    }

    fn messages(
        self,
    ) -> impl Stream<Item = Result<OpencodeMessage, MessageError>> + Send + 'static {
        let handle = self.events;
        async_stream::stream! {
            while let Ok(item) = handle.rx.recv_async().await {
                yield item.message;
                // the rest of `item` (its permits) drops here, once the consumer took the message
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EventKind {
    Role,
    Part,
    Unmapped,
}

/// One `event` row, decoded without type checks: SQLite stores whatever a writer put in a column,
/// and a checked `String` decode of a single NULL, BLOB or non-UTF-8 value would fail the whole
/// page and wedge the tail on that row forever. Non-text values decode lossily.
#[derive(Clone)]
struct EventRow {
    rowid: i64,
    seq: i64,
    id: Option<String>,
    aggregate_id: Option<String>,
    kind: Option<String>,
    data: Option<String>,
}

/// An [`EventRow`] with every column present.
struct Event<'a> {
    id: &'a str,
    aggregate: &'a str,
    seq: i64,
    kind: &'a str,
    data: &'a str,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for EventRow {
    fn from_row(row: &'r SqliteRow) -> Result<Self, sqlx::Error> {
        let text = |column: &str| -> Result<Option<String>, sqlx::Error> {
            let bytes: Option<Vec<u8>> = row.try_get_unchecked(column)?;
            Ok(bytes.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
        };
        Ok(Self {
            rowid: row.try_get_unchecked("rowid")?,
            seq: row.try_get_unchecked("seq")?,
            id: text("id")?,
            aggregate_id: text("aggregate_id")?,
            kind: text("type")?,
            data: text("data")?,
        })
    }
}

impl EventRow {
    fn classify(kind: &str) -> Option<EventKind> {
        match kind {
            "message.updated.1" => Some(EventKind::Role),
            "message.part.updated.1" => Some(EventKind::Part),
            k if k.starts_with("session.next.")
                || k.starts_with("message.updated.")
                || k.starts_with("message.part.updated.") =>
            {
                Some(EventKind::Unmapped)
            }
            _ => None,
        }
    }

    /// The row's columns, or `None` (logged) when one is NULL: SQLite allows that on a non-integer
    /// primary key and a hand-edited database is still a database.
    fn event(&self) -> Option<Event<'_>> {
        let (Some(id), Some(aggregate), Some(kind), Some(data)) = (
            self.id.as_deref(),
            self.aggregate_id.as_deref(),
            self.kind.as_deref(),
            self.data.as_deref(),
        ) else {
            tracing::warn!(rowid = self.rowid, "ignoring an event row with a NULL column");
            return None;
        };
        Some(Event {
            id,
            aggregate,
            seq: self.seq,
            kind,
            data,
        })
    }
}

impl TableSchema for EventRow {
    const TABLE: &'static str = "event";
    const COLUMNS: &'static [&'static str] =
        &["rowid", "id", "aggregate_id", "seq", "type", "data"];
}

impl Tailable for EventRow {
    type Cursor = i64;

    fn cursor(&self) -> i64 {
        self.rowid
    }

    /// The `id` is a never-reused primary key. A NULL id still anchors, as the empty string: a
    /// deleted or replaced row is then still noticed, only another NULL-id row in its place is not.
    fn identity(&self) -> Option<String> {
        Some(self.id.clone().unwrap_or_default())
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

/// opencode's clocks are `Date.now()` millisecond epochs: integers in practice, declared as finite
/// numbers.
#[allow(clippy::cast_possible_truncation, reason = "millisecond epochs fit i64 by a wide margin")]
fn epoch_millis(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_f64().map(|ms| ms.round() as i64))
}

#[derive(Debug, Clone)]
pub struct OpencodeMessage {
    role: Role,
    part: Value,
    time: Option<i64>,
}

impl OpencodeMessage {
    const fn part(role: Role, part: Value, time: Option<i64>) -> Self {
        Self { role, part, time }
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
            part: data,
            time,
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
    fn id(&self) -> Option<MessageId> {
        self.part["id"].as_str().map(|id| MessageId::from(id.to_owned()))
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
        let part = &self.part;
        match part["type"].as_str() {
            Some("text") => {
                vec![Content::Text(part["text"].as_str().unwrap_or_default().to_owned())]
            }
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
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use futures::StreamExt;
    use futures::stream::BoxStream;
    use rstest::rstest;

    use super::*;
    #[cfg(unix)]
    use crate::db::sqlite::Journaling;
    use crate::db::sqlite::Sqlite;
    use crate::db::{query, query_as};
    use crate::harnesstools::session::model::{Content, Role};
    use crate::harnesstools::session::{
        CaptureError, Message, SessionEvent, SessionEventKind, Sessions,
    };

    fn part_message(part: Value) -> OpencodeMessage {
        OpencodeMessage {
            role: Role::Assistant,
            part,
            time: Some(1_700_000_000_000),
        }
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
    #[case::a_rewind_replays_the_known_rows(&[
        (0, "a", true), (1, "b", true), (2, "c", true),
        (0, "a", false), (1, "b", false), (2, "c", false), (3, "d", true),
    ])]
    #[case::a_wipe_restarts_the_aggregate_with_ids_that_sort_lower(&[
        (0, "z", true), (1, "y", true),
        (0, "b", true), (1, "a", true), (2, "c", true),
        (0, "b", false), (1, "a", false), (2, "c", false),
    ])]
    #[case::a_clock_step_back_lowers_the_ids_while_the_seq_goes_on(&[
        (0, "e", true), (1, "f", true), (2, "b", true),
        (0, "e", false), (1, "f", false), (2, "b", false), (3, "g", true),
    ])]
    #[case::rows_below_the_first_seen_are_admitted_once(&[
        (5, "f", true), (6, "g", true),
        (0, "a", true), (1, "b", true), // a rewind cut short by another
        (0, "a", false), (1, "b", false), (2, "c", true), (3, "d", true), (4, "e", true),
        (5, "f", false), (6, "g", false), (7, "h", true),
        (0, "a", false), (1, "b", false), (2, "c", false), (3, "d", false), (4, "e", false),
        (5, "f", false), (6, "g", false), (7, "h", false), (8, "i", true),
    ])]
    #[case::a_restored_backup_resumes_among_the_seen_seqs(&[
        (0, "a", true), (1, "b", true), (2, "c", true), (3, "d", true),
        (0, "a", false), (1, "b", false), // the replay ends where the backup did
        (2, "x", true), (3, "y", true), (4, "z", true),
        (0, "a", false), (1, "b", false), (2, "x", false), (3, "y", false), (4, "z", false),
    ])]
    fn seen_admits_each_row_of_an_incarnation_once(#[case] rows: &[(i64, &str, bool)]) {
        let mut seen = Seen::default();
        for &(seq, id, admitted) in rows {
            assert_eq!(seen.admit(seq, id), admitted, "seq {seq} id {id}");
        }
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

    /// opencode's `event` table (a rowid table: `id` is a TEXT primary key) and its `message`
    /// projection, in WAL mode like the real database.
    async fn event_db(path: &Path) -> Sqlite {
        with_schema(Sqlite::builder(path.as_os_str()).open().await.unwrap()).await
    }

    async fn with_schema(sqlite: Sqlite) -> Sqlite {
        let mut conn = sqlite.pool().acquire().await.unwrap();
        for ddl in [
            "CREATE TABLE event (id TEXT PRIMARY KEY, aggregate_id TEXT NOT NULL, seq INTEGER NOT \
             NULL, type TEXT NOT NULL, data TEXT NOT NULL)",
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created \
             INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL)",
        ] {
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

    /// `(rowid, seq)` of an event: the positions the tail and the replay filter go by.
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

    /// A `message.part.updated.1` row with a text part of `message`.
    async fn text_row(db: &Sqlite, id: &str, session: &str, part: &str, message: &str, text: &str) {
        insert_event(db, id, session, "message.part.updated.1", &text_event(part, message, text))
            .await;
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

    fn events(path: &Path, replay: Replay) -> Events {
        OpencodeSessions::builder()
            .db(path)
            .replay(replay)
            .build()
            .listener()
            .unwrap()
            .events()
            .boxed()
    }

    /// The next `n` items, failing instead of hanging when the tail never delivers them.
    async fn next_n<S: Stream + Unpin + Send>(stream: &mut S, n: usize) -> Vec<S::Item> {
        tokio::time::timeout(Duration::from_secs(10), stream.by_ref().take(n).collect())
            .await
            .unwrap_or_else(|_| panic!("the stream did not deliver {n} items within 10s"))
    }

    /// One line per event, the way a consumer would log it: `started <session>`,
    /// `<session> <role> <text>` or `<session> <error kind>`.
    fn describe(event: &Result<SessionEvent<OpencodeMessage>, CaptureError>) -> String {
        match event {
            Ok(SessionEvent {
                session,
                kind: SessionEventKind::Started,
            }) => format!("started {session}"),
            Ok(SessionEvent {
                session,
                kind: SessionEventKind::Message(message),
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

    /// `Replay::FromNow` anchors where the table ends when the stream is *first polled*, so a row
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

        let mut got = described(&mut events(&path, Replay::All), 4).await;
        got.sort();
        assert_eq!(got, [
            "ses_1 Assistant hello",
            "ses_2 User world",
            "started ses_1",
            "started ses_2"
        ]);
    }

    #[rstest]
    #[tokio::test(flavor = "current_thread")]
    async fn bounded_demux_drains_past_capacity_on_one_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "seed", "ses_1", "msg_1", "assistant").await;
        let parts = DEMUX_CAP + 8;
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

        let got = described(&mut events(&path, Replay::All), parts + 1).await;
        let expected: Vec<String> = std::iter::once("started ses_1".to_owned())
            .chain((0..parts).map(|i| format!("ses_1 Assistant {i}")))
            .collect();
        assert_eq!(got, expected);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_from_now_skips_preexisting_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_pre", "m1", "user").await;

        let mut stream = events(&path, Replay::FromNow);
        let first = first_while_appending(&mut stream, &db, "ses_new", "message.updated.1", |i| {
            role_event(&format!("m{i}"), "user")
        })
        .await;
        assert_eq!(describe(&first), "started ses_new");
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_all_backfills_preexisting_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_pre", "m1", "user").await;

        assert_eq!(described(&mut events(&path, Replay::All), 1).await, ["started ses_pre"]);
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

        let mut stream = events(&path, Replay::All);
        let mut before = described(&mut stream, 5).await;
        before.sort();
        assert_eq!(before, [
            "ses_X User gone",
            "ses_Y User one",
            "ses_Y User two",
            "started ses_X",
            "started ses_Y"
        ]);

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

        let mut stream = events(&path, Replay::All);
        let mut before = described(&mut stream, 5).await;
        before.sort();
        assert_eq!(before, [
            "ses_X User one",
            "ses_X User two",
            "ses_Z User gone",
            "started ses_X",
            "started ses_Z"
        ]);

        // deleting the newest session takes the tail's anchor row with it: the tail rewinds and
        // ses_X's rows come by again, in rowid order, ahead of anything new
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

        let mut stream = events(&path, Replay::All);
        assert_eq!(described(&mut stream, 2).await, ["started ses_A", "ses_A User one"]);

        // opencode's reset migrations: rowids and the aggregate's seq both start over, and the
        // session resumed afterwards writes ids that sort below its old ones
        execute(&db, "DELETE FROM event").await;
        role_row(&db, &post_wrap(3), "ses_A", "msg_a2", "assistant").await;
        text_row(&db, &post_wrap(4), "ses_A", "prt_a2", "msg_a2", "two").await;
        assert_eq!(position(&db, &post_wrap(3)).await, (1, 0));

        assert_eq!(described(&mut stream, 1).await, ["ses_A Assistant two"]);
    }

    // Windows refuses to unlink or replace a file SQLite holds open (it opens without
    // FILE_SHARE_DELETE), so the database can only be swapped under a live tail on unix.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rows_of_a_replaced_database_file_are_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let old = event_db(&path).await;
        role_row(&old, "o1", "ses_old", "msg_o", "user").await;
        text_row(&old, "o2", "ses_old", "prt_o", "msg_o", "before").await;

        let mut stream = events(&path, Replay::All);
        assert_eq!(described(&mut stream, 2).await, ["started ses_old", "ses_old User before"]);

        // `rm opencode.db*` and a fresh opencode: a new file at the same path, rowids from 1
        old.pool().close().await;
        drop(old);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(dir.path().join(format!("opencode.db{suffix}")));
        }
        // written as one self-contained file (rollback journal) so it can be moved into place
        let fresh = dir.path().join("fresh.db");
        let db = with_schema(
            Sqlite::builder(fresh.as_os_str())
                .journal(Some(Journaling::Delete))
                .open()
                .await
                .unwrap(),
        )
        .await;
        role_row(&db, "n1", "ses_new", "msg_n", "user").await;
        text_row(&db, "n2", "ses_new", "prt_n", "msg_n", "after").await;
        db.pool().close().await;
        std::fs::rename(&fresh, &path).unwrap();

        assert_eq!(described(&mut stream, 2).await, ["started ses_new", "ses_new User after"]);
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

        let mut stream = events(&path, Replay::All);
        assert_eq!(described(&mut stream, 4).await, [
            "started ses_A",
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

        assert_eq!(described(&mut stream, 2).await, [
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
        insert_event(&db, "e1", "ses_old", "session.updated.1", r#"{"sessionID":"ses_old"}"#).await;
        insert_event(&db, "e2", "ses_gone", "session.deleted.1", r#"{"sessionID":"ses_gone"}"#)
            .await;
        role_row(&db, "e3", "ses_1", "msg_1", "user").await;
        text_row(&db, "e4", "ses_1", "prt_1", "msg_1", "hi").await;

        assert_eq!(described(&mut events(&path, Replay::All), 2).await, [
            "started ses_1",
            "ses_1 User hi"
        ]);
    }

    type Offers = BoxStream<'static, Result<OpencodeSession, WatchError>>;

    fn watch(path: &Path) -> Offers {
        OpencodeSessions::builder().db(path).build().listener().unwrap().watch().boxed()
    }

    /// The next session `watch()` offers, failing instead of hanging.
    async fn offered(stream: &mut Offers) -> OpencodeSession {
        next_n(stream, 1).await.pop().expect("watch() ended").unwrap()
    }

    /// `watch()` only forwards while polled: the first `n` messages of `session` with `stream`
    /// driven alongside.
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

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dropped_session_handle_is_offered_again_with_nothing_lost() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "user").await;
        text_row(&db, "e2", "ses_1", "prt_1", "msg_1", "one").await;

        let mut stream = watch(&path);
        // dropped before any row of it was queued: offered again along with its first row
        let first = offered(&mut stream).await;
        assert_eq!(first.id(), SessionId::from("ses_1".to_owned()));
        drop(first);
        let second = offered(&mut stream).await;
        assert_eq!(second.id(), SessionId::from("ses_1".to_owned()));
        // dropped with that row queued: offered again at once, no further row needed
        drop(second);
        let third = offered(&mut stream).await;
        assert_eq!(third.id(), SessionId::from("ses_1".to_owned()));

        text_row(&db, "e3", "ses_1", "prt_2", "msg_1", "two").await;
        assert_eq!(delivered(stream, third, 2).await, vec![
            (Role::User, vec![Content::Text("one".into())]),
            (Role::User, vec![Content::Text("two".into())]),
        ]);
    }

    /// Drives `watch()` while `session` is held until `n` rows are queued for it: nothing can
    /// be offered meanwhile, so each poll only forwards rows.
    async fn forwarded(stream: &mut Offers, session: &OpencodeSession, n: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while session.events.rx.len() < n {
                assert!(
                    tokio::time::timeout(Duration::from_millis(10), stream.next()).await.is_err(),
                    "watch() offered a session while one was held"
                );
            }
        })
        .await
        .unwrap_or_else(|_| panic!("watch() did not queue {n} rows within 10s"));
    }

    /// The first queued message of `session`; the handle is dropped afterwards.
    async fn take_one(session: OpencodeSession) -> Vec<Content> {
        let mut messages = session.messages().boxed();
        next_n(&mut messages, 1).await.pop().unwrap().unwrap().content()
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_re_offer_dropped_unread_waits_for_the_next_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "user").await;
        text_row(&db, "e2", "ses_1", "prt_1", "msg_1", "one").await;
        role_row(&db, "e3", "ses_2", "msg_2", "user").await;
        text_row(&db, "e4", "ses_2", "prt_2", "msg_2", "two").await;

        // a consumer that drops every offer: each session is offered on its role row, again
        // with its part and once more for dropping that, then the tail moves on
        let mut stream = watch(&path);
        let mut ids = Vec::new();
        for _ in 0..6 {
            ids.push(offered(&mut stream).await.id().to_string());
        }
        assert_eq!(ids, ["ses_1", "ses_1", "ses_1", "ses_2", "ses_2", "ses_2"]);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), stream.next()).await.is_err(),
            "watch() answered the drop of a re-offer without a row"
        );

        // the next row offers the session again, the row that waited still ahead of it
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
    async fn a_re_offer_read_from_is_offered_again_when_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "user").await;
        for (id, text) in [("e2", "one"), ("e3", "two"), ("e4", "three")] {
            text_row(&db, id, "ses_1", &format!("prt_{id}"), "msg_1", text).await;
        }

        let mut stream = watch(&path);
        drop(offered(&mut stream).await);
        // held until every part is queued, then dropped unread: answered at once
        let held = offered(&mut stream).await;
        forwarded(&mut stream, &held, 3).await;
        drop(held);
        // a consumer that reads one message per offer and lets go: each drop read something,
        // so each is answered at once, until nothing is queued
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
            "watch() offered a session with nothing queued for it"
        );
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sessions_dropped_with_full_queues_are_offered_again_while_the_cap_is_full() {
        // as many sessions with full queues as it takes to hold every in-flight permit
        const DROPPED: usize = INFLIGHT_CAP / DEMUX_CAP;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        seed_sessions(&db, DROPPED, DEMUX_CAP).await;
        role_row(&db, "z1", "ses_z", "msg_z", "user").await;
        text_row(&db, "z2", "ses_z", "prt_z", "msg_z", "after").await;

        let mut stream = watch(&path);
        // held without being drained, their rows fill the cap; ses_z is offered on its role row
        // and its part then parks watch() on the cap
        let mut held: Vec<OpencodeSession> =
            next_n(&mut stream, DROPPED + 1).await.into_iter().map(Result::unwrap).collect();
        let last = held.pop().unwrap();
        assert_eq!(last.id(), SessionId::from("ses_z".to_owned()));
        assert!(
            tokio::time::timeout(Duration::from_secs(1), stream.next()).await.is_err(),
            "watch() offered a session while every permit was held"
        );

        drop(held);
        let again: Vec<OpencodeSession> =
            next_n(&mut stream, DROPPED).await.into_iter().map(Result::unwrap).collect();
        let ids: Vec<String> = again.iter().map(|session| session.id().to_string()).collect();
        assert_eq!(ids, (0..DROPPED).map(|i| format!("ses_{i:04}")).collect::<Vec<_>>());

        // draining the re-offers frees the cap and the parked row goes through, roles intact
        for session in again {
            let mut messages = session.messages().boxed();
            let queued = next_n(&mut messages, DEMUX_CAP).await;
            assert!(queued.iter().all(|m| m.as_ref().unwrap().role() == Role::User));
        }
        assert_eq!(delivered(stream, last, 1).await, vec![(Role::User, vec![Content::Text(
            "after".into()
        )])]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sessions_let_go_of_as_they_end_are_offered_again_before_their_rows_pin_the_cap() {
        // the consumer moves on to each new session with the previous one's rows still queued;
        // those sessions get no further row, so each must come back on its own, and together
        // their queued rows exceed the cap
        const PARTS: usize = DEMUX_CAP - 4;
        const SESSIONS: usize = INFLIGHT_CAP / PARTS + 2;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        seed_sessions(&db, SESSIONS, PARTS).await;

        let mut stream = watch(&path);
        let mut seen = HashSet::new();
        let mut held = None;
        let mut drained = Vec::new();
        while drained.len() < SESSIONS - 1 {
            match offered(&mut stream).await {
                // a new session displaces the one held so far, its rows still queued
                session if seen.insert(session.id()) => drop(held.replace(session)),
                // a re-offer, drained in full without driving `watch()`: its rows were queued
                // before the session that displaced it was even offered
                session => {
                    let id = session.id().to_string();
                    let mut messages = session.messages().boxed();
                    assert!(next_n(&mut messages, PARTS).await.iter().all(Result::is_ok));
                    drained.push(id);
                }
            }
        }
        // each session came back right after the one that displaced it
        assert_eq!(drained, (0..SESSIONS - 1).map(|i| format!("ses_{i:04}")).collect::<Vec<_>>());
        assert_eq!(delivered(stream, held.unwrap(), PARTS).await.len(), PARTS);
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
            execute(
                &db,
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES \
                 ('msg_1', 'ses_1', 0, 0, '{\"role\": \"user\", \"time\": {\"created\": 0}}')",
            )
            .await;
        }

        // the tail starts between the message's role row and its first part
        let mut stream = events(&path, Replay::FromNow);
        let first =
            first_while_appending(&mut stream, &db, "ses_1", "message.part.updated.1", |i| {
                text_event(&format!("prt_{i}"), "msg_1", "hi")
            })
            .await;
        assert_eq!(describe(&first), "started ses_1");
        assert_eq!(described(&mut stream, 1).await, [format!("ses_1 {role:?} hi")]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_bad_row_is_reported_and_the_tail_continues() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        role_row(&db, "e1", "ses_1", "msg_1", "user").await;
        // valid JSON of the wrong shape, a BLOB, and a NULL primary key (legal on a TEXT one)
        insert_event(&db, "e2", "ses_1", "message.part.updated.1", "[]").await;
        execute(
            &db,
            "INSERT INTO event (id, aggregate_id, seq, type, data) VALUES ('e3', 'ses_1', 2, \
             'message.part.updated.1', X'FFFE')",
        )
        .await;
        execute(
            &db,
            "INSERT INTO event (id, aggregate_id, seq, type, data) VALUES (NULL, 'ses_1', 3, \
             'message.part.updated.1', '{}')",
        )
        .await;
        text_row(&db, "e5", "ses_1", "prt_1", "msg_1", "ok").await;

        assert_eq!(described(&mut events(&path, Replay::All), 4).await, [
            "started ses_1",
            "ses_1 json error",
            "ses_1 json error",
            "ses_1 User ok"
        ]);
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
    async fn a_backfill_keeps_at_most_the_cap_in_flight() {
        const SESSIONS: usize = 3_000;
        const PARTS: usize = 9;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let db = event_db(&path).await;
        // short sessions (a role row and nine parts) never fill their own queue, so only the
        // global cap stands between a backfill and the whole table sitting in memory
        seed_sessions(&db, SESSIONS, PARTS).await;

        let mut stream = events(&path, Replay::All);
        let (mut started, mut delivered) = (0, 0);
        while started < SESSIONS || delivered < SESSIONS * PARTS {
            let event = tokio::time::timeout(Duration::from_secs(10), stream.next())
                .await
                .expect("the backfill stalled")
                .expect("the stream ended")
                .unwrap();
            match event.kind {
                SessionEventKind::Started => {
                    // every part of the sessions before this one has been forwarded by now
                    let id: &str = event.session.as_ref();
                    let k: usize = id.strip_prefix("ses_").unwrap().parse().unwrap();
                    assert!(
                        k * PARTS <= delivered + INFLIGHT_CAP,
                        "{} rows in flight at {}",
                        k * PARTS - delivered,
                        event.session
                    );
                    started += 1;
                }
                SessionEventKind::Message(_) => delivered += 1,
            }
        }
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

        let mut started: HashSet<String> = HashSet::new();
        let mut by_session: HashMap<String, Vec<OpencodeMessage>> = HashMap::new();
        for event in next_n(&mut events(&path, Replay::All), 11).await {
            let event = event.unwrap();
            let sid = event.session.to_string();
            match event.kind {
                SessionEventKind::Started => {
                    started.insert(sid);
                }
                SessionEventKind::Message(message) => {
                    by_session.entry(sid).or_default().push(message);
                }
            }
        }

        assert_eq!(started, HashSet::from(["ses_A".to_owned(), "ses_B".to_owned()]));

        let a = &by_session["ses_A"];
        assert!(a.iter().all(|m| m.timestamp().is_some()), "a part is missing its timestamp");

        let a1 = a
            .iter()
            .find(|m| m.id() == Some(MessageId::from("prtA1".to_owned())))
            .expect("prtA1 missing");
        assert_eq!(a1.role(), Role::User);
        assert_eq!(a1.content(), vec![Content::Text("hello from A".into())]);

        let a2_final = a
            .iter()
            .rfind(|m| m.id() == Some(MessageId::from("prtA2".to_owned())))
            .expect("prtA2 missing");
        assert_eq!(a2_final.role(), Role::Assistant);
        assert_eq!(a2_final.content(), vec![Content::Text("final answer A".into())]);

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

        // session.created.1 and message.part.removed.1 yield nothing; the unmapped version and
        // the session.next event surface whole, dated and named by their type
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
        assert_eq!(prompted.role(), Role::Other("session.next.prompted".to_owned()));
        assert_eq!(prompted.timestamp().unwrap().unix_timestamp(), 1_700_000_011);
        assert!(matches!(
            prompted.content().as_slice(),
            [Content::Other(v)] if v["prompt"]["text"] == "second prompt from B"
        ));
    }
}
