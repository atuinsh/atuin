//! The sidecar's write path, on one synchronous connection.
//!
//! An append runs a dozen or so statements, and a replay appends hundreds of thousands of rows.
//! Through sqlx each statement is a round trip to its connection's worker thread, which cost
//! more than the statements themselves; here the whole write runs on a blocking thread, against
//! a rusqlite connection opened on the same database as the sqlx pool that serves the reads.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use atuin_domain::record::{HostId, RecordTag};
use parking_lot::Mutex;
use rusqlite::{Connection, Row, TransactionBehavior};
use tracing::warn;

use super::query::{FromRow, query, query_as, query_scalar};
use super::watermark::BUMP_GENERATION;
use super::{
    AiSessionDatabase, Appended, CallRow, Claimant, DbError, Generation, SessionKey, Tokens,
};
use crate::ai_session::{AtuinSessionId, Message};

/// Long enough to wait out the other connection's write, as sqlx's default is.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Prepared statements kept per connection: more than the write path has.
const STATEMENT_CACHE: usize = 64;

/// The sidecar's writing connection, shared by clones. Writes take turns on it.
#[derive(Debug, Clone)]
pub(super) struct Writer {
    conn: Arc<Mutex<Connection>>,
    interned: Arc<Mutex<Interned>>,
}

/// The `interned` ids this writer has looked up or stored, so the few values a session repeats
/// on every row cost no query after the first. Only what a committed transaction stored is
/// kept: an id from one rolled back would point at nothing.
#[derive(Debug, Default)]
pub(super) struct Interned {
    committed: HashMap<String, i64>,
    pending: HashMap<String, i64>,
}

impl Interned {
    /// The `interned` id of `value`, storing it the first time it is seen.
    fn id(&mut self, conn: &Connection, value: Option<&str>) -> Result<Option<i64>, DbError> {
        let Some(value) = value else {
            return Ok(None);
        };
        if let Some(id) = self.committed.get(value).or_else(|| self.pending.get(value)) {
            return Ok(Some(*id));
        }
        let id = match query_scalar("SELECT id FROM interned WHERE value = ?")
            .bind(value)
            .fetch_optional(conn)?
        {
            Some(id) => id,
            None => query("INSERT INTO interned (value) VALUES (?)")
                .bind(value)
                .execute(conn)?
                .last_insert_rowid(),
        };
        self.pending.insert(value.to_owned(), id);
        Ok(Some(id))
    }

    /// Run `write` in a transaction it commits, keeping the ids it stored only if it did.
    fn transaction<T>(
        &mut self,
        write: impl FnOnce(&mut Self) -> Result<(T, bool), DbError>,
    ) -> Result<T, DbError> {
        self.pending.clear();
        let (result, committed) = write(self)?;
        if committed {
            let pending = std::mem::take(&mut self.pending);
            self.committed.extend(pending);
        }
        Ok(result)
    }
}

impl Writer {
    /// Open `location`, a file the sqlx pool has created or a shared in-memory database's URI,
    /// set up as the pool's connections are.
    pub(super) fn open(location: &str) -> Result<Self, DbError> {
        let conn = Connection::open(location)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            interned: Arc::default(),
        })
    }

    /// [`Self::run`], with this writer's [`Interned`] ids.
    pub(super) async fn run_interning<T, F>(&self, write: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection, &mut Interned) -> Result<T, DbError> + Send + 'static,
    {
        let interned = Arc::clone(&self.interned);
        self.run(move |conn| write(conn, &mut interned.lock())).await
    }

    /// Run `write` on the connection, on a blocking thread. A panic in it is raised here, as if it
    /// had run inline.
    pub(super) async fn run<T, F>(&self, write: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        let task = tokio::task::spawn_blocking(move || {
            // A write that panicked rolled its transaction back as it unwound: the connection is
            // as good as before it.
            let mut conn = conn.lock();
            write(&mut conn)
        });
        match task.await {
            Ok(result) => result,
            Err(err) => match err.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                Err(_) => Err(DbError::WriterCancelled),
            },
        }
    }
}

impl FromRow for SessionKey {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            started_at: row.get("started_at")?,
            parent_harness: row.get("parent_harness")?,
            parent_session_id: row.get("parent_session_id")?,
            title: row.get("title")?,
        })
    }
}

impl FromRow for Claimant {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            session_id: row.get("session_id")?,
            scope: row.get("scope")?,
            started_at: row.get("started_at")?,
            parent_harness: row.get("parent_harness")?,
            parent_session_id: row.get("parent_session_id")?,
            usage_input: row.get("usage_input")?,
            usage_output: row.get("usage_output")?,
            usage_cache_read: row.get("usage_cache_read")?,
            usage_cache_write: row.get("usage_cache_write")?,
            usage_reasoning: row.get("usage_reasoning")?,
        })
    }
}

impl FromRow for CallRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            scope: row.get("scope")?,
            session_id: row.get("session_id")?,
            usage_input: row.get("usage_input")?,
            usage_output: row.get("usage_output")?,
            usage_cache_read: row.get("usage_cache_read")?,
            usage_cache_write: row.get("usage_cache_write")?,
            usage_reasoning: row.get("usage_reasoning")?,
        })
    }
}

/// A message made ready to append: what storing it derives from the message alone (its
/// encodings, compressed content and search text), worked out ahead of the writer, on any thread.
#[derive(Debug, Clone)]
pub struct PreparedMessage {
    msg: Message,
    role: String,
    content: String,
    content_z: Option<Vec<u8>>,
    stop_reason: Option<String>,
    title_change: Option<String>,
    body: String,
    preview: Option<String>,
    last_reply: Option<String>,
}

impl PreparedMessage {
    pub fn new(msg: Message) -> Result<Self, DbError> {
        let (content, content_z) =
            AiSessionDatabase::split_content(serde_json::to_string(&msg.content)?)?;
        Ok(Self {
            role: serde_json::to_string(&msg.role)?,
            content,
            content_z,
            stop_reason: msg.stop_reason.as_ref().map(serde_json::to_string).transpose()?,
            title_change: msg.title_change.as_ref().map(serde_json::to_string).transpose()?,
            body: AiSessionDatabase::searchable_body(&msg),
            preview: AiSessionDatabase::preview_text(&msg),
            last_reply: AiSessionDatabase::reply_text(&msg),
            msg,
        })
    }
}

impl AiSessionDatabase {
    /// Append each of `msgs` in order, in one transaction, unless an invalidation since
    /// `generation`: see [`Self::append_all`].
    pub(super) fn append_page(
        conn: &mut Connection,
        interned: &mut Interned,
        msgs: &[PreparedMessage],
        generation: Generation,
    ) -> Result<bool, DbError> {
        interned.transaction(|interned| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: i64 = query_scalar("SELECT generation FROM projection_state WHERE id = 0")
                .fetch_one(&tx)?;
            if current != generation.0 {
                return Ok((false, false));
            }
            for msg in msgs {
                Self::append_in(&tx, interned, msg)?;
            }
            tx.commit()?;
            Ok((true, true))
        })
    }

    /// Append `msg` in its own transaction: see [`Self::append`].
    pub(super) fn append_one(
        conn: &mut Connection,
        interned: &mut Interned,
        msg: &PreparedMessage,
    ) -> Result<Appended, DbError> {
        interned.transaction(|interned| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let appended = Self::append_in(&tx, interned, msg)?;
            tx.commit()?;
            Ok((appended, true))
        })
    }

    fn append_in(
        conn: &Connection,
        interned: &mut Interned,
        prepared: &PreparedMessage,
    ) -> Result<Appended, DbError> {
        let msg = &prepared.msg;
        let harness = msg.session.harness as i64;
        let session_id = msg.session.session.as_ref();
        let source_id = msg.source_id.as_ref();
        let id = msg.id.0.as_bytes().as_slice();

        let (parent_harness, parent_session_id) = match &msg.parent {
            Some(parent) => (Some(parent.harness as i64), Some(parent.session.as_ref().to_owned())),
            None => (None, None),
        };

        let timestamp = Self::millis(msg.timestamp);
        let [usage_input, usage_output, usage_cache_read, usage_cache_write, _] =
            Self::fold_usage(msg.usage.as_ref());
        // Unlike the others, a row's reasoning stays NULL when unreported: most harnesses never
        // break it out, and zero would claim the call did not reason.
        let usage_reasoning =
            msg.usage.and_then(|u| u.reasoning).map(|n| i64::try_from(n).unwrap_or(i64::MAX));
        let cwd = msg.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());
        let host = msg.host.map(Self::host_repr);

        // Capture gives every row it stores its session's id. A row appended without one (only
        // ever directly, as tests do) gets a fresh one, which MIN settles like any other.
        let atuin_id = msg.atuin_id.unwrap_or_else(|| AtuinSessionId::mint(msg.timestamp));
        let atuin_id = atuin_id.as_bytes().as_slice();
        let before = Self::session_key(conn, harness, session_id)?;

        // Messages reference their session by id, so it has to exist first. The upsert below
        // folds this row in exactly as it would a fresh insert: same timestamps, nothing counted.
        // A new session starts as its own root; regroup() below places it.
        let session = match &before {
            Some(before) => before.id,
            None => query(
                "INSERT INTO sessions (harness, session_id, started_at, updated_at, root_harness,
                    root_session_id, atuin_id)
                VALUES (?1, ?2, ?3, ?3, ?1, ?2, ?4)",
            )
            .bind(harness)
            .bind(session_id)
            .bind(timestamp)
            .bind(atuin_id)
            .execute(conn)?
            .last_insert_rowid(),
        };

        let inserted = query(
            "INSERT INTO messages (
                id, harness, session, source_id, parent_harness, parent_session_id,
                parent_source_id, timestamp, role, content, content_z, cwd, git_branch, model,
                usage_input, usage_output, usage_cache_read, usage_cache_write,
                usage_reasoning, stop_reason, usage_present, turn_id, title_change, host
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(session, source_id) DO NOTHING",
        )
        .bind(id)
        .bind(harness)
        .bind(session)
        .bind(source_id)
        .bind(parent_harness)
        .bind(parent_session_id.clone())
        .bind(msg.parent_source_id.as_ref().map(|s| s.as_ref()))
        .bind(timestamp)
        .bind(interned.id(conn, Some(&prepared.role))?)
        .bind(&prepared.content)
        .bind(prepared.content_z.as_deref())
        .bind(interned.id(conn, cwd.as_deref())?)
        .bind(interned.id(conn, msg.git_branch.as_deref())?)
        .bind(interned.id(conn, msg.model.as_deref())?)
        .bind(usage_input)
        .bind(usage_output)
        .bind(usage_cache_read)
        .bind(usage_cache_write)
        .bind(usage_reasoning)
        .bind(prepared.stop_reason.as_deref())
        .bind(i64::from(msg.usage.is_some()))
        .bind(msg.turn_id.as_deref())
        .bind(prepared.title_change.as_deref())
        .bind(interned.id(conn, host.as_deref())?)
        .execute(conn)?;

        if inserted.rows_affected() == 0 {
            // A row stored without a host learns it when the reproject replays its record.
            if let Some(host) = &host {
                let host = interned.id(conn, Some(host))?;
                Self::backfill_host(conn, session, source_id, host)?;
            }
            // Another host may have captured this row under another id.
            query("UPDATE sessions SET atuin_id = MIN(atuin_id, ?) WHERE id = ?")
                .bind(atuin_id)
                .bind(session)
                .execute(conn)?;
            return Ok(Appended::Duplicate);
        }

        let rowid = inserted.last_insert_rowid();
        query("INSERT INTO messages_fts(rowid, title, body) VALUES (?, ?, ?)")
            .bind(rowid)
            .bind(msg.session_title.as_deref().unwrap_or(""))
            .bind(&prepared.body)
            .execute(conn)?;

        // Session title denormalised onto the message so it survives a reproject from records
        // (which carry messages only). The session upsert below takes the newest row's, so a
        // cleared title clears; the capture pipeline stamps every row with the ranked title.
        let title = msg.session_title.as_deref();
        let (preview, last_reply) = (&prepared.preview, &prepared.last_reply);

        // Usage is not folded in here: it is attributed per model call below. A structural row
        // (usage, title, session context, a tree node with nothing to show) is no message.
        let counted = i64::from(!msg.content.is_empty());
        query(
            "INSERT INTO sessions (
                harness, session_id, parent_harness, parent_session_id, cwd, git_branch, model,
                started_at, updated_at, message_count, title, title_source, preview, last_reply,
                last_reply_at, parent_kind, host_id, atuin_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(harness, session_id) DO UPDATE SET
                host_id = COALESCE(sessions.host_id, excluded.host_id),
                atuin_id = MIN(sessions.atuin_id, excluded.atuin_id),
                parent_harness = COALESCE(excluded.parent_harness, sessions.parent_harness),
                parent_session_id = COALESCE(excluded.parent_session_id, \
             sessions.parent_session_id),
                parent_kind = COALESCE(excluded.parent_kind, sessions.parent_kind),
                cwd = COALESCE(excluded.cwd, sessions.cwd),
                git_branch = COALESCE(excluded.git_branch, sessions.git_branch),
                model = COALESCE(excluded.model, sessions.model),
                started_at = MIN(sessions.started_at, excluded.started_at),
                updated_at = MAX(sessions.updated_at, excluded.updated_at),
                message_count = sessions.message_count + excluded.message_count,
                title = CASE WHEN excluded.updated_at >= sessions.updated_at THEN excluded.title \
             ELSE sessions.title END,
                title_source = CASE WHEN excluded.updated_at >= sessions.updated_at THEN \
             excluded.title_source ELSE sessions.title_source END,
                preview = COALESCE(sessions.preview, excluded.preview),
                last_reply = CASE WHEN excluded.last_reply_at >= COALESCE(sessions.last_reply_at, \
             excluded.last_reply_at) THEN excluded.last_reply ELSE sessions.last_reply END,
                last_reply_at = MAX(COALESCE(sessions.last_reply_at, excluded.last_reply_at), \
             COALESCE(excluded.last_reply_at, sessions.last_reply_at))",
        )
        .bind(harness)
        .bind(session_id)
        .bind(parent_harness)
        .bind(parent_session_id)
        .bind(cwd)
        .bind(msg.git_branch.as_deref())
        .bind(msg.model.as_deref())
        .bind(timestamp)
        .bind(timestamp)
        .bind(counted)
        .bind(title)
        .bind(msg.session_title_source.map(Self::title_source_repr))
        .bind(preview)
        .bind(last_reply.as_deref())
        .bind(last_reply.as_ref().map(|_| timestamp))
        .bind(msg.parent.as_ref().and(msg.parent_kind).map(Self::parent_kind_repr))
        .bind(host.as_deref())
        .bind(atuin_id)
        .execute(conn)?;

        // A session is on the host of its earliest row, whatever order its rows arrive in (a
        // replay takes one host's series at a time). A row at or before the earliest so far may
        // be the new earliest, so the session's host is worked out again.
        if before.as_ref().is_some_and(|b| timestamp <= b.started_at) {
            Self::refresh_session_host(conn, session)?;
        }

        // The session as this row left it. Nothing below moves its start, parent or title.
        let after = match &before {
            Some(_) => Self::session_key(conn, harness, session_id)?,
            None => None,
        };

        // A session is placed in its group when it first appears and when it learns its parent
        // (the parent link only ever goes from absent to present).
        let gained_parent =
            before.as_ref().is_some_and(|b| b.parent_session_id.is_none() && msg.parent.is_some());
        if before.is_none() || gained_parent {
            Self::regroup(conn, harness, session_id)?;
        }
        // A copy names no parent, so it is linked to its original through the calls they share
        // (see `sessions.copy_of_session_id` in the schema). A link that moves other than by being
        // made regroups everything.
        let mut relinked = false;
        if let Some(before) = &before {
            if gained_parent {
                relinked |= Self::unlink(conn, harness, session_id)?;
            }
            if after.as_ref().map(|a| a.started_at) != Some(before.started_at) {
                relinked |= Self::relink_sharers(conn, harness, session_id)?;
            }
        }
        if let Some(turn) = &msg.turn_id {
            relinked |= Self::link_copies(conn, harness, session_id, turn)?;
        }
        if relinked {
            Self::regroup_all(conn)?;
        }
        // A content-derived call is scoped to its holders' group, and owned by who in it descends
        // from no other holder: a session whose root or ancestry moved has its calls attributed
        // afresh.
        if gained_parent {
            Self::mark_descendants(conn, &format!("[{session}]"))?;
        }
        // Only the steps above regroup (`regrouped` is empty between transactions), so a row
        // that took none of them has nothing to take: most rows, and a query saved on each.
        let regrouped = before.is_none()
            || gained_parent
            || msg.turn_id.is_some()
            || after.as_ref().map(|a| a.started_at) != before.as_ref().map(|b| b.started_at);
        let mut recount: BTreeSet<(i64, String)> = if regrouped {
            Self::take_regrouped_calls(conn)?.into_iter().collect()
        } else {
            BTreeSet::new()
        };

        if let Some(usage) = &msg.usage {
            match &msg.turn_id {
                Some(turn) => {
                    recount.insert((harness, turn.clone()));
                }
                // A row outside any model call counts on its own.
                None => {
                    let tokens = Self::fold_usage(Some(usage));
                    Self::add_usage(conn, harness, session_id, tokens, 1)?;
                }
            }
        }
        // Which session owns a call shared with others depends on each claimant's start and
        // ancestry: when either moves, every call this session claims is attributed afresh.
        let existed = before.is_some();
        let previous_title = match before {
            Some(before) => {
                if after.as_ref().map(SessionKey::rank) != Some(before.rank()) {
                    let turns: Vec<String> = query_scalar(
                        "SELECT DISTINCT turn_id FROM messages WHERE session = ? AND \
                         usage_present = 1 AND turn_id IS NOT NULL",
                    )
                    .bind(session)
                    .fetch_all(conn)?;
                    recount.extend(turns.into_iter().map(|turn| (harness, turn)));
                }
                before.title
            }
            None => None,
        };
        for (harness, turn) in &recount {
            Self::attribute_call(conn, *harness, turn)?;
        }

        // A changed (or cleared) title has to reach the rows indexed before it. messages_fts is
        // contentless, so a single-column UPDATE is not supported: rewrite each of the session's
        // index rows, re-deriving the body from the stored content. Gated on the session's title
        // actually changing, since every replayed line of a titled session carries it.
        let current_title = after.and_then(|a| a.title);
        if existed && current_title != previous_title {
            let title = current_title.as_deref().unwrap_or("");
            type BodyRow =
                (i64, String, Option<Vec<u8>>, Option<String>, Option<String>, Option<String>);
            let rows: Vec<BodyRow> = query_as(
                "SELECT rowid, content, content_z, (SELECT value FROM interned WHERE id = cwd), \
                 (SELECT value FROM interned WHERE id = git_branch), (SELECT value FROM interned \
                 WHERE id = model) FROM messages WHERE session = ?",
            )
            .bind(session)
            .fetch_all(conn)?;

            for (rowid, content, content_z, cwd, git_branch, model) in rows {
                let body = Self::body_from_parts(
                    content,
                    content_z,
                    cwd.as_deref(),
                    git_branch.as_deref(),
                    model.as_deref(),
                )
                .unwrap_or_else(|err| {
                    warn!(?err, rowid, "failed to decode ai-session message; indexing empty");
                    String::new()
                });
                query("INSERT OR REPLACE INTO messages_fts(rowid, title, body) VALUES (?, ?, ?)")
                    .bind(rowid)
                    .bind(title)
                    .bind(&body)
                    .execute(conn)?;
            }
        }

        Ok(Appended::New)
    }

    /// What decides a session's claim on a shared model call (see [`Self::attribute_call`]),
    /// plus its title, read before and after an append.
    fn session_key(
        conn: &Connection,
        harness: i64,
        session_id: &str,
    ) -> Result<Option<SessionKey>, DbError> {
        Ok(query_as(
            "SELECT id, started_at, parent_harness, parent_session_id, title FROM sessions WHERE \
             harness = ? AND session_id = ?",
        )
        .bind(harness)
        .bind(session_id)
        .fetch_optional(conn)?)
    }

    /// The content-derived calls (not [`linkable_turn`]) held by the sessions whose root changed
    /// since this was last asked, which the `sessions_regrouped` trigger records wherever
    /// sessions are regrouped, or whose ancestry [`Self::mark_descendants`] recorded as moved.
    /// Such a call is counted once per group of its holders (see [`Self::attribute_call`]), so
    /// each has to be attributed afresh. Forgets the sessions recorded.
    fn take_regrouped_calls(conn: &Connection) -> Result<Vec<(i64, String)>, DbError> {
        // CROSS JOIN keeps this order: the (almost always empty) recorded sessions, then their
        // rows. Left to choose, the planner walks every message by its turn index to serve the
        // DISTINCT, and as this runs on every append, a rebuild went quadratic in the messages.
        let turns = query_as(concat!(
            "SELECT DISTINCT m.harness, m.turn_id FROM regrouped r CROSS JOIN messages m ON \
             m.session = r.session WHERE m.usage_present = 1 AND m.turn_id IS NOT NULL AND NOT ",
            linkable_turn!()
        ))
        .fetch_all(conn)?;
        query("DELETE FROM regrouped").execute(conn)?;
        Ok(turns)
    }

    /// Record `sessions` (a JSON array of their row ids) and every session descending from them
    /// by parent links for [`Self::take_regrouped_calls`]: their ancestry, which decides who in a
    /// group owns a call shared there, moves when one of `sessions` learns its parent or goes,
    /// even if no root does (a cycle closing under its least member, a copy learning a parent
    /// in its own group).
    fn mark_descendants(conn: &Connection, sessions: &str) -> Result<(), DbError> {
        query(
            "WITH RECURSIVE down (id, harness, session_id) AS (SELECT id, harness, session_id \
             FROM sessions WHERE id IN (SELECT value FROM json_each(?)) UNION SELECT s.id, \
             s.harness, s.session_id FROM down d JOIN sessions s ON s.parent_harness = d.harness \
             AND s.parent_session_id = d.session_id) INSERT OR IGNORE INTO regrouped (session) \
             SELECT id FROM down",
        )
        .bind(sessions)
        .execute(conn)?;
        Ok(())
    }

    /// Group every session afresh under its top-most stored ancestor, following the parent else
    /// the copy link: however deep the chain, and with a cycle headed by its least member (by
    /// harness, then id), whichever member a walk up entered it by.
    pub(super) fn regroup_all(conn: &Connection) -> Result<(), DbError> {
        query(
            "WITH RECURSIVE up (harness, session_id, up_harness, up_session_id) AS (SELECT \
             a.harness, a.session_id, p.harness, p.session_id FROM sessions a JOIN sessions p ON \
             p.harness = CASE WHEN a.parent_session_id IS NULL THEN a.harness ELSE \
             a.parent_harness END AND p.session_id = COALESCE(a.parent_session_id, \
             a.copy_of_session_id)), chain (harness, session_id, anc_harness, anc_session_id, \
             depth, path) AS (SELECT harness, session_id, harness, session_id, 0, \
             json_array(harness || ':' || session_id) FROM sessions UNION ALL SELECT c.harness, \
             c.session_id, u.up_harness, u.up_session_id, c.depth + 1, json_insert(c.path, \
             '$[#]', u.up_harness || ':' || u.up_session_id) FROM chain c JOIN up u ON u.harness \
             = c.anc_harness AND u.session_id = c.anc_session_id WHERE NOT EXISTS (SELECT 1 FROM \
             json_each(c.path) v WHERE v.value = u.up_harness || ':' || u.up_session_id)), ends \
             AS (SELECT harness, session_id, anc_harness, anc_session_id, max(depth) AS depth \
             FROM chain GROUP BY harness, session_id), loops AS (SELECT e.harness, e.session_id, \
             c.depth AS entered FROM ends e JOIN up u ON u.harness = e.anc_harness AND \
             u.session_id = e.anc_session_id JOIN chain c ON c.harness = e.harness AND \
             c.session_id = e.session_id AND c.anc_harness = u.up_harness AND c.anc_session_id = \
             u.up_session_id), heads AS (SELECT l.harness, l.session_id, c.anc_harness, \
             c.anc_session_id, row_number() OVER (PARTITION BY l.harness, l.session_id ORDER BY \
             c.anc_harness, c.anc_session_id) AS n FROM loops l JOIN chain c ON c.harness = \
             l.harness AND c.session_id = l.session_id AND c.depth >= l.entered), roots AS \
             (SELECT e.harness, e.session_id, COALESCE(h.anc_harness, e.anc_harness) AS \
             anc_harness, COALESCE(h.anc_session_id, e.anc_session_id) AS anc_session_id FROM \
             ends e LEFT JOIN heads h ON h.harness = e.harness AND h.session_id = e.session_id \
             AND h.n = 1) UPDATE sessions SET root_harness = roots.anc_harness, root_session_id = \
             roots.anc_session_id FROM roots WHERE roots.harness = sessions.harness AND \
             roots.session_id = sessions.session_id",
        )
        .execute(conn)?;
        Ok(())
    }

    /// For a row of `session` holding call `turn`, just stored: offer each other session holding
    /// the call as the other's original. Each keeps the lowest-ranked original it is offered (see
    /// `sessions.copy_of_session_id` in the schema), so this only ever lowers a link, and while no
    /// session's start or parent moves, the links end up the same whatever order the rows arrive
    /// in.
    ///
    /// A session linked for the first time was a root, and is placed like one learning its
    /// parent. Returns whether an existing link moved instead, which needs
    /// [`Self::regroup_all`].
    fn link_copies(
        conn: &Connection,
        harness: i64,
        session_id: &str,
        turn: &str,
    ) -> Result<bool, DbError> {
        let others: Vec<String> = query_scalar(concat!(
            "SELECT DISTINCT s.session_id FROM messages m JOIN sessions s ON s.id = m.session \
             WHERE m.harness = ? AND m.turn_id = ? AND s.session_id <> ? AND ",
            linkable_turn!()
        ))
        .bind(harness)
        .bind(turn)
        .bind(session_id)
        .fetch_all(conn)?;

        let mut moved = false;
        for other in &others {
            for (copy, original) in [(session_id, other.as_str()), (other.as_str(), session_id)] {
                let previous: Option<String> = query_scalar(
                    "SELECT copy_of_session_id FROM sessions WHERE harness = ? AND session_id = ?",
                )
                .bind(harness)
                .bind(copy)
                .fetch_one(conn)?;
                // Only a parentless session links, only to a parentless one ranked below it, and
                // only when that ranks below the original it has.
                let linked = query(
                    "UPDATE sessions SET copy_of_session_id = ?3 WHERE harness = ?1 AND \
                     session_id = ?2 AND parent_session_id IS NULL AND EXISTS (SELECT 1 FROM \
                     sessions o WHERE o.harness = ?1 AND o.session_id = ?3 AND \
                     o.parent_session_id IS NULL AND (o.started_at, o.session_id) < \
                     (sessions.started_at, sessions.session_id) AND NOT EXISTS (SELECT 1 FROM \
                     sessions c WHERE c.harness = ?1 AND c.session_id = \
                     sessions.copy_of_session_id AND (c.started_at, c.session_id) <= \
                     (o.started_at, o.session_id)))",
                )
                .bind(harness)
                .bind(copy)
                .bind(original)
                .execute(conn)?;
                if linked.rows_affected() == 0 {
                    continue;
                }
                match previous {
                    None => Self::regroup(conn, harness, copy)?,
                    Some(_) => moved = true,
                }
            }
        }
        Ok(moved)
    }

    /// Link `session` to its original afresh from every call it shares. Returns whether its link
    /// changed.
    fn relink(conn: &Connection, harness: i64, session_id: &str) -> Result<bool, DbError> {
        let (previous, parent): (Option<String>, Option<String>) = query_as(
            "SELECT copy_of_session_id, parent_session_id FROM sessions WHERE harness = ? AND \
             session_id = ?",
        )
        .bind(harness)
        .bind(session_id)
        .fetch_one(conn)?;
        let original: Option<String> = match parent {
            Some(_) => None,
            // CROSS JOIN keeps this order: the session's own rows, then who else holds each call.
            None => query_scalar(concat!(
                "SELECT t.session_id FROM sessions s CROSS JOIN messages m ON m.session = s.id \
                 CROSS JOIN messages o ON o.harness = m.harness AND o.turn_id = m.turn_id AND \
                 o.session <> m.session CROSS JOIN sessions t ON t.id = o.session WHERE s.harness \
                 = ? AND s.session_id = ? AND ",
                linkable_turn!(),
                " AND t.parent_session_id IS NULL AND (t.started_at, t.session_id) < \
                 (s.started_at, s.session_id) ORDER BY t.started_at, t.session_id LIMIT 1"
            ))
            .bind(harness)
            .bind(session_id)
            .fetch_optional(conn)?,
        };
        if original == previous {
            return Ok(false);
        }
        query("UPDATE sessions SET copy_of_session_id = ? WHERE harness = ? AND session_id = ?")
            .bind(original)
            .bind(harness)
            .bind(session_id)
            .execute(conn)?;
        Ok(true)
    }

    /// `session`'s start moved: it may now rank below sessions sharing its calls, or they below
    /// it, so it and each of them is linked afresh. Returns whether any link changed.
    fn relink_sharers(conn: &Connection, harness: i64, session_id: &str) -> Result<bool, DbError> {
        let sharers: Vec<String> = query_scalar(concat!(
            "SELECT DISTINCT t.session_id FROM messages m CROSS JOIN messages o ON o.harness = \
             m.harness AND o.turn_id = m.turn_id AND o.session <> m.session CROSS JOIN sessions t \
             ON t.id = o.session WHERE m.session = (SELECT id FROM sessions WHERE harness = ? AND \
             session_id = ?) AND ",
            linkable_turn!()
        ))
        .bind(harness)
        .bind(session_id)
        .fetch_all(conn)?;
        if sharers.is_empty() {
            return Ok(false);
        }

        let mut changed = Self::relink(conn, harness, session_id)?;
        for sharer in &sharers {
            changed |= Self::relink(conn, harness, sharer)?;
        }
        Ok(changed)
    }

    /// `session` learned its parent: it groups by that now, and can no longer be anyone's
    /// original, so its own link goes and the sessions linked to it are linked afresh. Returns
    /// whether any link changed.
    fn unlink(conn: &Connection, harness: i64, session_id: &str) -> Result<bool, DbError> {
        let mut changed = Self::relink(conn, harness, session_id)?;
        let copies: Vec<String> = query_scalar(
            "SELECT session_id FROM sessions WHERE harness = ? AND copy_of_session_id = ?",
        )
        .bind(harness)
        .bind(session_id)
        .fetch_all(conn)?;
        for copy in &copies {
            changed |= Self::relink(conn, harness, copy)?;
        }
        Ok(changed)
    }

    /// Record `host` (its `interned` id) on a stored row that has none, and work its session's
    /// host out again.
    fn backfill_host(
        conn: &Connection,
        session: i64,
        source_id: &str,
        host: Option<i64>,
    ) -> Result<(), DbError> {
        let filled = query(
            "UPDATE messages SET host = ? WHERE session = ? AND source_id = ? AND host IS NULL",
        )
        .bind(host)
        .bind(session)
        .bind(source_id)
        .execute(conn)?
        .rows_affected()
            > 0;
        if filled {
            Self::refresh_session_host(conn, session)?;
        }
        Ok(())
    }

    /// Set `session`'s host to its earliest row's: the one with the lowest timestamp, then the
    /// lowest record id, among the rows whose host is known. A session with no such row keeps
    /// whatever it has.
    fn refresh_session_host(conn: &Connection, session: i64) -> Result<(), DbError> {
        query(
            "UPDATE sessions SET host_id = COALESCE((SELECT (SELECT value FROM interned WHERE id \
             = host) FROM messages WHERE session = ?1 AND host IS NOT NULL ORDER BY timestamp, id \
             LIMIT 1), host_id) WHERE id = ?1",
        )
        .bind(session)
        .execute(conn)?;
        Ok(())
    }

    /// Place a session that just appeared, just learned its parent or was just linked to its
    /// original (see [`Self::link_copies`]) in its group.
    ///
    /// Every session's root is its top-most stored ancestor, following parents else copy links.
    /// A session is only placed when it is a root (it is new, or had neither), so it and
    /// everything grouped under it move to its parent's root. Being new, it may also be the missing parent of sessions stored before it,
    /// which were roots of their own: those groups move under it too. The result depends only on
    /// the sessions stored, never on the order they arrived in.
    fn regroup(conn: &Connection, harness: i64, session_id: &str) -> Result<(), DbError> {
        let (parent_harness, parent_session_id, copy_of): (
            Option<i64>,
            Option<String>,
            Option<String>,
        ) = query_as(
            "SELECT parent_harness, parent_session_id, copy_of_session_id FROM sessions WHERE \
             harness = ? AND session_id = ?",
        )
        .bind(harness)
        .bind(session_id)
        .fetch_one(conn)?;
        // A copy, which names no parent, groups under its original.
        let (parent_harness, parent_session_id) = match (parent_session_id, copy_of) {
            (None, Some(original)) => (Some(harness), Some(original)),
            (parent, _) => (parent_harness, parent),
        };

        let parent_root: Option<(i64, String)> = match (parent_harness, parent_session_id) {
            (Some(parent_harness), Some(parent_session_id)) => query_as(
                "SELECT root_harness, root_session_id FROM sessions WHERE harness = ? AND \
                 session_id = ? AND root_harness IS NOT NULL",
            )
            .bind(parent_harness)
            .bind(parent_session_id)
            .fetch_optional(conn)?,
            _ => None,
        };
        // The link closes a cycle when the parent is grouped under this very session, or the
        // root it is grouped under was waiting for this session as its own parent. A cycle has no
        // top: it is headed by its least member whichever order its sessions arrived in, which
        // `regroup_all` works out.
        if let Some((root_harness, root_session_id)) = &parent_root {
            let closes_cycle = (*root_harness, root_session_id.as_str()) == (harness, session_id)
                || query_scalar::<i64>(
                    "SELECT 1 FROM sessions WHERE harness = ?1 AND session_id = ?2 AND \
                     ((parent_harness = ?3 AND parent_session_id = ?4) OR (parent_session_id IS \
                     NULL AND harness = ?3 AND copy_of_session_id = ?4))",
                )
                .bind(*root_harness)
                .bind(root_session_id)
                .bind(harness)
                .bind(session_id)
                .fetch_optional(conn)?
                .is_some();
            if closes_cycle {
                return Self::regroup_all(conn);
            }
        }
        // A parent that is not stored leaves the session a root.
        let (root_harness, root_session_id) =
            parent_root.unwrap_or_else(|| (harness, session_id.to_owned()));

        if (root_harness, root_session_id.as_str()) != (harness, session_id) {
            query(
                "UPDATE sessions SET root_harness = ?, root_session_id = ? WHERE root_harness = ? \
                 AND root_session_id = ?",
            )
            .bind(root_harness)
            .bind(&root_session_id)
            .bind(harness)
            .bind(session_id)
            .execute(conn)?;
        }

        // Sessions stored before their parent (this one) arrived, still roots of their own.
        query(
            "UPDATE sessions SET root_harness = ?1, root_session_id = ?2 WHERE (root_harness, \
             root_session_id) IN (SELECT harness, session_id FROM sessions WHERE parent_harness = \
             ?3 AND parent_session_id = ?4 AND root_harness = harness AND root_session_id = \
             session_id AND NOT (harness = ?1 AND session_id = ?2))",
        )
        .bind(root_harness)
        .bind(&root_session_id)
        .bind(harness)
        .bind(session_id)
        .execute(conn)?;
        Ok(())
    }

    /// Add `sign * tokens` to a session's usage totals.
    fn add_usage(
        conn: &Connection,
        harness: i64,
        session_id: &str,
        tokens: Tokens,
        sign: i64,
    ) -> Result<(), DbError> {
        let [input, output, cache_read, cache_write, reasoning] =
            tokens.map(|n| n.saturating_mul(sign));
        query(
            "UPDATE sessions SET usage_input = usage_input + ?, usage_output = usage_output + ?, \
             usage_cache_read = usage_cache_read + ?, usage_cache_write = usage_cache_write + ?, \
             usage_reasoning = usage_reasoning + ? WHERE harness = ? AND session_id = ?",
        )
        .bind(input)
        .bind(output)
        .bind(cache_read)
        .bind(cache_write)
        .bind(reasoning)
        .bind(harness)
        .bind(session_id)
        .execute(conn)?;
        Ok(())
    }

    /// Count model call `turn` once, whichever sessions its rows were copied into (forks,
    /// subagent replays): its usage is the field-wise max over every row reporting it (streamed
    /// rows grow), and it belongs to one session, the owner.
    ///
    /// The owner is the earliest-started claimant (ties broken by session id), ignoring any
    /// claimant descended from another -- a fork's copies can carry the parent's own timestamps,
    /// so start times alone may tie. That depends only on the rows and sessions stored, never on
    /// the order they arrived in, so capture, import and a rebuild from records on any host all
    /// agree; [`Self::append`] calls this again whenever a claimant's start or parent moves.
    ///
    /// Only a turn id the harness gave the call (see [`linkable_turn`]) is the same call wherever
    /// it is held. One capture derived from the line's content can be shared by unrelated
    /// sessions, so it is counted once per group: the claimants sharing a root (see
    /// [`Self::regroup`], which follows parents, else copy links, and heads a cycle by its least
    /// member) share one count, owned among them as above, and the `calls` row is scoped to that
    /// root. Whenever a claimant's root or ancestry moves, the call is attributed again (see
    /// [`Self::take_regrouped_calls`]).
    fn attribute_call(conn: &Connection, harness: i64, turn: &str) -> Result<(), DbError> {
        let claimants: Vec<Claimant> = query_as(
            "SELECT s.session_id AS session_id, COALESCE(s.root_harness || ':' || \
             s.root_session_id, s.harness || ':' || s.session_id) AS scope, s.started_at AS \
             started_at, s.parent_harness AS parent_harness, s.parent_session_id AS \
             parent_session_id, MAX(m.usage_input) AS usage_input, MAX(m.usage_output) AS \
             usage_output, MAX(m.usage_cache_read) AS usage_cache_read, MAX(m.usage_cache_write) \
             AS usage_cache_write, COALESCE(MAX(m.usage_reasoning), 0) AS usage_reasoning FROM \
             messages m JOIN sessions s ON s.id = m.session WHERE m.harness = ? AND m.turn_id = ? \
             AND m.usage_present = 1 GROUP BY m.session",
        )
        .bind(harness)
        .bind(turn)
        .fetch_all(conn)?;

        // Each scope the call is counted in, with the claimants counting toward it.
        let mut scopes: BTreeMap<&str, Vec<&Claimant>> = BTreeMap::new();
        let linkable = Self::is_linkable_turn(conn, harness, turn)?;
        for claimant in &claimants {
            let scope = if linkable {
                ""
            } else {
                claimant.scope.as_str()
            };
            scopes.entry(scope).or_default().push(claimant);
        }

        let mut calls = Vec::with_capacity(scopes.len());
        for (scope, members) in scopes {
            let tokens = members.iter().fold([0; 5], |acc: Tokens, c| {
                let row = c.tokens();
                std::array::from_fn(|i| acc[i].max(row[i]))
            });
            let ids: HashSet<&str> = members.iter().map(|c| c.session_id.as_str()).collect();
            let mut eligible = Vec::with_capacity(members.len());
            for claimant in &members {
                if members.len() == 1 || !Self::descends_from(conn, harness, claimant, &ids)? {
                    eligible.push(*claimant);
                }
            }
            // Only a parent cycle leaves nobody; fall back to the plain ranking.
            if eligible.is_empty() {
                eligible.extend(&members);
            }
            let Some(owner) = eligible
                .into_iter()
                .min_by(|a, b| (a.started_at, &a.session_id).cmp(&(b.started_at, &b.session_id)))
            else {
                continue;
            };
            calls.push(CallRow {
                scope: scope.to_owned(),
                session_id: owner.session_id.clone(),
                usage_input: tokens[0],
                usage_output: tokens[1],
                usage_cache_read: tokens[2],
                usage_cache_write: tokens[3],
                usage_reasoning: tokens[4],
            });
        }

        let previous: Vec<CallRow> = query_as(
            "SELECT scope, session_id, usage_input, usage_output, usage_cache_read, \
             usage_cache_write, usage_reasoning FROM calls WHERE harness = ? AND turn_id = ? \
             ORDER BY scope",
        )
        .bind(harness)
        .bind(turn)
        .fetch_all(conn)?;
        if previous == calls {
            return Ok(());
        }
        for call in &previous {
            Self::add_usage(conn, harness, &call.session_id, call.tokens(), -1)?;
        }
        query("DELETE FROM calls WHERE harness = ? AND turn_id = ?")
            .bind(harness)
            .bind(turn)
            .execute(conn)?;
        for call in &calls {
            Self::add_usage(conn, harness, &call.session_id, call.tokens(), 1)?;
            query(
                "INSERT INTO calls (harness, turn_id, scope, session_id, usage_input, \
                 usage_output, usage_cache_read, usage_cache_write, usage_reasoning) VALUES (?, \
                 ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(harness)
            .bind(turn)
            .bind(&call.scope)
            .bind(&call.session_id)
            .bind(call.usage_input)
            .bind(call.usage_output)
            .bind(call.usage_cache_read)
            .bind(call.usage_cache_write)
            .bind(call.usage_reasoning)
            .execute(conn)?;
        }
        Ok(())
    }

    /// Whether `turn` is an id `harness` gave the model call: [`linkable_turn`], asked of it alone.
    fn is_linkable_turn(conn: &Connection, harness: i64, turn: &str) -> Result<bool, DbError> {
        Ok(query_scalar(concat!(
            "SELECT EXISTS (SELECT 1 FROM (SELECT ? AS harness, ? AS turn_id) m WHERE ",
            linkable_turn!(),
            ")"
        ))
        .bind(harness)
        .bind(turn)
        .fetch_one(conn)?)
    }

    /// Whether another of `claimants` is an ancestor of `claimant`, following stored parent
    /// links within the harness.
    fn descends_from(
        conn: &Connection,
        harness: i64,
        claimant: &Claimant,
        claimants: &HashSet<&str>,
    ) -> Result<bool, DbError> {
        let mut seen = HashSet::from([claimant.session_id.clone()]);
        let mut next = claimant.parent_of(harness);
        while let Some(parent) = next {
            if claimants.contains(parent.as_str()) {
                return Ok(true);
            }
            if !seen.insert(parent.clone()) {
                return Ok(false);
            }
            next = Self::session_key(conn, harness, &parent)?.and_then(|key| {
                key.parent_session_id.filter(|_| key.parent_harness == Some(harness))
            });
        }
        Ok(false)
    }

    /// [`Self::forget_host_sparing`], on the writer.
    pub(super) fn forget_host_in(
        conn: &mut Connection,
        host: HostId,
        spare: Option<HostId>,
    ) -> Result<Option<bool>, DbError> {
        let spare = spare.map(Self::host_repr);
        let host = Self::host_repr(host);
        let tag = RecordTag::AiSession.as_str();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        // The sessions going, as a JSON array of their row ids for `json_each`.
        let going: String = query_scalar(
            "SELECT json_group_array(id) FROM sessions WHERE host_id = ?1 OR id IN (SELECT \
             session FROM messages WHERE host = (SELECT id FROM interned WHERE value = ?1))",
        )
        .bind(&host)
        .fetch_one(&tx)?;

        let mut turns: BTreeSet<(i64, String)> = query_as(
            "SELECT DISTINCT m.harness, m.turn_id FROM messages m WHERE m.session IN (SELECT \
             value FROM json_each(?)) AND m.turn_id IS NOT NULL AND m.usage_present = 1",
        )
        .bind(&going)
        .fetch_all(&tx)?
        .into_iter()
        .collect();
        // Every other host with a row in the sessions going: NULL for a row of unknown host.
        let contributors: Vec<Option<String>> = query_scalar(
            "SELECT DISTINCT (SELECT value FROM interned WHERE id = m.host) FROM messages m WHERE \
             m.session IN (SELECT value FROM json_each(?1)) AND m.host IS NOT (SELECT id FROM \
             interned WHERE value = ?2)",
        )
        .bind(&going)
        .bind(&host)
        .fetch_all(&tx)?;
        if let Some(spare) = &spare
            && (*spare == host
                || contributors.iter().any(|c| c.as_ref().is_none_or(|c| c == spare)))
        {
            tx.rollback()?;
            return Ok(None);
        }

        // The sessions left below those going lose part of their ancestry.
        Self::mark_descendants(&tx, &going)?;
        for sql in [
            "DELETE FROM messages_fts WHERE rowid IN (SELECT rowid FROM messages WHERE session IN \
             (SELECT value FROM json_each(?)))",
            "DELETE FROM messages WHERE session IN (SELECT value FROM json_each(?))",
            "DELETE FROM calls WHERE (harness, session_id) IN (SELECT harness, session_id FROM \
             sessions WHERE id IN (SELECT value FROM json_each(?)))",
            "DELETE FROM sessions WHERE id IN (SELECT value FROM json_each(?))",
        ] {
            query(sql).bind(&going).execute(&tx)?;
        }
        // Sessions left grouped under a root that went.
        let orphaned: i64 = query_scalar(
            "SELECT count(*) FROM sessions c WHERE NOT EXISTS (SELECT 1 FROM sessions r WHERE \
             r.harness = c.root_harness AND r.session_id = c.root_session_id)",
        )
        .fetch_one(&tx)?;

        // Copies of a session that went link to the lowest-ranked original left, if any.
        let stranded: Vec<(i64, String)> = query_as(
            "SELECT harness, session_id FROM sessions s WHERE copy_of_session_id IS NOT NULL AND \
             NOT EXISTS (SELECT 1 FROM sessions o WHERE o.harness = s.harness AND o.session_id = \
             s.copy_of_session_id)",
        )
        .fetch_all(&tx)?;
        let mut relinked = false;
        for (harness, session_id) in &stranded {
            relinked |= Self::relink(&tx, *harness, session_id)?;
        }
        if orphaned > 0 || relinked {
            Self::regroup_all(&tx)?;
        }
        turns.extend(Self::take_regrouped_calls(&tx)?);
        for (harness, turn) in &turns {
            Self::attribute_call(&tx, *harness, turn)?;
        }

        if contributors.iter().any(Option::is_none) {
            query("DELETE FROM reproject_watermark WHERE tag = ?").bind(tag).execute(&tx)?;
        } else {
            for forgotten in contributors.iter().flatten().chain([&host]) {
                query("DELETE FROM reproject_watermark WHERE host = ? AND tag = ?")
                    .bind(forgotten)
                    .bind(tag)
                    .execute(&tx)?;
            }
        }
        query(BUMP_GENERATION).execute(&tx)?;

        tx.commit()?;
        Ok(Some(!contributors.is_empty()))
    }
}
