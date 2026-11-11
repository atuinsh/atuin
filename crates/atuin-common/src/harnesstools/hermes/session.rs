//! Hermes Agent sessions, read from its `state.db`.
//!
//! Every surface Hermes has (CLI, TUI, Desktop, ACP, the gateway platforms, cron) writes to one
//! SQLite store per home: `$HERMES_HOME/state.db` (`~/.hermes/state.db`), and one per profile,
//! `<home>/profiles/<name>/state.db`. A session is a `sessions` row, its conversation the
//! `messages` rows naming it, in `messages.id` order: that id is global across the store,
//! AUTOINCREMENT and never reused, so it is each row's order and its checkpoint. Hermes orders by
//! it too, never by timestamp. A row is captured under its `message_uid` instead (see
//! `row_key`), the one per-row value Hermes keeps when a session is imported, branched or
//! compacted: `messages.id` is minted afresh by each of those.
//!
//! **Which sessions.** Only coding work becomes a thread: the CLI, TUI, ACP and one-shot (`-q`)
//! runs, and since v0.22 the `local` sessions those surfaces (and Desktop) create (see
//! `store` for the one place the lists live). Gateway platforms, cron, kanban,
//! tool runs and subagents are not read; subagents (`source = 'subagent'`) are a follow-up. A
//! session that carries on one Hermes ended for compression (its parent's `end_reason` is
//! `compression`, the rotating layout from before in-place compaction) is a session of its own,
//! naming that parent as a [`ParentKind::Continuation`].
//!
//! **Rows.**
//! - `user`: what the user typed, a steer sent mid-turn (`display_kind = 'steer'`) included.
//! - `assistant`: the model's `reasoning` as thinking, its `content` as text (often empty beside
//!   tool calls, then left out), and each `tool_calls[]` entry as a call, with
//!   `function.arguments` parsed from the JSON string it is (kept as the string if it isn't
//!   JSON). `finish_reason` is the stop reason. A turn that failed (`display_kind =
//!   'failed_turn'`, a 401 from the provider) is an error: what `display_metadata.error` says.
//! - `tool`: a call's result, as the string Hermes stored, context Hermes appended to it
//!   (`[Subdirectory context discovered: …]`) included. It is an error when the JSON it starts
//!   with has an `exit_code` other than 0, a non-null `error`, or a `status` of `blocked` or
//!   `error`; plain text is no error.
//! - Content stored as `"\x00json:"` and JSON is multimodal parts: text parts are text, the rest
//!   kept as they are.
//!
//! Skipped: compaction summaries (`_compressed_summary`), scaffolding Hermes hides
//! (`display_kind = 'hidden'`, `display_metadata.model_only`), gateway lurk rows (`observed`), a
//! row with nothing in it, and a row whose `message_uid` an earlier row of the session has (a
//! copy in-place compaction carried into the live transcript). The system prompt
//! (`system_prompts`) is not captured, as no other harness's is.
//!
//! Every row carries the session's `cwd` and the model's rows its `model` and provider
//! (`model_config.gateway_runtime.provider`). There is no per-row usage: Hermes keeps token
//! counts per session only.
//!
//! **Titles.** `sessions.title`, which Hermes writes at the start of a turn and may upgrade from a
//! model call later, and `/title` sets. The reader yields it as a row of its own (a title row, as
//! Kiro's reader does) whenever it changes, after the rows of the read that saw it change: a
//! `user` or legacy (NULL) `title_source` is [`TitleSource::Named`], `derived` and `llm`
//! [`TitleSource::Generated`]. A title row's id is its text and how many title rows the read gave
//! before it, so a read from the start mints none it gave before. Hermes keeps no title history
//! and atuin writes nothing to its store, so one case is lost: a session renamed and then renamed
//! back while capture is stopped reads, on restart, as the title it first had, whose row is
//! already stored, and keeps the rename until its title next changes (opencode's title rows share
//! this).
//!
//! **Rewinds.** `/undo`, `/retry` and edits don't delete rows: they mark the rows they drop no
//! longer active (`active = 0`, `compacted = 0`) and append the new turn. Each row names a parent
//! so the database keeps the dropped turn as a dead branch, as it does Claude Code's rewinds: a
//! live row hangs from the nearest earlier row this read yielded that is live now, a rewound one
//! from the nearest earlier row it yielded, whatever its state. Live capture reads rows as they
//! land, so its links are exact; an import of old rewinds can join two undone turns in a row
//! into one dead branch, since nothing says the second was undone apart from the first. A row
//! still counts as live when a copy of it is (a compaction clone under the same `message_uid`).
//!
//! In-place edits of a row's content (`transcript_revision`) are not followed: what was
//! captured stands.
//!
//! **Not holding the store.** Hermes refuses its own maintenance while another process holds the
//! store (see `store`), so nothing here keeps a connection between reads. The
//! listener watches the store's directory (`state.db` and `state.db-wal`, whose modification
//! time moves on every commit) and, on a change, opens the store, lists its coding sessions,
//! closes it, and wakes the sessions that changed. A session then opens the store for each page
//! it reads, and first checks the row it last yielded is still there as it was: a store replaced
//! under it (a backup restored, a repair) can hand out the ids it read past again, so a read
//! whose row is gone or changed starts the session over (a new checkpoint generation).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures::stream::{BoxStream, SelectAll};
use futures::{Stream, StreamExt};
use serde_json::Value;
use time::OffsetDateTime;
use tokio::sync::watch;
use typed_builder::TypedBuilder;
use xxhash_rust::xxh3::xxh3_64;

use super::store::{
    self, DB_FILE, MessageFlags, MessageRow, PROFILES_DIR, SessionRow, Store, WAL_FILE,
};
use super::{Hermes, hermes_home};
use crate::fs::tree_watcher::TreeWatcher;
use crate::harnesstools::session::model::{
    Content, MessageId, ParentKind, Role, StopReason, TitleChange, TitleSource, ToolCallId,
    ToolResult, ToolUse,
};
use crate::harnesstools::session::synthetic::SYNTHETIC;
use crate::harnesstools::session::{
    Checkpoint, Listener, Message, MessageError, Observable, RuntimeError, Session, SessionId,
    Sessions, WatchError,
};
use crate::sync::BlockingPool;

/// How many rows a session reads per open of the store.
const PAGE: u32 = 500;

/// How often the listener looks for profiles made since it started.
const PROFILE_SCAN: Duration = Duration::from_secs(30);

/// The prefix Hermes stores multimodal content under, followed by its JSON.
const JSON_CONTENT: &str = "\u{0}json:";

/// The kind of the reader's own title rows, which no Hermes row has: their role.
const TITLE_ROW: &str = "atuin-title";

#[derive(Debug, Clone, TypedBuilder)]
pub struct HermesSessions {
    /// The Hermes home; [`hermes_home`] when not given.
    #[builder(default, setter(strip_option, into))]
    home: Option<PathBuf>,
    // Unused: sqlx reads the store on threads of its own. Taken so every harness's sessions are
    // built alike.
    #[allow(dead_code)]
    pool: BlockingPool,
}

impl HermesSessions {
    fn root(&self) -> PathBuf {
        self.home.clone().unwrap_or_else(hermes_home)
    }
}

/// The store under the Hermes home `root` holding session `id`, `None` when none does or the
/// stores can't be read.
pub(crate) async fn locate(root: &Path, id: &str) -> Option<PathBuf> {
    for db in store::stores(root).into_iter().flatten() {
        let Ok(mut store) = Store::open(&db).await else {
            continue;
        };
        let held = store.holds(id).await;
        store.close().await;
        if matches!(held, Ok(true)) {
            return Some(db);
        }
    }
    None
}

/// The newest modification time of a store's database or log.
fn store_modified(db: &Path) -> Option<SystemTime> {
    let mtime = |path: &Path| std::fs::metadata(path).ok()?.modified().ok();
    mtime(db).max(mtime(&db.with_file_name(WAL_FILE)))
}

impl Sessions for HermesSessions {
    type Listener = HermesListener;

    fn last_active(&self) -> Option<SystemTime> {
        store::stores(&self.root()).iter().flatten().filter_map(|db| store_modified(db)).max()
    }

    fn listener(&self) -> Result<HermesListener, RuntimeError> {
        let root = self.root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(HermesListener {
            root,
            profile_scan: PROFILE_SCAN,
        })
    }

    fn existing(
        &self,
    ) -> Result<
        impl Stream<Item = Result<HermesSession, RuntimeError>> + Send + 'static,
        RuntimeError,
    > {
        let root = self.root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(async_stream::stream! {
            for db in store::stores(&root) {
                let db = match db {
                    Ok(db) => db,
                    Err(err) => {
                        yield Err(RuntimeError::Io(err));
                        continue;
                    }
                };
                match list(&db).await {
                    Ok(rows) => {
                        let db: Arc<Path> = Arc::from(db.as_path());
                        for row in rows {
                            yield Ok(HermesSession::new(Arc::clone(&db), &row, None));
                        }
                    }
                    Err(err) => yield Err(RuntimeError::Io(std::io::Error::other(err))),
                }
            }
        })
    }
}

/// Every coding session in the store `db`, read in one open.
async fn list(db: &Path) -> Result<Vec<SessionRow>, sqlx::Error> {
    let mut store = Store::open(db).await?;
    let rows = store.sessions().await;
    store.close().await;
    rows
}

impl Observable for Hermes {
    type Sessions = HermesSessions;

    fn sessions(&self, pool: BlockingPool) -> HermesSessions {
        HermesSessions::builder().pool(pool).build()
    }
}

#[derive(Debug, Clone)]
pub struct HermesListener {
    root: PathBuf,
    /// How often to look for profiles made since the listener started.
    profile_scan: Duration,
}

/// What the listener last saw of a session, to tell when to wake its reader.
struct Known {
    wake: watch::Sender<u64>,
    seen: Mark,
}

/// What of a session's row says its reader has something new to read: a row or a title. A
/// rewind adds no row, and the reader checks which rows it undid when the next one lands.
#[derive(Debug, PartialEq, Eq)]
struct Mark {
    tip: Option<i64>,
    title: Option<String>,
    title_source: Option<String>,
}

impl Mark {
    fn of(row: &SessionRow) -> Self {
        Self {
            tip: row.tip,
            title: row.title.clone(),
            title_source: row.title_source.clone(),
        }
    }
}

/// A unit for each change to `file` (once when it appears, then each write), ending when it goes.
fn changes_of(
    dir: PathBuf,
    mut stat: watch::Receiver<crate::fs::tree_watcher::FileStat>,
) -> BoxStream<'static, PathBuf> {
    async_stream::stream! {
        yield dir.clone();
        while stat.changed().await.is_ok() {
            yield dir.clone();
        }
    }
    .boxed()
}

/// Whether `path` is a file of a store the listener watches.
fn is_store_file(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == DB_FILE || name == WAL_FILE)
}

impl HermesListener {
    /// The directories holding a store, or that may come to: the home and each profile's.
    fn store_dirs(root: &Path) -> Vec<PathBuf> {
        let mut dirs = vec![root.to_path_buf()];
        let mut profiles: Vec<PathBuf> = std::fs::read_dir(root.join(PROFILES_DIR))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
            .map(|entry| entry.path())
            .collect();
        profiles.sort();
        dirs.extend(profiles);
        dirs
    }
}

impl Listener for HermesListener {
    type Session = HermesSession;

    /// Each coding session of every store under the home, as the store is first read, and each
    /// one made later, as the change that made it is seen. A profile made while the listener runs
    /// is found within 30 seconds (`PROFILE_SCAN`).
    fn watch(self) -> impl Stream<Item = Result<HermesSession, WatchError>> + Send + 'static {
        let root = self.root;
        let profile_scan = self.profile_scan;
        async_stream::stream! {
            // Not recursive: a home holds Hermes's own checkout and caches beside the store.
            let mut watched: HashSet<PathBuf> = HashSet::new();
            let mut files: SelectAll<BoxStream<'static, (PathBuf, crate::fs::tree_watcher::WatchedFile)>> =
                SelectAll::new();
            let mut changes: SelectAll<BoxStream<'static, PathBuf>> = SelectAll::new();
            let mut stores: HashMap<PathBuf, HashMap<String, Known>> = HashMap::new();
            let mut scan = tokio::time::interval(profile_scan);
            scan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = scan.tick() => {
                        for dir in Self::store_dirs(&root) {
                            if watched.contains(&dir) {
                                continue;
                            }
                            match TreeWatcher::builder().recursive(false).filter(is_store_file).watch(&dir) {
                                Ok(watcher) => {
                                    watched.insert(dir.clone());
                                    files.push(watcher.map(move |file| (dir.clone(), file)).boxed());
                                }
                                Err(err) => {
                                    tracing::warn!(dir = %dir.display(), %err, "can't watch a Hermes home");
                                    if dir == root {
                                        yield Err(WatchError::from(err));
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    Some((dir, file)) = files.next(), if !files.is_empty() => {
                        let (_, stat) = file.into_parts();
                        changes.push(changes_of(dir, stat));
                    }
                    Some(dir) = changes.next(), if !changes.is_empty() => {
                        let db = dir.join(DB_FILE);
                        if !db.is_file() {
                            continue;
                        }
                        let rows = match list(&db).await {
                            Ok(rows) => rows,
                            Err(err) => {
                                // A store mid-replace, or locked past the timeout: the next change
                                // reads it again.
                                tracing::warn!(db = %db.display(), %err, "can't read a Hermes store");
                                continue;
                            }
                        };
                        let known = stores.entry(db.clone()).or_default();
                        let shared: Arc<Path> = Arc::from(db.as_path());
                        let mut present = HashSet::new();
                        for row in rows {
                            present.insert(row.id.clone());
                            let mark = Mark::of(&row);
                            match known.get_mut(&row.id) {
                                Some(seen) => {
                                    if seen.seen != mark {
                                        seen.seen = mark;
                                        seen.wake.send_modify(|n| *n = n.wrapping_add(1));
                                    }
                                }
                                None => {
                                    let (wake, woken) = watch::channel(0);
                                    known.insert(row.id.clone(), Known { wake, seen: mark });
                                    yield Ok(HermesSession::new(Arc::clone(&shared), &row, Some(woken)));
                                }
                            }
                        }
                        // A session gone from the store (deleted, or no longer a coding one) ends
                        // its reader after one last read.
                        known.retain(|id, _| present.contains(id));
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct HermesSession {
    id: SessionId,
    /// The store holding the session.
    db: Arc<Path>,
    /// When Hermes last recorded anything of the session, as the store was last listed.
    touched: Option<SystemTime>,
    /// Wakes [`messages`](Session::messages) when the listener sees the session change; `None`
    /// reads it once.
    changes: Option<watch::Receiver<u64>>,
}

/// `secs` (unix seconds, as Hermes stores times) as a time, `None` when out of range.
fn time_of(secs: f64) -> Option<OffsetDateTime> {
    if !secs.is_finite() {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    OffsetDateTime::from_unix_timestamp_nanos((secs * 1e9) as i128).ok()
}

impl HermesSession {
    fn new(db: Arc<Path>, row: &SessionRow, changes: Option<watch::Receiver<u64>>) -> Self {
        Self {
            id: SessionId::from(row.id.clone()),
            db,
            touched: time_of(row.touched()).map(SystemTime::from),
            changes,
        }
    }

    /// Session `id` of the store `db`, read once as it stands.
    #[must_use]
    pub fn open(id: SessionId, db: &Path) -> Self {
        Self {
            id,
            db: Arc::from(db),
            touched: None,
            changes: None,
        }
    }

    /// The store holding the session.
    #[must_use]
    pub fn db(&self) -> &Path {
        &self.db
    }
}

/// What every row of a session carries, from its `sessions` row.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Context {
    cwd: Option<PathBuf>,
    model: Option<String>,
    provider: Option<String>,
    /// The session this one continues, after a compaction that rotated it.
    continues: Option<SessionId>,
}

impl Context {
    fn of(row: &SessionRow) -> Self {
        Self {
            cwd: row.cwd.as_deref().filter(|cwd| !cwd.is_empty()).map(PathBuf::from),
            model: row.model.clone().filter(|model| !model.is_empty()),
            provider: row.provider(),
            continues: row
                .parent
                .clone()
                .filter(|_| row.parent_end_reason.as_deref() == Some("compression"))
                .map(SessionId::from),
        }
    }
}

/// The checkpoint just past row `id`, written at `timestamp` with `role`: a row Hermes deleted and
/// another later took the id of (a replaced store) doesn't name it.
fn checkpoint(id: i64, timestamp: f64, role: &str) -> Checkpoint {
    let mut item = id.to_le_bytes().to_vec();
    item.extend_from_slice(&timestamp.to_bits().to_le_bytes());
    item.extend_from_slice(role.as_bytes());
    #[allow(clippy::cast_sign_loss)]
    Checkpoint::new(id.max(0) as u64, &item)
}

/// The title `row` gives its session, `None` for none.
fn title_of(row: &SessionRow) -> Option<TitleChange> {
    let source = match row.title_source.as_deref() {
        Some("derived" | "llm") => TitleSource::Generated,
        _ => TitleSource::Named,
    };
    Some(TitleChange::new(source, row.title.as_deref()?)).filter(|title| title.text.is_some())
}

/// What a read carries from one row to the rows after it.
#[derive(Debug, Default)]
struct Reading {
    /// The rows this read yielded (or, resumed, would have), oldest first: each one's id, whether
    /// it is live, and its key ([`row_key`]).
    yielded: Vec<(i64, bool, String)>,
    /// The `message_uid` of every row read.
    uids: HashSet<String>,
    /// The last row read, yielded or not.
    cursor: i64,
    /// The title last yielded, and how many title rows were.
    title: Option<TitleChange>,
    titled: u32,
}

impl Reading {
    /// Whether to yield a row with `flags`, noting its `message_uid` either way.
    fn keeps(&mut self, flags: &MessageFlags) -> bool {
        let copy = flags.message_uid.clone().is_some_and(|uid| !self.uids.insert(uid));
        let shown = !(flags.observed
            || flags.compressed_summary
            || flags.model_only
            || copy
            || flags.display_kind.as_deref() == Some("hidden"));
        shown && (flags.has_body || flags.role == "tool")
    }

    /// Note row `id`, keyed `key`, yielded, as the newest row of the read.
    fn push(&mut self, id: i64, live: bool, key: String) {
        self.yielded.push((id, live, key));
    }

    /// The key of the row a row that is `live` (or not) hangs from (see the module docs).
    fn parent(&self, live: bool) -> Option<String> {
        self.yielded.iter().rev().find(|(_, was, _)| !live || *was).map(|(_, _, key)| key.clone())
    }

    /// Mark the rows in `rewound` no longer live, and the rest live.
    fn rewind(&mut self, rewound: &HashSet<i64>) {
        for (id, live, _) in &mut self.yielded {
            *live = !rewound.contains(id);
        }
    }

    /// Take in the rows a read resumed after a checkpoint skipped over, so it links and dedupes
    /// what it reads next as a read from the start would.
    fn prime(&mut self, rows: &[MessageFlags]) {
        for flags in rows {
            if self.keeps(flags) {
                self.push(flags.id, flags.live, row_key(flags.id, flags.message_uid.as_deref()));
            }
            self.cursor = flags.id;
        }
    }
}

/// The key a row is captured under: its `message_uid`, which Hermes keeps through an import, a
/// `/branch` and a compaction copy, so a row atuin writes into a store reads back under the key
/// it was written from. `messages.id` (`id`), which every write mints afresh, stands in for a row
/// with none (a store from before v0.22, or a NULL Hermes hasn't backfilled yet).
fn row_key(id: i64, message_uid: Option<&str>) -> String {
    message_uid.filter(|uid| !uid.is_empty()).map_or_else(|| id.to_string(), str::to_owned)
}

/// What the reader needs of `row` to decide whether it is kept.
fn flags_of(row: &MessageRow) -> MessageFlags {
    let metadata = metadata(row);
    let calls = row.tool_calls.as_deref().is_some_and(|calls| calls.len() > 2);
    MessageFlags {
        id: row.id,
        role: row.role.clone(),
        live: row.live,
        observed: row.observed,
        compressed_summary: row.compressed_summary,
        display_kind: row.display_kind.clone(),
        model_only: metadata["model_only"].as_bool() == Some(true),
        message_uid: row.message_uid.clone(),
        has_body: row.content.as_deref().is_some_and(|c| !c.is_empty())
            || row.reasoning.as_deref().is_some_and(|r| !r.is_empty())
            || calls,
    }
}

fn metadata(row: &MessageRow) -> Value {
    row.display_metadata
        .as_deref()
        .and_then(|m| serde_json::from_str(m).ok())
        .unwrap_or(Value::Null)
}

/// What one open of the store read of the session.
struct Page {
    session: Option<SessionRow>,
    rows: Vec<MessageRow>,
    rewound: Option<HashSet<i64>>,
    /// The row the read last yielded is gone or no longer the same: the store was replaced (a
    /// backup restored, a repair) or the session's rows rewritten, so ids past it may have been
    /// handed out again, and the read has to start over.
    moved: bool,
}

impl HermesSession {
    /// Where a read from `from` starts, and the rows it skips over: `None` when it starts from the
    /// beginning because `from` no longer names its row.
    async fn resume_at(&self, from: Checkpoint) -> Result<Option<Vec<MessageFlags>>, sqlx::Error> {
        let mut store = Store::open(&self.db).await?;
        let at = i64::try_from(from.at).unwrap_or(i64::MAX);
        let found = async {
            store.begin_snapshot().await?;
            let Some((timestamp, role)) = store.row_stamp(self.id.as_ref(), at).await? else {
                return Ok(None);
            };
            if checkpoint(at, timestamp, &role) != from {
                return Ok(None);
            }
            let flags = store.flags_upto(self.id.as_ref(), at).await?;
            store.end_snapshot().await?;
            Ok(Some(flags))
        }
        .await;
        store.close().await;
        found
    }

    /// One page of the session after row `after`, with its `sessions` row, and when `recheck`,
    /// which rows up to `after` are rewound now, all from one snapshot: an `/undo` committed
    /// between reading the page and the flags would show the page's rows live and their own
    /// turn's earlier rows rewound, and hang them from before their turn. `yielded` is the
    /// checkpoint of the row the read last yielded, which has to be there still.
    async fn page(
        &self,
        after: i64,
        recheck: bool,
        yielded: Option<Checkpoint>,
    ) -> Result<Page, sqlx::Error> {
        let mut store = Store::open(&self.db).await?;
        let read = async {
            store.begin_snapshot().await?;
            if let Some(yielded) = yielded {
                let at = i64::try_from(yielded.at).unwrap_or(i64::MAX);
                let stamp = store.row_stamp(self.id.as_ref(), at).await?;
                let same = stamp.is_some_and(|(timestamp, role)| {
                    checkpoint(at, timestamp, &role).digest == yielded.digest
                });
                if !same {
                    store.end_snapshot().await?;
                    return Ok(Page {
                        session: None,
                        rows: Vec::new(),
                        rewound: None,
                        moved: true,
                    });
                }
            }
            let session = store.session(self.id.as_ref()).await?;
            let rows = store.messages_after(self.id.as_ref(), after, PAGE).await?;
            let rewound = if recheck && !rows.is_empty() {
                Some(store.rewound_upto(self.id.as_ref(), after).await?)
            } else {
                None
            };
            store.end_snapshot().await?;
            Ok(Page {
                session,
                rows,
                rewound,
                moved: false,
            })
        }
        .await;
        store.close().await;
        read
    }

    /// The messages of the session from `from`, each with the checkpoint just past it, following
    /// the session when the listener gave it a wake.
    fn messages_at(
        self,
        from: Option<Checkpoint>,
    ) -> impl Stream<Item = Result<(Checkpoint, HermesMessage), MessageError>> + Send + 'static
    {
        async_stream::stream! {
            let mut reading = Reading::default();
            // How many times this stream started the session over: its checkpoints' generation.
            let mut starts = 0u32;
            // The checkpoint of the last row yielded, which a title row goes with.
            let mut last = None;
            if let Some(from) = from {
                match self.resume_at(from).await {
                    Ok(Some(skipped)) => {
                        reading.prime(&skipped);
                        last = Some(from);
                    }
                    Ok(None) => starts = 1,
                    Err(err) => {
                        tracing::warn!(db = %self.db.display(), %err, "can't read a Hermes session");
                        yield Err(MessageError::Incomplete);
                        starts = 1;
                    }
                }
            }
            let mut changes = self.changes.clone();
            loop {
                // Every page until the session's end as it stands.
                loop {
                    let page = match self.page(reading.cursor, !reading.yielded.is_empty(), last).await {
                        Ok(page) => page,
                        Err(err) => {
                            tracing::warn!(db = %self.db.display(), %err, "can't read a Hermes session");
                            yield Err(MessageError::Incomplete);
                            break;
                        }
                    };
                    if page.moved {
                        tracing::debug!(db = %self.db.display(), "a Hermes store changed under a read; reading it again");
                        reading = Reading::default();
                        last = None;
                        starts += 1;
                        continue;
                    }
                    let Some(session) = page.session else { break };
                    if let Some(rewound) = &page.rewound {
                        reading.rewind(rewound);
                    }
                    let context = Arc::new(Context::of(&session));
                    let full = page.rows.len() >= PAGE as usize;
                    for row in page.rows {
                        reading.cursor = row.id;
                        if !reading.keeps(&flags_of(&row)) {
                            continue;
                        }
                        let parent = reading.parent(row.live);
                        reading.push(row.id, row.live, row_key(row.id, row.message_uid.as_deref()));
                        let at = Checkpoint {
                            generation: starts,
                            ..checkpoint(row.id, row.timestamp, &row.role)
                        };
                        last = Some(at);
                        yield Ok((at, HermesMessage::row(row, parent, Arc::clone(&context))));
                    }
                    let title = title_of(&session);
                    if title != reading.title
                        && let Some(at) = last
                        && (title.is_some() || reading.title.is_some())
                    {
                        let change = title.clone().unwrap_or(TitleChange {
                            source: reading.title.as_ref().map_or(TitleSource::Named, |t| t.source),
                            text: None,
                        });
                        let when = time_of(session.touched());
                        yield Ok((at, HermesMessage::title_row(change, reading.titled, when, &context)));
                        reading.titled += 1;
                        reading.title = title;
                    }
                    if !full {
                        break;
                    }
                }
                let Some(wake) = changes.as_mut() else { break };
                if wake.changed().await.is_err() {
                    // The listener is gone: the rows written since it last looked are read once
                    // more, then the session ends.
                    changes = None;
                }
            }
        }
    }
}

impl Session for HermesSession {
    type Message = HermesMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages_from(
        self,
        from: Option<Checkpoint>,
    ) -> impl Stream<Item = Result<(Checkpoint, HermesMessage), MessageError>> + Send + 'static
    {
        self.messages_at(from)
    }

    fn read(&self) -> impl Stream<Item = Result<HermesMessage, MessageError>> + Send + 'static {
        Self {
            changes: None,
            ..self.clone()
        }
        .messages_at(None)
        .map(|item| item.map(|(_, message)| message))
    }

    /// Once Hermes records anything of the session after `since`: at once if the store said so
    /// when it was listed, else when the listener next sees it change.
    async fn changed_after(mut self, since: SystemTime) -> Option<Self> {
        // Hermes's clock and ours may disagree by a little; a row written just after `since` can
        // be stamped just before.
        const SLACK: Duration = Duration::from_secs(2);
        let since = since.checked_sub(SLACK).unwrap_or(since);
        if self.touched.is_none_or(|at| at >= since) {
            return Some(self);
        }
        let changed = self.changes.as_mut()?.changed().await;
        changed.ok().map(|()| self)
    }
}

/// What a [`HermesMessage`] is.
#[derive(Debug, Clone)]
enum Kind {
    Row(MessageRow),
    /// A title row (see the module docs), with its id and time.
    Title {
        id: String,
        change: TitleChange,
        at: Option<OffsetDateTime>,
    },
}

/// One conversation row of a Hermes session, or a title row the reader made.
#[derive(Debug, Clone)]
pub struct HermesMessage {
    kind: Kind,
    /// The key ([`row_key`]) of the row this one hangs from.
    parent: Option<String>,
    context: Arc<Context>,
}

impl HermesMessage {
    fn row(row: MessageRow, parent: Option<String>, context: Arc<Context>) -> Self {
        Self {
            kind: Kind::Row(row),
            parent,
            context,
        }
    }

    /// The `nth` title row of a read: the same title read again from the start is the same row,
    /// and a title the session is renamed back to a new one.
    fn title_row(
        change: TitleChange,
        nth: u32,
        at: Option<OffsetDateTime>,
        context: &Context,
    ) -> Self {
        let mut keyed = change.text.as_deref().unwrap_or_default().as_bytes().to_vec();
        keyed.extend_from_slice(&nth.to_le_bytes());
        let id = format!("{SYNTHETIC}title-{:016x}", xxh3_64(&keyed));
        Self {
            kind: Kind::Title { id, change, at },
            parent: None,
            context: Arc::new(Context {
                continues: context.continues.clone(),
                ..Context::default()
            }),
        }
    }

    fn row_ref(&self) -> Option<&MessageRow> {
        match &self.kind {
            Kind::Row(row) => Some(row),
            Kind::Title { .. } => None,
        }
    }

    fn is_assistant(&self) -> bool {
        self.row_ref().is_some_and(|row| row.role == "assistant")
    }
}

/// `content` as Hermes means it: multimodal parts (text parts as text, the rest as they are) when
/// it carries [`JSON_CONTENT`], else plain text. Empty text is nothing.
fn decode(content: &str) -> Vec<Content> {
    let Some(json) = content.strip_prefix(JSON_CONTENT) else {
        return (!content.is_empty())
            .then(|| Content::Text(content.to_owned()))
            .into_iter()
            .collect();
    };
    match serde_json::from_str::<Value>(json) {
        // A plain string that happened to start with the prefix, wrapped so it can't be taken
        // for parts.
        Ok(Value::String(text)) => decode_text(text),
        Ok(Value::Array(parts)) => parts
            .into_iter()
            .flat_map(|part| match part["text"].as_str() {
                Some(text) if part["type"] == "text" => decode_text(text.to_owned()),
                _ => vec![Content::Other(part)],
            })
            .collect(),
        Ok(other) => vec![Content::Other(other)],
        Err(_) => vec![Content::Text(content.to_owned())],
    }
}

fn decode_text(text: String) -> Vec<Content> {
    (!text.is_empty()).then_some(Content::Text(text)).into_iter().collect()
}

/// Whether a tool result says its call failed (see the module docs): only the JSON value it
/// starts with is read, since Hermes appends context after it.
fn failed(output: &str) -> bool {
    let first = serde_json::Deserializer::from_str(output).into_iter::<Value>().next();
    let Some(Ok(Value::Object(result))) = first else {
        return false;
    };
    let exit = result.get("exit_code").and_then(Value::as_i64).is_some_and(|code| code != 0);
    let error = result.get("error").is_some_and(|error| !error.is_null());
    let status = matches!(result.get("status").and_then(Value::as_str), Some("blocked" | "error"));
    exit || error || status
}

/// The calls of an assistant row's `tool_calls` (OpenAI's shape).
fn calls(raw: &str) -> Vec<Content> {
    let Ok(Value::Array(calls)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    calls
        .into_iter()
        .map(|call| {
            let id = call["id"].as_str().or_else(|| call["call_id"].as_str()).unwrap_or_default();
            let function = &call["function"];
            let input = match &function["arguments"] {
                Value::String(args) => {
                    serde_json::from_str(args).unwrap_or_else(|_| Value::String(args.clone()))
                }
                other => other.clone(),
            };
            Content::ToolUse(ToolUse {
                id: ToolCallId::from(id.to_owned()),
                name: function["name"].as_str().unwrap_or_default().to_owned(),
                input,
            })
        })
        .collect()
}

/// Whether `row` is a turn Hermes stored in place of a reply it never got.
fn failed_turn(row: &MessageRow) -> bool {
    row.display_kind.as_deref() == Some("failed_turn")
}

impl Message for HermesMessage {
    fn id(&self) -> Option<MessageId> {
        Some(MessageId::from(match &self.kind {
            Kind::Row(row) => row_key(row.id, row.message_uid.as_deref()),
            Kind::Title { id, .. } => id.clone(),
        }))
    }

    fn role(&self) -> Role {
        match &self.kind {
            Kind::Row(row) => match row.role.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                "system" => Role::System,
                other => Role::Other(other.to_owned()),
            },
            Kind::Title { .. } => Role::Other(TITLE_ROW.to_owned()),
        }
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        match &self.kind {
            Kind::Row(row) => time_of(row.timestamp),
            Kind::Title { at, .. } => *at,
        }
    }

    fn content(&self) -> Vec<Content> {
        let Kind::Row(row) = &self.kind else {
            return Vec::new();
        };
        let content = row.content.as_deref().unwrap_or_default();
        match row.role.as_str() {
            "assistant" if failed_turn(row) => {
                let error = metadata(row)["error"].as_str().map(str::to_owned);
                vec![Content::Error(error.unwrap_or_else(|| content.to_owned()))]
            }
            "assistant" => {
                let reasoning =
                    row.reasoning.clone().filter(|r| !r.is_empty()).map(Content::Reasoning);
                let calls = row.tool_calls.as_deref().map(calls).unwrap_or_default();
                reasoning.into_iter().chain(decode(content)).chain(calls).collect()
            }
            "tool" => {
                let output = match decode(content).as_slice() {
                    [] => Value::String(String::new()),
                    [Content::Text(text)] => Value::String(text.clone()),
                    parts => Value::Array(
                        parts
                            .iter()
                            .map(|part| match part {
                                Content::Text(text) => {
                                    serde_json::json!({"type": "text", "text": text})
                                }
                                Content::Other(other) => other.clone(),
                                _ => Value::Null,
                            })
                            .collect(),
                    ),
                };
                vec![Content::ToolResult(ToolResult {
                    call: ToolCallId::from(row.tool_call_id.clone().unwrap_or_default()),
                    output,
                    error: failed(content),
                })]
            }
            _ => decode(content),
        }
    }

    fn model(&self) -> Option<String> {
        self.is_assistant().then(|| self.context.model.clone()).flatten()
    }

    fn provider(&self) -> Option<String> {
        self.is_assistant().then(|| self.context.provider.clone()).flatten()
    }

    fn stop_reason(&self) -> Option<StopReason> {
        let row = self.row_ref().filter(|row| row.role == "assistant")?;
        if failed_turn(row) {
            return Some(StopReason::Error);
        }
        Some(match row.finish_reason.as_deref()? {
            "stop" => StopReason::EndTurn,
            "tool_calls" => StopReason::ToolUse,
            "length" => StopReason::MaxTokens,
            "content_filter" => StopReason::Refusal,
            other => StopReason::Other(other.to_owned()),
        })
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.row_ref().and(self.context.cwd.clone())
    }

    fn parent_id(&self) -> Option<MessageId> {
        self.parent.clone().map(MessageId::from)
    }

    fn parent_session(&self) -> Option<SessionId> {
        self.context.continues.clone()
    }

    fn parent_kind(&self) -> Option<ParentKind> {
        self.context.continues.as_ref().map(|_| ParentKind::Continuation)
    }

    fn title(&self) -> Option<TitleChange> {
        match &self.kind {
            Kind::Title { change, .. } => Some(change.clone()),
            Kind::Row(_) => None,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
