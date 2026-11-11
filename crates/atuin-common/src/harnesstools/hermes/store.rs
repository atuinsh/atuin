//! One Hermes `state.db`, opened for one pass and closed after it.
//!
//! Hermes refuses `hermes sessions optimize-storage`, repair and journal-mode changes, and skips
//! its auto-VACUUM, while any other process holds `state.db` or a sidecar. The daemon runs all
//! day, so nothing here keeps a connection: a [`Store`] lives for one read and is dropped, and the
//! next read opens the file again (which also picks up a file Hermes replaced).
//!
//! The open is read-only, with a busy timeout, and retried on `SQLITE_IOERR`, as Hermes's own
//! readers retry through a WAL reset. When the last Hermes connection closes, SQLite removes the
//! `-wal` and `-shm` files. The SQLite atuin bundles still opens the store read-only then (and
//! makes the sidecars, as Hermes's own Python does), but some builds refuse (Apple's
//! `sqlite3 -readonly`: `SQLITE_CANTOPEN`), so that error falls back to a read-write open with
//! `PRAGMA query_only`, which can't write rows either.
//!
//! Columns are selected by name, never `*` (`codex_reasoning_items` can hold 120 KB), and the
//! optional ones are probed first: Hermes only ever adds columns, so a store an older Hermes wrote
//! reads as far as its columns allow, and a column it lacks reads as `NULL`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteRow};
use sqlx::{AssertSqlSafe, Connection, Row, Sqlite};

use crate::db::{query, query_scalar};
use crate::path::PathBufExt as _;

/// The store's file name, in a Hermes home and in each profile's.
pub const DB_FILE: &str = "state.db";

/// The write-ahead log beside [`DB_FILE`], whose modification time moves on every commit.
pub const WAL_FILE: &str = "state.db-wal";

/// The directory under a Hermes home holding one home per profile (`hermes -p <name>`).
pub const PROFILES_DIR: &str = "profiles";

/// How long a statement waits on a Hermes writer's lock, as Hermes's own readers wait.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// How often, and how far apart, an open that hits `SQLITE_IOERR` is tried again: Hermes's own
/// readers retry 3 times, 50 ms apart, through a WAL checkpoint resetting the log.
const IOERR_RETRIES: u32 = 3;
const IOERR_BACKOFF: Duration = Duration::from_millis(50);

/// The sessions that are coding work, which become threads: Hermes's CLI, TUI, ACP and one-shot
/// runs, which before v0.22 were stored under their own `source`, and since are `local` sessions
/// with the surface in `created_source`. Gateway platforms (Telegram and the rest), cron, kanban
/// and tool runs are not. Subagents are threads by the session they work for (see
/// [`Store::sessions`]).
const CODING_SOURCES: &str = "'cli', 'tui', 'acp', 'oneshot'";
const CODING_LOCAL_SOURCES: &str = "'cli', 'tui', 'acp', 'desktop'";

/// How far a chain of subagents is followed up to the session it works for: deeper than Hermes
/// nests them (its `delegation.max_spawn_depth` defaults to 1), so only a loop in a broken store
/// reaches it.
const MAX_DELEGATION_DEPTH: u32 = 64;

/// The sessions of `rows` that are threads: those marked coding, and each subagent whose
/// delegator is one, at any depth. Every row is settled once, with the chain that led to it, so
/// this is linear in the rows; a loop of delegators, or one leading out of `rows` (a gateway
/// chat, a session Hermes deleted), is no thread.
fn threads(rows: Vec<(bool, SessionRow)>) -> Vec<SessionRow> {
    let index: HashMap<&str, usize> =
        rows.iter().enumerate().map(|(i, (_, row))| (row.id.as_str(), i)).collect();
    let mut settled: Vec<Option<bool>> =
        rows.iter().map(|(coding, _)| coding.then_some(true)).collect();
    for start in 0..rows.len() {
        let mut chain = Vec::new();
        let mut at = Some(start);
        let thread = loop {
            let Some(i) = at else {
                break false;
            };
            if let Some(thread) = settled[i] {
                break thread;
            }
            if chain.contains(&i) {
                break false;
            }
            chain.push(i);
            at = rows[i].1.delegated_by.as_deref().and_then(|d| index.get(d).copied());
        };
        for i in chain {
            settled[i] = Some(thread);
        }
    }
    drop(index);
    rows.into_iter()
        .zip(settled)
        .filter_map(|((_, row), thread)| thread.unwrap_or(false).then_some(row))
        .collect()
}

/// The stores under the Hermes home `root`: its own, then each profile's. An `Err` is a
/// `profiles` directory, or an entry of it, that can't be read, so a scan can say it is partial;
/// a home with no `profiles` directory has no profiles.
pub fn stores(root: &Path) -> Vec<std::io::Result<PathBuf>> {
    let own = root.join(DB_FILE);
    let mut found: Vec<std::io::Result<PathBuf>> =
        std::iter::once(own).filter(|p| p.is_file()).map(Ok).collect();
    let entries = match std::fs::read_dir(root.join(PROFILES_DIR)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return found,
        Err(err) => {
            found.push(Err(err));
            return found;
        }
    };
    let mut profiles: Vec<std::io::Result<PathBuf>> = entries
        .map(|entry| entry.map(|entry| entry.path().joined(DB_FILE)))
        .filter(|path| path.as_ref().map_or(true, |path| path.is_file()))
        .collect();
    profiles.sort_by(|a, b| match (a, b) {
        (Ok(a), Ok(b)) => a.cmp(b),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => std::cmp::Ordering::Equal,
    });
    found.extend(profiles);
    found
}

/// The profile whose store `db` is (`<root>/profiles/<name>/state.db`), `None` for a home's own.
pub fn profile_of(db: &Path) -> Option<String> {
    let home = db.parent()?;
    let profiles = home.parent()?;
    (profiles.file_name()? == PROFILES_DIR)
        .then(|| home.file_name()?.to_str().map(str::to_owned))?
}

/// Which optional columns this store has.
#[derive(Debug, Clone)]
struct Columns {
    sessions: HashSet<String>,
    messages: HashSet<String>,
    tables: HashSet<String>,
}

impl Columns {
    /// `table.name` when the store has it, else `NULL` under the same name.
    fn pick(&self, table: &str, alias: &str, name: &str) -> String {
        let has = match table {
            "sessions" => self.sessions.contains(name),
            _ => self.messages.contains(name),
        };
        if has {
            format!("{alias}.{name} AS {name}")
        } else {
            format!("NULL AS {name}")
        }
    }
}

/// An open store, for one pass.
pub struct Store {
    conn: SqliteConnection,
    columns: Columns,
}

/// What a store says of one of its sessions.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub title: Option<String>,
    pub title_source: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub model_config: Option<String>,
    pub parent: Option<String>,
    /// The parent's `end_reason`: `compression` makes this session its continuation.
    pub parent_end_reason: Option<String>,
    /// For a subagent (`delegate_task`'s child), the session that delegated to it.
    pub delegated_by: Option<String>,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub last_activity_at: Option<f64>,
    /// The newest row of the session, `None` while it has none.
    pub tip: Option<i64>,
    /// When the newest row was written.
    pub tip_at: Option<f64>,
}

impl SessionRow {
    /// The latest moment Hermes recorded doing anything in the session.
    pub fn touched(&self) -> f64 {
        [Some(self.started_at), self.last_activity_at, self.tip_at, self.ended_at]
            .into_iter()
            .flatten()
            .fold(f64::MIN, f64::max)
    }

    /// The provider Hermes routed the session's model calls to.
    pub fn provider(&self) -> Option<String> {
        let config: serde_json::Value = serde_json::from_str(self.model_config.as_deref()?).ok()?;
        config["gateway_runtime"]["provider"].as_str().map(str::to_owned)
    }
}

/// One `messages` row, as the reader takes it.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageRow {
    pub id: i64,
    pub role: String,
    pub content: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_calls: Option<String>,
    pub tool_name: Option<String>,
    pub timestamp: f64,
    pub finish_reason: Option<String>,
    pub reasoning: Option<String>,
    pub observed: bool,
    pub compressed_summary: bool,
    /// Whether the row is in the conversation Hermes shows: active, compacted (summarized away
    /// but still history), or copied into the live transcript under another id.
    pub live: bool,
    pub display_kind: Option<String>,
    pub display_metadata: Option<String>,
    pub message_uid: Option<String>,
}

/// The columns of `messages` the reader needs to decide whether a row is kept, without its
/// content: for rebuilding what a read resumed past a checkpoint had already seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFlags {
    pub id: i64,
    pub role: String,
    pub live: bool,
    pub observed: bool,
    pub compressed_summary: bool,
    pub display_kind: Option<String>,
    pub model_only: bool,
    pub message_uid: Option<String>,
    /// Whether the row has anything to show: content, reasoning or tool calls.
    pub has_body: bool,
}

/// Whether `err` is SQLite's `SQLITE_IOERR`, in any of its extended forms.
fn is_ioerr(err: &sqlx::Error) -> bool {
    sqlite_code(err).is_some_and(|code| code & 0xff == 10)
}

/// Whether `err` is SQLite's `SQLITE_CANTOPEN`.
fn is_cantopen(err: &sqlx::Error) -> bool {
    sqlite_code(err).is_some_and(|code| code & 0xff == 14)
}

fn sqlite_code(err: &sqlx::Error) -> Option<i64> {
    err.as_database_error()?.code()?.parse().ok()
}

impl Store {
    /// Open the store at `db` for reads (see the module docs).
    pub async fn open(db: &Path) -> Result<Self, sqlx::Error> {
        let mut tries = 0;
        loop {
            match Self::open_once(db).await {
                Err(err) if is_ioerr(&err) && tries < IOERR_RETRIES => {
                    tries += 1;
                    tokio::time::sleep(IOERR_BACKOFF).await;
                }
                done => return done,
            }
        }
    }

    async fn open_once(db: &Path) -> Result<Self, sqlx::Error> {
        let read_only = SqliteConnectOptions::new()
            .filename(db)
            .create_if_missing(false)
            .read_only(true)
            .busy_timeout(BUSY_TIMEOUT);
        // SQLite opens lazily: a store it can't read without sidecars fails on the first
        // statement, not the connect.
        let opened = match SqliteConnection::connect_with(&read_only).await {
            Ok(mut conn) => Self::probe(&mut conn).await.map(|columns| Self { conn, columns }),
            Err(err) => Err(err),
        };
        match opened {
            Err(err) if is_cantopen(&err) => {
                tracing::debug!(db = %db.display(), %err, "read-only open failed; opening query-only");
                let query_only = SqliteConnectOptions::new()
                    .filename(db)
                    .create_if_missing(false)
                    .busy_timeout(BUSY_TIMEOUT)
                    .pragma("query_only", "1");
                let mut conn = SqliteConnection::connect_with(&query_only).await?;
                let columns = Self::probe(&mut conn).await?;
                Ok(Self { conn, columns })
            }
            opened => opened,
        }
    }

    async fn probe(conn: &mut SqliteConnection) -> Result<Columns, sqlx::Error> {
        let names = |sql: &'static str| query_scalar::<Sqlite, String>(sql);
        let tables = names("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(&mut *conn)
            .await?;
        let sessions =
            names("SELECT name FROM pragma_table_info('sessions')").fetch_all(&mut *conn).await?;
        let messages =
            names("SELECT name FROM pragma_table_info('messages')").fetch_all(&mut *conn).await?;
        Ok(Columns {
            sessions: sessions.into_iter().collect(),
            messages: messages.into_iter().collect(),
            tables: tables.into_iter().collect(),
        })
    }

    /// Start a read transaction: every query until [`Self::end_snapshot`] sees the store as one
    /// commit left it, where each would otherwise see the latest (WAL readers take a snapshot
    /// per statement outside a transaction). Deferred, so it takes no lock a Hermes writer waits
    /// on.
    pub async fn begin_snapshot(&mut self) -> Result<(), sqlx::Error> {
        query::<Sqlite>("BEGIN DEFERRED").execute(&mut self.conn).await.map(drop)
    }

    /// End the read transaction [`Self::begin_snapshot`] started.
    pub async fn end_snapshot(&mut self) -> Result<(), sqlx::Error> {
        query::<Sqlite>("COMMIT").execute(&mut self.conn).await.map(drop)
    }

    /// Close the connection, so Hermes sees nobody holding the store. An open read transaction
    /// goes with it.
    pub async fn close(self) {
        if let Err(err) = self.conn.close().await {
            tracing::debug!(%err, "closing a Hermes store failed");
        }
    }

    /// The SQL condition (on `sessions` as `s`) for a coding session (see [`CODING_SOURCES`]),
    /// subagents left out.
    fn coding(&self) -> String {
        let local = if self.columns.sessions.contains("created_source") {
            format!(" OR (s.source = 'local' AND s.created_source IN ({CODING_LOCAL_SOURCES}))")
        } else {
            String::new()
        };
        let delegate = if self.columns.sessions.contains("model_config") {
            " AND (CASE WHEN json_valid(s.model_config) THEN json_extract(s.model_config, \
             '$._delegate_from') END) IS NULL"
        } else {
            ""
        };
        format!("((s.source IN ({CODING_SOURCES}){local}){delegate})")
    }

    /// The `live` column of a `messages` row (as `m`) (see [`MessageRow::live`]), in a query
    /// whose `?1` is the row's session.
    ///
    /// A `CASE`, so only a row that isn't live itself looks for a copy that is: per row, that
    /// lookup costs a scan of the session. A live copy is found through Hermes's own partial index
    /// (`idx_messages_session_uid`, on `(session_id, message_uid) WHERE active = 1`), which a
    /// condition on `active OR compacted` can't use; compacted copies are gathered once per query.
    fn live(&self) -> String {
        let has = |name: &str| self.columns.messages.contains(name);
        let own = if has("compacted") {
            "m.active = 1 OR m.compacted = 1"
        } else {
            "m.active = 1"
        };
        let mut live = format!("CASE WHEN {own} THEN 1");
        if has("message_uid") {
            live.push_str(
                " WHEN EXISTS (SELECT 1 FROM messages c WHERE c.session_id = m.session_id AND \
                 c.message_uid = m.message_uid AND c.active = 1 AND c.id <> m.id) THEN 1",
            );
            if has("compacted") {
                live.push_str(
                    " WHEN m.message_uid IN (SELECT c.message_uid FROM messages c WHERE \
                     c.session_id = ?1 AND c.compacted = 1 AND c.message_uid IS NOT NULL) THEN 1",
                );
            }
        }
        live.push_str(" ELSE 0 END");
        live
    }

    fn session_select(&self) -> String {
        self.session_select_with("")
    }

    /// [`Self::session_select`] with `extra` (`, <expr> AS <name>`...) among its columns.
    fn session_select_with(&self, extra: &str) -> String {
        let pick = |name: &str| self.columns.pick("sessions", "s", name);
        let parent_end = if self.columns.sessions.contains("parent_session_id")
            && self.columns.sessions.contains("end_reason")
        {
            "(SELECT p.end_reason FROM sessions p WHERE p.id = s.parent_session_id) AS \
             parent_end_reason"
        } else {
            "NULL AS parent_end_reason"
        };
        let delegated_by = format!(
            "CASE WHEN s.source = 'subagent' THEN {} END AS delegated_by",
            self.delegator()
        );
        format!(
            "SELECT s.id AS id, {}, {}, {}, {}, {}, {}, {parent_end}, {delegated_by}, \
             s.started_at AS started_at, {}, {}, (SELECT max(m.id) FROM messages m WHERE \
             m.session_id = s.id) AS tip, (SELECT max(m.timestamp) FROM messages m WHERE \
             m.session_id = s.id) AS tip_at{extra} FROM sessions s",
            pick("title"),
            pick("title_source"),
            pick("cwd"),
            pick("model"),
            pick("model_config"),
            pick("parent_session_id"),
            pick("ended_at"),
            pick("last_activity_at"),
        )
    }

    fn session_of(row: &SqliteRow) -> Result<SessionRow, sqlx::Error> {
        Ok(SessionRow {
            id: row.try_get("id")?,
            title: row.try_get("title")?,
            title_source: row.try_get("title_source")?,
            cwd: row.try_get("cwd")?,
            model: row.try_get("model")?,
            model_config: row.try_get("model_config")?,
            parent: row.try_get("parent_session_id")?,
            parent_end_reason: row.try_get("parent_end_reason")?,
            delegated_by: row.try_get("delegated_by")?,
            started_at: row.try_get("started_at")?,
            ended_at: row.try_get("ended_at")?,
            last_activity_at: row.try_get("last_activity_at")?,
            tip: row.try_get("tip")?,
            tip_at: row.try_get("tip_at")?,
        })
    }

    /// The SQL (on `sessions` as `s`) for the session a subagent's `s` works for: Hermes's own
    /// rule (its v33 migration), `model_config._delegate_from`, else `parent_session_id`. A
    /// subagent whose parent is gone names none Hermes keeps (`__orphaned__`).
    fn delegator(&self) -> String {
        let has = |name: &str| self.columns.sessions.contains(name);
        let delegate = has("model_config").then_some(
            "(CASE WHEN json_valid(s.model_config) THEN json_extract(s.model_config, \
             '$._delegate_from') END)",
        );
        let parent = has("parent_session_id").then_some("s.parent_session_id");
        match (delegate, parent) {
            (Some(delegate), Some(parent)) => format!("COALESCE({delegate}, {parent})"),
            (Some(only), None) | (None, Some(only)) => only.to_owned(),
            (None, None) => "NULL".to_owned(),
        }
    }

    /// Every session in the store that is a thread: each coding session (see
    /// [`CODING_SOURCES`]), and each subagent working, at any depth, for one (see [`threads`]).
    /// A gateway chat's or a cron job's subagents are not.
    pub async fn sessions(&mut self) -> Result<Vec<SessionRow>, sqlx::Error> {
        let coding = self.coding();
        let sql = format!(
            "{} WHERE {coding} OR s.source = 'subagent' ORDER BY s.started_at, s.id",
            self.session_select_with(&format!(", {coding} AS is_coding")),
        );
        let rows = query::<Sqlite>(AssertSqlSafe(sql)).fetch_all(&mut self.conn).await?;
        let rows = rows
            .iter()
            .map(|row| Ok((row.try_get::<bool, _>("is_coding")?, Self::session_of(row)?)))
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        Ok(threads(rows))
    }

    /// Session `id`, `None` when the store has no such session or it is no thread. It walks up
    /// from `id` to the session it works for, one lookup a step, rather than find every thread.
    pub async fn session(&mut self, id: &str) -> Result<Option<SessionRow>, sqlx::Error> {
        let up = format!(
            "WITH RECURSIVE up(id, delegator, depth) AS (SELECT s.id, CASE WHEN s.source = \
             'subagent' THEN {delegator} END, 0 FROM sessions s WHERE s.id = ?1 UNION ALL SELECT \
             s.id, CASE WHEN s.source = 'subagent' THEN {delegator} END, up.depth + 1 FROM \
             sessions s JOIN up ON s.id = up.delegator WHERE up.depth < {MAX_DELEGATION_DEPTH}) ",
            delegator = self.delegator(),
        );
        let sql = format!(
            "{up}{} WHERE s.id = ?1 AND EXISTS (SELECT 1 FROM up JOIN sessions s ON s.id = up.id \
             WHERE up.delegator IS NULL AND {})",
            self.session_select(),
            self.coding()
        );
        let row =
            query::<Sqlite>(AssertSqlSafe(sql)).bind(id).fetch_optional(&mut self.conn).await?;
        row.as_ref().map(Self::session_of).transpose()
    }

    /// Session `id`, whatever kind it is, `None` when the store has no such session.
    pub async fn any_session(&mut self, id: &str) -> Result<Option<SessionRow>, sqlx::Error> {
        let sql = format!("{} WHERE s.id = ?1", self.session_select());
        let row =
            query::<Sqlite>(AssertSqlSafe(sql)).bind(id).fetch_optional(&mut self.conn).await?;
        row.as_ref().map(Self::session_of).transpose()
    }

    /// Whether the store holds session `id`, whatever kind it is.
    pub async fn holds(&mut self, id: &str) -> Result<bool, sqlx::Error> {
        let found = query_scalar::<Sqlite, i64>("SELECT 1 FROM sessions WHERE id = ?1 LIMIT 1")
            .bind(id)
            .fetch_optional(&mut self.conn)
            .await?;
        Ok(found.is_some())
    }

    fn message_select(&self) -> String {
        let pick = |name: &str| self.columns.pick("messages", "m", name);
        let flag = |name: &str, alias: &str| {
            if self.columns.messages.contains(name) {
                format!("COALESCE(m.{name}, 0) = 1 AS {alias}")
            } else {
                format!("0 AS {alias}")
            }
        };
        format!(
            "SELECT m.id AS id, m.role AS role, m.content AS content, m.tool_call_id AS \
             tool_call_id, m.tool_calls AS tool_calls, {}, m.timestamp AS timestamp, {}, {}, {}, \
             {}, {} AS live, {}, {}, {} FROM messages m",
            pick("tool_name"),
            pick("finish_reason"),
            pick("reasoning"),
            flag("observed", "observed"),
            flag("_compressed_summary", "compressed_summary"),
            self.live(),
            pick("display_kind"),
            pick("display_metadata"),
            pick("message_uid"),
        )
    }

    /// Up to `limit` rows of session `session` after row `after`, oldest first.
    pub async fn messages_after(
        &mut self,
        session: &str,
        after: i64,
        limit: u32,
    ) -> Result<Vec<MessageRow>, sqlx::Error> {
        let sql = format!(
            "{} WHERE m.session_id = ?1 AND m.id > ?2 ORDER BY m.id LIMIT ?3",
            self.message_select()
        );
        let rows = query::<Sqlite>(AssertSqlSafe(sql))
            .bind(session)
            .bind(after)
            .bind(limit)
            .fetch_all(&mut self.conn)
            .await?;
        rows.iter()
            .map(|row| {
                Ok(MessageRow {
                    id: row.try_get("id")?,
                    role: row.try_get("role")?,
                    content: row.try_get("content")?,
                    tool_call_id: row.try_get("tool_call_id")?,
                    tool_calls: row.try_get("tool_calls")?,
                    tool_name: row.try_get("tool_name")?,
                    timestamp: row.try_get("timestamp")?,
                    finish_reason: row.try_get("finish_reason")?,
                    reasoning: row.try_get("reasoning")?,
                    observed: row.try_get("observed")?,
                    compressed_summary: row.try_get("compressed_summary")?,
                    live: row.try_get("live")?,
                    display_kind: row.try_get("display_kind")?,
                    display_metadata: row.try_get("display_metadata")?,
                    message_uid: row.try_get("message_uid")?,
                })
            })
            .collect()
    }

    /// The flags of every row of session `session` up to row `upto`, oldest first.
    pub async fn flags_upto(
        &mut self,
        session: &str,
        upto: i64,
    ) -> Result<Vec<MessageFlags>, sqlx::Error> {
        let has = |name: &str| self.columns.messages.contains(name);
        let flag = |name: &str, alias: &str| {
            if has(name) {
                format!("COALESCE(m.{name}, 0) = 1 AS {alias}")
            } else {
                format!("0 AS {alias}")
            }
        };
        let model_only = if has("display_metadata") {
            "COALESCE(CASE WHEN json_valid(m.display_metadata) THEN \
             json_extract(m.display_metadata, '$.model_only') END, 0) = 1 AS model_only"
        } else {
            "0 AS model_only"
        };
        let reasoning = if has("reasoning") {
            "OR length(m.reasoning) > 0 "
        } else {
            ""
        };
        let sql = format!(
            "SELECT m.id AS id, m.role AS role, {} AS live, {}, {}, {}, {model_only}, {}, \
             (length(m.content) > 0 {reasoning}OR length(m.tool_calls) > 2) AS has_body FROM \
             messages m WHERE m.session_id = ?1 AND m.id <= ?2 ORDER BY m.id",
            self.live(),
            flag("observed", "observed"),
            flag("_compressed_summary", "compressed_summary"),
            self.columns.pick("messages", "m", "display_kind"),
            self.columns.pick("messages", "m", "message_uid"),
        );
        let rows = query::<Sqlite>(AssertSqlSafe(sql))
            .bind(session)
            .bind(upto)
            .fetch_all(&mut self.conn)
            .await?;
        rows.iter()
            .map(|row| {
                Ok(MessageFlags {
                    id: row.try_get("id")?,
                    role: row.try_get("role")?,
                    live: row.try_get("live")?,
                    observed: row.try_get("observed")?,
                    compressed_summary: row.try_get("compressed_summary")?,
                    display_kind: row.try_get("display_kind")?,
                    model_only: row.try_get("model_only")?,
                    message_uid: row.try_get("message_uid")?,
                    has_body: row.try_get::<Option<bool>, _>("has_body")?.unwrap_or(false),
                })
            })
            .collect()
    }

    /// The ids of session `session`'s rows up to `upto` that are no longer live (see
    /// [`MessageRow::live`]): rewound by `/undo`, `/retry` or an edit.
    pub async fn rewound_upto(
        &mut self,
        session: &str,
        upto: i64,
    ) -> Result<HashSet<i64>, sqlx::Error> {
        let sql = format!(
            "SELECT m.id FROM messages m WHERE m.session_id = ?1 AND m.id <= ?2 AND NOT {}",
            self.live()
        );
        let ids = query_scalar::<Sqlite, i64>(AssertSqlSafe(sql))
            .bind(session)
            .bind(upto)
            .fetch_all(&mut self.conn)
            .await?;
        Ok(ids.into_iter().collect())
    }

    /// The timestamp and role of row `id` of session `session`, `None` when it is gone.
    pub async fn row_stamp(
        &mut self,
        session: &str,
        id: i64,
    ) -> Result<Option<(f64, String)>, sqlx::Error> {
        let row = query::<Sqlite>(
            "SELECT timestamp, role FROM messages WHERE id = ?1 AND session_id = ?2",
        )
        .bind(id)
        .bind(session)
        .fetch_optional(&mut self.conn)
        .await?;
        row.map(|row| Ok((row.try_get(0)?, row.try_get(1)?))).transpose()
    }

    /// The compression lineage root of session `id`: the session itself, unless it continues a
    /// parent Hermes ended for compression, and so on up.
    pub async fn lineage_root(&mut self, id: &str) -> Result<String, sqlx::Error> {
        if !(self.columns.sessions.contains("parent_session_id")
            && self.columns.sessions.contains("end_reason"))
        {
            return Ok(id.to_owned());
        }
        let mut root = id.to_owned();
        // A parent cycle (a corrupt store) is cut short, not followed for ever.
        let mut seen = HashSet::from([root.clone()]);
        loop {
            let parent = query_scalar::<Sqlite, String>(
                "SELECT p.id FROM sessions s JOIN sessions p ON p.id = s.parent_session_id WHERE \
                 s.id = ?1 AND p.end_reason = 'compression'",
            )
            .bind(&root)
            .fetch_optional(&mut self.conn)
            .await?;
            match parent {
                Some(parent) if seen.insert(parent.clone()) => root = parent,
                _ => return Ok(root),
            }
        }
    }

    /// Whether a turn holds a lease on conversation `root` that runs past `now` (unix seconds).
    pub async fn leased(&mut self, root: &str, now: f64) -> Result<bool, sqlx::Error> {
        if !self.columns.tables.contains("session_turn_leases") {
            return Ok(false);
        }
        let found = query_scalar::<Sqlite, i64>(
            "SELECT 1 FROM session_turn_leases WHERE conversation_id = ?1 AND expires_at > ?2",
        )
        .bind(root)
        .bind(now)
        .fetch_optional(&mut self.conn)
        .await?;
        Ok(found.is_some())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn row(id: &str, delegated_by: Option<&str>) -> SessionRow {
        SessionRow {
            id: id.to_owned(),
            title: None,
            title_source: None,
            cwd: None,
            model: None,
            model_config: None,
            parent: None,
            parent_end_reason: None,
            delegated_by: delegated_by.map(str::to_owned),
            started_at: 0.0,
            ended_at: None,
            last_activity_at: None,
            tip: None,
            tip_at: None,
        }
    }

    /// A subagent is a thread when its chain of delegators reaches a coding session, whatever
    /// order the rows come in; one whose chain leaves the rows, or loops, is not.
    #[rstest]
    fn a_subagent_is_a_thread_when_its_chain_reaches_a_coding_session() {
        let rows = vec![
            (false, row("deep", Some("mid"))),
            (false, row("mid", Some("top"))),
            (true, row("top", None)),
            (false, row("of-a-chat", Some("chat"))),
            (false, row("loop-a", Some("loop-b"))),
            (false, row("loop-b", Some("loop-a"))),
            (false, row("untagged", None)),
        ];
        let found: Vec<String> = threads(rows).into_iter().map(|row| row.id).collect();
        assert_eq!(found, ["deep", "mid", "top"]);
    }
}
