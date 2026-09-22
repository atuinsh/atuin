use std::path::{Path, PathBuf};

use atuin_common::db::sqlite::fts::{TextHighlighter, match_expression};
use atuin_common::db::sqlite::{Sqlite, SqliteOpenOrCreateError};
use atuin_common::db::{self};
use atuin_common::harnesstools::session::{Content, Role, SessionMeta, Usage};
use atuin_domain::record::RecordId;
use futures::{Stream, StreamExt, TryStreamExt};
use time::OffsetDateTime;
use tracing::warn;

use super::{
    HarnessKind, HarnessSession, Message, NativeSessionId, Session, SessionMatch, SourceId,
};

const COMPRESS_THRESHOLD: usize = 256;
const ZSTD_LEVEL: i32 = 3;
const REINDEX_CHUNK: i64 = 512;
const SNIPPET_TOKENS: usize = 32;

#[derive(Debug, Clone)]
pub struct AiSessionDatabase {
    db: Sqlite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appended {
    New,
    Duplicate,
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("failed to open the ai-session sqlite database: {0}")]
    Open(#[from] SqliteOpenOrCreateError),
    #[error("failed to run ai-session sqlite migrations: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("ai-session sqlite query failed: {0}")]
    Query(#[from] sqlx::Error),
    #[error("failed to serialize ai-session message: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to compress ai-session message content: {0}")]
    Compress(std::io::Error),
    #[error("stored ai-session timestamp is out of range: {0}")]
    Time(#[from] time::error::ComponentRange),
    #[error("unknown ai-session harness discriminant {0}")]
    UnknownHarness(i64),
    #[error("failed to decompress ai-session message content: {0}")]
    Decompress(std::io::Error),
    #[error("stored ai-session content is not valid utf-8")]
    InvalidContentEncoding,
    #[error("stored ai-session record id is not a valid uuid")]
    InvalidRecordId,
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    harness: i64,
    session_id: String,
    parent_harness: Option<i64>,
    parent_session_id: Option<String>,
    cwd: Option<String>,
    git_branch: Option<String>,
    model: Option<String>,
    started_at: i64,
    updated_at: i64,
    message_count: i64,
    usage_input: i64,
    usage_output: i64,
    usage_cache_read: i64,
    usage_cache_write: i64,
    title: Option<String>,
    preview: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: Vec<u8>,
    harness: i64,
    session_id: String,
    source_id: String,
    parent_harness: Option<i64>,
    parent_session_id: Option<String>,
    parent_source_id: Option<String>,
    thread: Option<String>,
    timestamp: i64,
    role: String,
    content: String,
    content_z: Option<Vec<u8>>,
    cwd: Option<String>,
    git_branch: Option<String>,
    model: Option<String>,
    usage_input: i64,
    usage_output: i64,
    usage_cache_read: i64,
    usage_cache_write: i64,
    stop_reason: Option<String>,
    usage_present: i64,
    turn_id: Option<String>,
}

#[derive(sqlx::FromRow)]
struct SearchRow {
    #[sqlx(flatten)]
    session: SessionRow,
    match_content: String,
    match_content_z: Option<Vec<u8>>,
    match_cwd: Option<String>,
    match_git_branch: Option<String>,
    match_model: Option<String>,
    score: f64,
}

#[derive(sqlx::FromRow)]
struct ReindexRow {
    rowid: i64,
    #[sqlx(flatten)]
    message: MessageRow,
    session_title: Option<String>,
}

impl AiSessionDatabase {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let db = Sqlite::builder(path.as_ref().as_os_str()).restrict_permissions().open().await?;
        let db = Self { db };
        db.migrate().await?;
        db.reindex().await?;
        Ok(db)
    }

    pub async fn in_memory() -> Result<Self, DbError> {
        let db = Sqlite::builder_in_memory().open().await?;
        let db = Self { db };
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> Result<(), DbError> {
        let pool = self.db.pool();
        db::migrate!(pool, "./src/ai_session/migrations").await?;
        Ok(())
    }

    pub async fn append(&self, msg: &Message) -> Result<Appended, DbError> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;

        let harness = msg.session.harness as i64;
        let session_id = msg.session.session.as_ref();
        let source_id = msg.source_id.as_ref();
        let id = msg.id.0.as_bytes().as_slice();

        let (parent_harness, parent_session_id) = match &msg.parent {
            Some(parent) => (Some(parent.harness as i64), Some(parent.session.as_ref().to_owned())),
            None => (None, None),
        };

        let timestamp = Self::millis(msg.timestamp);
        let role_json = serde_json::to_string(&msg.role)?;
        let content_json = serde_json::to_string(&msg.content)?;
        let (content, content_z) = Self::split_content(content_json)?;
        let stop_reason_json = msg.stop_reason.as_ref().map(serde_json::to_string).transpose()?;
        let (usage_input, usage_output, usage_cache_read, usage_cache_write) =
            Self::fold_usage(msg.usage.as_ref());
        let cwd = msg.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());

        let inserted = db::query(
            "INSERT INTO messages (
                id, harness, session_id, source_id, parent_harness, parent_session_id,
                parent_source_id, thread, timestamp, role, content, content_z, cwd, git_branch,
                model, usage_input, usage_output, usage_cache_read, usage_cache_write, stop_reason,
                usage_present, turn_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(harness, session_id, source_id) DO NOTHING",
        )
        .bind(id)
        .bind(harness)
        .bind(session_id)
        .bind(source_id)
        .bind(parent_harness)
        .bind(parent_session_id.clone())
        .bind(msg.parent_source_id.as_ref().map(|s| s.as_ref()))
        .bind(msg.thread.as_deref())
        .bind(timestamp)
        .bind(role_json)
        .bind(content)
        .bind(content_z)
        .bind(cwd.clone())
        .bind(msg.git_branch.as_deref())
        .bind(msg.model.as_deref())
        .bind(usage_input)
        .bind(usage_output)
        .bind(usage_cache_read)
        .bind(usage_cache_write)
        .bind(stop_reason_json)
        .bind(i64::from(msg.usage.is_some()))
        .bind(msg.turn_id.as_deref())
        .execute(&mut *tx)
        .await?;

        if inserted.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(Appended::Duplicate);
        }

        let rowid = inserted.last_insert_rowid();
        let body = Self::searchable_body(msg);
        db::query("INSERT INTO messages_fts(rowid, title, body) VALUES (?, ?, ?)")
            .bind(rowid)
            .bind(msg.session_title.as_deref().unwrap_or(""))
            .bind(&body)
            .execute(&mut *tx)
            .await?;

        // Session title denormalised onto the message so it survives a reproject from records
        // (which carry messages only). The session upsert below applies it latest-non-null-wins.
        let title = msg.session_title.as_deref();
        let preview = Self::preview_text(msg);

        db::query(
            "INSERT INTO sessions (
                harness, session_id, parent_harness, parent_session_id, cwd, git_branch, model,
                started_at, updated_at, message_count, usage_input, usage_output,
                usage_cache_read, usage_cache_write, title, preview
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(harness, session_id) DO UPDATE SET
                parent_harness = COALESCE(excluded.parent_harness, sessions.parent_harness),
                parent_session_id = COALESCE(excluded.parent_session_id, \
             sessions.parent_session_id),
                cwd = COALESCE(excluded.cwd, sessions.cwd),
                git_branch = COALESCE(excluded.git_branch, sessions.git_branch),
                model = COALESCE(excluded.model, sessions.model),
                started_at = MIN(sessions.started_at, excluded.started_at),
                updated_at = MAX(sessions.updated_at, excluded.updated_at),
                message_count = sessions.message_count + 1,
                usage_input = sessions.usage_input + excluded.usage_input,
                usage_output = sessions.usage_output + excluded.usage_output,
                usage_cache_read = sessions.usage_cache_read + excluded.usage_cache_read,
                usage_cache_write = sessions.usage_cache_write + excluded.usage_cache_write,
                title = COALESCE(excluded.title, sessions.title),
                preview = COALESCE(sessions.preview, excluded.preview)",
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
        .bind(usage_input)
        .bind(usage_output)
        .bind(usage_cache_read)
        .bind(usage_cache_write)
        .bind(title)
        .bind(preview)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(Appended::New)
    }

    pub async fn record_session_meta(
        &self,
        handle: &HarnessSession,
        meta: &SessionMeta,
    ) -> Result<(), DbError> {
        let now = Self::millis(OffsetDateTime::now_utc());
        let cwd = meta.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());
        let parent_harness = meta.parent.as_ref().map(|_| handle.harness as i64);
        let parent_session_id = meta.parent.as_ref().map(ToString::to_string);

        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;

        // Seed updated_at at 0, not `now`: appended messages set updated_at via
        // MAX(sessions.updated_at, excluded.updated_at), so a wall-clock seed would pin recency at
        // capture time and outrank every real message timestamp when an old session is replayed.
        db::query(
            "INSERT INTO sessions (
                harness, session_id, parent_harness, parent_session_id, cwd, git_branch, model,
                started_at, updated_at, message_count, usage_input, usage_output, \
             usage_cache_read, usage_cache_write, title
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, 0, 0, 0, 0, 0, ?)
            ON CONFLICT(harness, session_id) DO UPDATE SET
                parent_harness = COALESCE(sessions.parent_harness, excluded.parent_harness),
                parent_session_id = COALESCE(sessions.parent_session_id, \
             excluded.parent_session_id),
                cwd = COALESCE(sessions.cwd, excluded.cwd),
                git_branch = COALESCE(sessions.git_branch, excluded.git_branch),
                model = COALESCE(sessions.model, excluded.model),
                title = COALESCE(excluded.title, sessions.title)",
        )
        .bind(handle.harness as i64)
        .bind(handle.session.as_ref())
        .bind(parent_harness)
        .bind(parent_session_id)
        .bind(cwd)
        .bind(meta.git_branch.as_deref())
        .bind(meta.model.as_deref())
        .bind(now)
        .bind(meta.title.as_deref())
        .execute(&mut *tx)
        .await?;

        if let Some(title) = meta.title.as_deref() {
            // messages_fts is contentless, so a single-column UPDATE is not supported: rewrite
            // each of the session's index rows, re-deriving the body from the stored content.
            type BodyRow =
                (i64, String, Option<Vec<u8>>, Option<String>, Option<String>, Option<String>);
            let rows: Vec<BodyRow> = db::query_as(
                "SELECT rowid, content, content_z, cwd, git_branch, model FROM messages WHERE \
                 harness = ? AND session_id = ?",
            )
            .bind(handle.harness as i64)
            .bind(handle.session.as_ref())
            .fetch_all(&mut *tx)
            .await?;

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
                db::query(
                    "INSERT OR REPLACE INTO messages_fts(rowid, title, body) VALUES (?, ?, ?)",
                )
                .bind(rowid)
                .bind(title)
                .bind(&body)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }

    pub async fn contains_message(
        &self,
        session: &HarnessSession,
        source_id: &SourceId,
    ) -> Result<bool, DbError> {
        let found: Option<(i64,)> = db::query_as(
            "SELECT 1 FROM messages WHERE harness = ? AND session_id = ? AND source_id = ? LIMIT 1",
        )
        .bind(session.harness as i64)
        .bind(session.session.as_ref())
        .bind(source_id.as_ref())
        .fetch_optional(self.db.pool())
        .await?;

        Ok(found.is_some())
    }

    pub async fn get_session(&self, session: &HarnessSession) -> Result<Option<Session>, DbError> {
        let row: Option<SessionRow> = db::query_as(
            "SELECT harness, session_id, parent_harness, parent_session_id, cwd, git_branch, \
             model, started_at, updated_at, message_count, usage_input, usage_output, \
             usage_cache_read, usage_cache_write, title, preview FROM sessions WHERE harness = ? \
             AND session_id = ?",
        )
        .bind(session.harness as i64)
        .bind(session.session.as_ref())
        .fetch_optional(self.db.pool())
        .await?;

        row.map(Self::session_from_row).transpose()
    }

    pub async fn list_sessions(
        &self,
        harness: Option<HarnessKind>,
    ) -> Result<Vec<Session>, DbError> {
        let mut sql = String::from(
            "SELECT harness, session_id, parent_harness, parent_session_id, cwd, git_branch, \
             model, started_at, updated_at, message_count, usage_input, usage_output, \
             usage_cache_read, usage_cache_write, title, preview FROM sessions WHERE 1 = 1",
        );

        if harness.is_some() {
            sql.push_str(" AND harness = ?");
        }
        sql.push_str(" ORDER BY updated_at DESC");

        let mut query = db::query_as::<_, SessionRow>(sqlx::AssertSqlSafe(sql));
        if let Some(harness) = harness {
            query = query.bind(harness as i64);
        }

        let rows: Vec<SessionRow> = query.fetch_all(self.db.pool()).await?;
        rows.into_iter().map(Self::session_from_row).collect()
    }

    pub fn messages(
        &self,
        session: &HarnessSession,
    ) -> impl Stream<Item = Result<Message, DbError>> + Send + 'static {
        let pool = self.db.pool().clone();
        let harness = session.harness as i64;
        let session_id = session.session.as_ref().to_owned();

        async_stream::try_stream! {
            let mut rows = db::query_as::<_, MessageRow>(
                "SELECT id, harness, session_id, source_id, parent_harness, parent_session_id, \
                 parent_source_id, thread, timestamp, role, content, content_z, cwd, git_branch, \
                 model, usage_input, usage_output, usage_cache_read, usage_cache_write, \
                 stop_reason, usage_present, turn_id FROM messages WHERE harness = ? AND \
                 session_id = ? \
                 ORDER BY timestamp, id",
            )
            .bind(harness)
            .bind(session_id)
            .fetch(&pool);

            while let Some(row) = rows.try_next().await? {
                yield Self::message_from_row(row)?;
            }
        }
    }

    pub fn transcript(
        &self,
        session: &HarnessSession,
    ) -> impl Stream<Item = Result<String, DbError>> + Send + 'static {
        self.messages(session).map(|result| result.map(|msg| Self::render_transcript_chunk(&msg)))
    }

    pub fn search(
        &self,
        query: &str,
        harness: Option<HarnessKind>,
        limit: u32,
    ) -> impl Stream<Item = Result<SessionMatch, DbError>> + Send + 'static {
        let pool = self.db.pool().clone();
        let query = query.to_owned();

        async_stream::try_stream! {
            let Some(expr) = match_expression(&query) else {
                return;
            };

            let harness_clause = if harness.is_some() { " AND m.harness = ?" } else { "" };
            let limit_clause = if limit == 0 { "" } else { " LIMIT ?" };

            // messages_fts is contentless: it can rank (bm25) but cannot render highlight() or
            // snippet(), so the query returns the best message's stored content and the marking
            // happens in Rust below.
            let sql = format!(
                "WITH ranked AS MATERIALIZED (\
                 SELECT messages_fts.rowid AS rowid, m.harness AS h, m.session_id AS sid, \
                 -bm25(messages_fts) AS score \
                 FROM messages_fts JOIN messages m ON m.rowid = messages_fts.rowid \
                 WHERE messages_fts MATCH ?{harness_clause}), \
                 best AS (SELECT rowid, max(score) AS score FROM ranked \
                 GROUP BY h, sid ORDER BY score DESC{limit_clause}) \
                 SELECT s.harness, s.session_id, s.parent_harness, s.parent_session_id, s.cwd, \
                 s.git_branch, s.model, s.started_at, s.updated_at, s.message_count, \
                 s.usage_input, s.usage_output, s.usage_cache_read, s.usage_cache_write, s.title, \
                 s.preview, \
                 m.content AS match_content, m.content_z AS match_content_z, m.cwd AS match_cwd, \
                 m.git_branch AS match_git_branch, m.model AS match_model, \
                 best.score AS score FROM best \
                 JOIN messages m ON m.rowid = best.rowid \
                 JOIN sessions s ON s.harness = m.harness AND s.session_id = m.session_id \
                 ORDER BY best.score DESC, s.updated_at DESC, s.session_id",
            );

            let mut stmt = db::query_as::<_, SearchRow>(sqlx::AssertSqlSafe(sql)).bind(expr);
            if let Some(harness) = harness {
                stmt = stmt.bind(harness as i64);
            }
            if limit != 0 {
                stmt = stmt.bind(i64::from(limit));
            }

            let highlighter = TextHighlighter::default();
            let mut rows = stmt.fetch(&pool);
            while let Some(row) = rows.try_next().await? {
                let title = row.session.title.clone().unwrap_or_default();
                let body = Self::body_from_parts(
                    row.match_content,
                    row.match_content_z,
                    row.match_cwd.as_deref(),
                    row.match_git_branch.as_deref(),
                    row.match_model.as_deref(),
                )
                .unwrap_or_else(|err| {
                    warn!(?err, "failed to decode matched ai-session message; empty preview");
                    String::new()
                });
                let preview = Self::preview_snippet(&body, &query, SNIPPET_TOKENS);

                // No highlight spans are produced: consumers only render the plain text, so the
                // marker machinery isn't worth its keep. sanitize() strips any stray marker
                // codepoints in stored data so they can't masquerade as spans downstream.
                yield SessionMatch {
                    session: Self::session_from_row(row.session)?,
                    title: highlighter.as_highlighted(highlighter.sanitize(&title).into_owned()),
                    preview: highlighter
                        .as_highlighted(highlighter.sanitize(&preview).into_owned()),
                    score: row.score,
                };
            }
        }
    }

    pub async fn reindex(&self) -> Result<(), DbError> {
        let pool = self.db.pool();
        // messages_fts rowids are always a contiguous prefix of messages rowids: append writes the
        // message and its FTS row in one tx, and this backfill (the only other writer, running
        // under open() before the daemon serves) walks rowids ascending. So the highest indexed
        // rowid alone determines coverage — gate on max(rowid) (O(log N)) rather than counting
        // both tables. This also repopulates the index from scratch after a migration rebuilds
        // messages_fts.
        let mut watermark: i64 =
            db::query_scalar("SELECT coalesce(max(rowid), 0) FROM messages_fts")
                .fetch_one(pool)
                .await?;
        let last_message: i64 = db::query_scalar("SELECT coalesce(max(rowid), 0) FROM messages")
            .fetch_one(pool)
            .await?;
        if watermark >= last_message {
            return Ok(());
        }

        loop {
            let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
            let rows: Vec<ReindexRow> = db::query_as::<_, ReindexRow>(
                "SELECT m.rowid AS rowid, m.id, m.harness, m.session_id, m.source_id, \
                 m.parent_harness, m.parent_session_id, m.parent_source_id, m.thread, \
                 m.timestamp, m.role, m.content, m.content_z, m.cwd, m.git_branch, m.model, \
                 m.usage_input, m.usage_output, m.usage_cache_read, m.usage_cache_write, \
                 m.stop_reason, m.usage_present, m.turn_id, s.title AS session_title FROM \
                 messages m LEFT JOIN sessions s ON s.harness = m.harness AND s.session_id = \
                 m.session_id WHERE m.rowid > ? ORDER BY m.rowid LIMIT ?",
            )
            .bind(watermark)
            .bind(REINDEX_CHUNK)
            .fetch_all(&mut *tx)
            .await?;

            let Some(last) = rows.last() else {
                break;
            };
            watermark = last.rowid;

            for row in rows {
                let rowid = row.rowid;
                let title = row.session_title.unwrap_or_default();
                let body = match Self::message_from_row(row.message) {
                    Ok(msg) => Self::searchable_body(&msg),
                    Err(err) => {
                        warn!(?err, rowid, "failed to decode ai-session message; indexing empty");
                        String::new()
                    }
                };
                db::query(
                    "INSERT OR REPLACE INTO messages_fts(rowid, title, body) VALUES (?, ?, ?)",
                )
                .bind(rowid)
                .bind(&title)
                .bind(&body)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
        }
        Ok(())
    }

    /// Fold text the way the index's `unicode61` tokenizer does — lowercase with combining marks
    /// stripped — so preview placement agrees with what FTS5 actually matched (e.g. a query for
    /// `cafe` matches a stored `café`).
    fn fts_fold(text: &str) -> String {
        use unicode_normalization::UnicodeNormalization as _;
        use unicode_normalization::char::is_combining_mark;
        text.nfd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
    }

    /// The folded `unicode61`-style tokens (alphanumeric runs) of `text`.
    fn fts_tokens(text: &str) -> impl Iterator<Item = String> + '_ {
        Self::fts_fold(text)
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// The index of the first whitespace word where a query term matches the way the FTS index
    /// matched it: each term is a phrase of folded tokens that must appear consecutively in the
    /// body's token stream (so `app` matches the token `app`, not the word `apple`, and `foo-bar`
    /// matches `foo bar` across words).
    fn preview_hit(words: &[&str], query: &str) -> Option<usize> {
        let phrases: Vec<Vec<String>> = query
            .split_whitespace()
            .map(|term| Self::fts_tokens(term).collect())
            .filter(|p: &Vec<String>| !p.is_empty())
            .collect();
        if phrases.is_empty() {
            return None;
        }

        // (word index, folded token) stream over the whole body.
        let tokens: Vec<(usize, String)> = words
            .iter()
            .enumerate()
            .flat_map(|(i, w)| Self::fts_tokens(w).map(move |t| (i, t)))
            .collect();

        phrases
            .iter()
            .filter_map(|phrase| {
                (0..tokens.len().saturating_sub(phrase.len() - 1)).find(|&i| {
                    phrase.iter().zip(&tokens[i..]).all(|(p, (_, t))| p == t)
                })
            })
            .min()
            .map(|i| tokens[i].0)
    }

    /// `s` cut to at most `max` bytes on a char boundary.
    fn truncate_chars(s: &str, max: usize) -> &str {
        if s.len() <= max {
            return s;
        }
        let end = s.char_indices().take_while(|(i, _)| *i <= max).last().map_or(0, |(i, _)| i);
        &s[..end]
    }

    /// A plain preview of the matched message: up to `max_tokens` whitespace-separated words,
    /// windowed around the first FTS-style term match so it is visible, with `…` marking
    /// truncation. Bounded by a char budget so whitespace-free blobs (minified output) cannot
    /// blow up the preview. The replacement for FTS5's `snippet()`, which the contentless index
    /// cannot render.
    fn preview_snippet(body: &str, query: &str, max_tokens: usize) -> String {
        const MAX_CHARS: usize = 400;
        const LEAD_CHARS: usize = 80;

        let words: Vec<&str> = body.split_whitespace().collect();
        if words.is_empty() || max_tokens == 0 {
            return String::new();
        }

        let hit = Self::preview_hit(&words, query).unwrap_or(0);

        // Lead-in: a little context before the match, capped in words and chars so a giant
        // preceding blob cannot push the match itself out of the char budget.
        let mut start = hit;
        let mut lead = 0;
        while start > 0
            && hit - start < max_tokens / 8
            && lead + words[start - 1].len() < LEAD_CHARS
        {
            start -= 1;
            lead += words[start].len() + 1;
        }

        let mut out = String::new();
        if start > 0 {
            out.push('…');
        }
        let mut end = start;
        let mut clipped = false;
        for (i, word) in words.iter().enumerate().skip(start).take(max_tokens) {
            if i > start {
                if out.len() + 1 + word.len() > MAX_CHARS {
                    clipped = true;
                    break;
                }
                out.push(' ');
            }
            // The first (match-bearing) word always appears, truncated if it alone overflows.
            let room = MAX_CHARS.saturating_sub(out.len());
            let cut = Self::truncate_chars(word, room);
            clipped |= cut.len() < word.len();
            out.push_str(cut);
            end = i + 1;
        }
        if clipped || end < words.len() {
            out.push('…');
        }
        out
    }

    /// The searchable body for a message stored as raw columns: decode the (possibly compressed)
    /// content and append the metadata terms, mirroring [`Self::searchable_body`].
    fn body_from_parts(
        content: String,
        content_z: Option<Vec<u8>>,
        cwd: Option<&str>,
        git_branch: Option<&str>,
        model: Option<&str>,
    ) -> Result<String, DbError> {
        let contents = Self::read_content(content, content_z)?;
        let mut out = String::new();
        for content in &contents {
            Self::push_content_text(&mut out, content);
        }
        for extra in [cwd, git_branch, model].into_iter().flatten() {
            out.push_str(extra);
            out.push('\n');
        }
        Ok(out)
    }

    fn split_content(json: String) -> Result<(String, Option<Vec<u8>>), DbError> {
        if json.len() < COMPRESS_THRESHOLD {
            return Ok((json, None));
        }

        let compressed =
            zstd::stream::encode_all(json.as_bytes(), ZSTD_LEVEL).map_err(DbError::Compress)?;
        Ok((String::new(), Some(compressed)))
    }

    fn preview_text(msg: &Message) -> Option<String> {
        if msg.role != Role::User {
            return None;
        }

        msg.content.iter().find_map(|content| match content {
            Content::Text(text) => Some(text.clone()),
            _ => None,
        })
    }

    fn searchable_body(msg: &Message) -> String {
        let mut out = String::new();
        for content in &msg.content {
            Self::push_content_text(&mut out, content);
        }
        if let Some(cwd) = &msg.cwd {
            out.push_str(&cwd.to_string_lossy());
            out.push('\n');
        }
        if let Some(branch) = &msg.git_branch {
            out.push_str(branch);
            out.push('\n');
        }
        if let Some(model) = &msg.model {
            out.push_str(model);
            out.push('\n');
        }
        out
    }

    fn push_content_text(out: &mut String, content: &Content) {
        match content {
            Content::Text(text) | Content::Reasoning(text) => {
                out.push_str(text);
                out.push('\n');
            }
            Content::ToolUse(tool) => {
                out.push_str(&tool.name);
                out.push('\n');
                Self::push_json_text(out, &tool.input);
            }
            Content::ToolResult(result) => Self::push_json_text(out, &result.output),
            Content::Other(value) => Self::push_json_text(out, value),
            // Activity metadata is not conversational text and adds no useful search terms.
            Content::ReasoningSummary { .. } => {}
        }
    }

    fn push_json_text(out: &mut String, value: &serde_json::Value) {
        use std::fmt::Write as _;

        match value {
            serde_json::Value::String(text) => {
                out.push_str(text);
                out.push('\n');
            }
            serde_json::Value::Number(number) => {
                let _ = writeln!(out, "{number}");
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    Self::push_json_text(out, item);
                }
            }
            serde_json::Value::Object(entries) => {
                for (key, value) in entries {
                    out.push_str(key);
                    out.push('\n');
                    Self::push_json_text(out, value);
                }
            }
            serde_json::Value::Bool(_) | serde_json::Value::Null => {}
        }
    }

    fn fold_usage(usage: Option<&Usage>) -> (i64, i64, i64, i64) {
        let Some(usage) = usage else {
            return (0, 0, 0, 0);
        };

        (
            i64::try_from(usage.input.unwrap_or(0)).unwrap_or(i64::MAX),
            i64::try_from(usage.output.unwrap_or(0)).unwrap_or(i64::MAX),
            i64::try_from(usage.cache_read.unwrap_or(0)).unwrap_or(i64::MAX),
            i64::try_from(usage.cache_write.unwrap_or(0)).unwrap_or(i64::MAX),
        )
    }

    fn millis(ts: OffsetDateTime) -> i64 {
        i64::try_from(ts.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
    }

    fn time_from_millis(ms: i64) -> Result<OffsetDateTime, DbError> {
        Ok(OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)?)
    }

    fn harness_from_repr(n: i64) -> Result<HarnessKind, DbError> {
        match n {
            0 => Ok(HarnessKind::Unknown),
            1 => Ok(HarnessKind::ClaudeCode),
            2 => Ok(HarnessKind::Codex),
            3 => Ok(HarnessKind::Copilot),
            4 => Ok(HarnessKind::Opencode),
            5 => Ok(HarnessKind::Pi),
            other => Err(DbError::UnknownHarness(other)),
        }
    }

    fn optional_session(
        harness: Option<i64>,
        session_id: Option<String>,
    ) -> Result<Option<HarnessSession>, DbError> {
        match (harness, session_id) {
            (Some(harness), Some(session_id)) => Ok(Some(HarnessSession {
                harness: Self::harness_from_repr(harness)?,
                session: NativeSessionId::from(session_id),
            })),
            _ => Ok(None),
        }
    }

    fn read_content(content: String, content_z: Option<Vec<u8>>) -> Result<Vec<Content>, DbError> {
        let json = match content_z {
            Some(bytes) => {
                let decompressed =
                    zstd::stream::decode_all(bytes.as_slice()).map_err(DbError::Decompress)?;
                String::from_utf8(decompressed).map_err(|_| DbError::InvalidContentEncoding)?
            }
            None => content,
        };

        Ok(serde_json::from_str(&json)?)
    }

    fn message_from_row(row: MessageRow) -> Result<Message, DbError> {
        let harness = Self::harness_from_repr(row.harness)?;
        let parent = Self::optional_session(row.parent_harness, row.parent_session_id)?;
        let content = Self::read_content(row.content, row.content_z)?;
        let role: Role = serde_json::from_str(&row.role)?;
        let stop_reason = row.stop_reason.map(|s| serde_json::from_str(&s)).transpose()?;
        let id = uuid::Uuid::from_slice(&row.id).map_err(|_| DbError::InvalidRecordId)?;

        Ok(Message::builder()
            .id(RecordId(id))
            .session(HarnessSession {
                harness,
                session: NativeSessionId::from(row.session_id),
            })
            .source_id(SourceId::from(row.source_id))
            .parent(parent)
            .parent_source_id(row.parent_source_id.map(SourceId::from))
            .thread(row.thread)
            .timestamp(Self::time_from_millis(row.timestamp)?)
            .role(role)
            .content(content)
            .cwd(row.cwd.map(PathBuf::from))
            .git_branch(row.git_branch)
            .model(row.model)
            .usage((row.usage_present != 0).then(|| Usage {
                input: Some(u64::try_from(row.usage_input).unwrap_or(0)),
                output: Some(u64::try_from(row.usage_output).unwrap_or(0)),
                cache_read: Some(u64::try_from(row.usage_cache_read).unwrap_or(0)),
                cache_write: Some(u64::try_from(row.usage_cache_write).unwrap_or(0)),
            }))
            .stop_reason(stop_reason)
            .turn_id(row.turn_id)
            .build())
    }

    fn render_transcript_chunk(message: &Message) -> String {
        let role = match &message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
            Role::Tool => "tool",
            Role::Other(other) => other.as_str(),
        };

        let body = message
            .content
            .iter()
            .filter_map(|content| match content {
                Content::Text(text) | Content::Reasoning(text) => Some(text.clone()),
                Content::ReasoningSummary { tokens } => {
                    Some(atuin_common::harnesstools::session::model::reasoning_label(*tokens))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Trailing newline: chunks are concatenated verbatim by consumers, so the separator has
        // to live in the chunk or every message would run together on one line.
        format!("{role}: {body}\n")
    }

    fn session_from_row(row: SessionRow) -> Result<Session, DbError> {
        let harness = Self::harness_from_repr(row.harness)?;
        let parent = Self::optional_session(row.parent_harness, row.parent_session_id)?;

        Ok(Session::builder()
            .handle(HarnessSession {
                harness,
                session: NativeSessionId::from(row.session_id),
            })
            .parent(parent)
            .cwd(row.cwd.map(PathBuf::from))
            .git_branch(row.git_branch)
            .model(row.model)
            .started_at(Self::time_from_millis(row.started_at)?)
            .updated_at(Self::time_from_millis(row.updated_at)?)
            .message_count(u64::try_from(row.message_count).unwrap_or(0))
            .usage(Usage {
                input: Some(u64::try_from(row.usage_input).unwrap_or(0)),
                output: Some(u64::try_from(row.usage_output).unwrap_or(0)),
                cache_read: Some(u64::try_from(row.usage_cache_read).unwrap_or(0)),
                cache_write: Some(u64::try_from(row.usage_cache_write).unwrap_or(0)),
            })
            .title(row.title)
            .preview(row.preview)
            .build())
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::harnesstools::session::{
        Content, Role, SessionMeta, ToolCallId, ToolResult, ToolUse, Usage,
    };
    use atuin_domain::record::RecordId;
    use futures::TryStreamExt;
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::{AiSessionDatabase, Appended, COMPRESS_THRESHOLD};
    use crate::ai_session::{
        HarnessKind, HarnessSession, Message, NativeSessionId, SessionMatch, SourceId,
    };

    fn sample_message() -> Message {
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(HarnessSession {
                harness: HarnessKind::ClaudeCode,
                session: NativeSessionId::from("native-session".to_owned()),
            })
            .source_id(SourceId::from("source-id".to_owned()))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .role(Role::User)
            .content(vec![Content::Text("hello".to_owned())])
            .build()
    }

    #[rstest]
    #[tokio::test]
    async fn append_is_idempotent_on_native_triple() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let mut m = sample_message();
        m.turn_id = Some("msg_01".to_owned());
        assert_eq!(db.append(&m).await.unwrap(), Appended::New);
        assert_eq!(db.append(&m).await.unwrap(), Appended::Duplicate);

        let s = db.get_session(&m.session).await.unwrap().unwrap();
        assert_eq!(s.message_count, 1);
        let got: Vec<_> = db.messages(&m.session).try_collect().await.unwrap();
        assert_eq!(got[0].turn_id.as_deref(), Some("msg_01"));
    }

    fn message_in(session: &HarnessSession, index: i64, text: &str) -> Message {
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(session.clone())
            .source_id(SourceId::from(format!("source-{index}")))
            .timestamp(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(index))
            .role(Role::User)
            .content(vec![Content::Text(text.to_owned())])
            .build()
    }

    fn sample_handle() -> HarnessSession {
        HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from("ordered-session".to_owned()),
        }
    }

    fn three_sessions_oldest_first() -> Vec<Message> {
        (0..3)
            .map(|i| {
                let session = HarnessSession {
                    harness: HarnessKind::ClaudeCode,
                    session: NativeSessionId::from(format!("session-{i}")),
                };
                message_in(&session, i, "hello")
            })
            .collect()
    }

    fn ordered_messages(session: &HarnessSession) -> Vec<Message> {
        (0..3).map(|i| message_in(session, i, &format!("message {i}"))).collect()
    }

    #[rstest]
    #[tokio::test]
    async fn list_is_newest_first() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for m in three_sessions_oldest_first() {
            db.append(&m).await.unwrap();
        }

        let sessions = db.list_sessions(None).await.unwrap();
        assert_eq!(sessions.len(), 3);
        assert!(sessions.windows(2).all(|w| w[0].updated_at >= w[1].updated_at));
    }

    #[rstest]
    #[tokio::test]
    async fn messages_come_back_in_order() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        for m in ordered_messages(&session) {
            db.append(&m).await.unwrap();
        }
        let got: Vec<_> = db.messages(&session).try_collect().await.unwrap();
        assert!(got.windows(2).all(|w| w[0].timestamp <= w[1].timestamp));
    }

    #[rstest]
    #[tokio::test]
    async fn same_timestamp_orders_by_capture_id_not_source_id() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        let ts = OffsetDateTime::UNIX_EPOCH;

        // Two messages sharing a stored timestamp; source_ids sort opposite to capture order, so
        // ordering by source_id would reverse them. The monotonic capture id must break the tie.
        let first = Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(session.clone())
            .source_id(SourceId::from("zzz-first".to_owned()))
            .timestamp(ts)
            .role(Role::User)
            .content(vec![Content::Text("first".to_owned())])
            .build();
        let second = Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(session.clone())
            .source_id(SourceId::from("aaa-second".to_owned()))
            .timestamp(ts)
            .role(Role::User)
            .content(vec![Content::Text("second".to_owned())])
            .build();

        db.append(&first).await.unwrap();
        db.append(&second).await.unwrap();

        let got: Vec<_> = db.messages(&session).try_collect().await.unwrap();
        assert!(
            got.windows(2).all(|w| w[0].id.0 <= w[1].id.0),
            "same-timestamp messages must order by monotonic capture id, not source_id"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn content_z_round_trips_compressed_message() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        let long_text: String =
            (0..64).map(|i| format!("segment-{i:03}-distinct ")).collect::<String>();
        assert!(long_text.len() >= COMPRESS_THRESHOLD);

        let m = message_in(&session, 0, &long_text);
        db.append(&m).await.unwrap();

        let got: Vec<_> = db.messages(&session).try_collect().await.unwrap();
        assert_eq!(got.len(), 1);

        let text = match got[0].content.first() {
            Some(Content::Text(text)) => text.clone(),
            _ => panic!("expected Content::Text"),
        };
        assert_eq!(text, long_text);
    }

    #[rstest]
    #[tokio::test]
    async fn contains_message_reflects_presence() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let m = sample_message();
        assert!(!db.contains_message(&m.session, &m.source_id).await.unwrap());
        db.append(&m).await.unwrap();
        assert!(db.contains_message(&m.session, &m.source_id).await.unwrap());
    }

    #[rstest]
    #[tokio::test]
    async fn started_at_tracks_earliest_message() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        // Later message arrives first, then an earlier one (out-of-order sync / replay).
        db.append(&message_in(&session, 100, "later")).await.unwrap();
        db.append(&message_in(&session, 50, "earlier")).await.unwrap();

        let s = db.get_session(&session).await.unwrap().unwrap();
        assert_eq!(s.started_at, OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(50));
    }

    #[rstest]
    #[tokio::test]
    async fn updated_at_tracks_last_message_not_capture_time() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        // Metadata is recorded first (a rediscovered old session), then historical messages replay.
        // updated_at must reflect the newest message, not the wall-clock capture instant.
        db.record_session_meta(&session, &SessionMeta::default()).await.unwrap();
        db.append(&message_in(&session, 50, "old")).await.unwrap();

        let s = db.get_session(&session).await.unwrap().unwrap();
        let ts = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(50);
        assert_eq!(s.updated_at, ts);
        assert_eq!(s.started_at, ts);
    }

    #[rstest]
    #[tokio::test]
    async fn usage_absence_round_trips_as_none() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();

        // A message with no usage block must not come back as Some(zeros).
        db.append(&message_in(&session, 0, "no usage")).await.unwrap();

        let with_usage = Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(session.clone())
            .source_id(SourceId::from("with-usage".to_owned()))
            .timestamp(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1))
            .role(Role::Assistant)
            .content(vec![Content::Text("has usage".to_owned())])
            .usage(Some(Usage {
                input: Some(0),
                output: Some(0),
                cache_read: Some(0),
                cache_write: Some(0),
            }))
            .build();
        db.append(&with_usage).await.unwrap();

        let got: Vec<_> = db.messages(&session).try_collect().await.unwrap();
        assert_eq!(got[0].usage, None, "absent usage must not become Some(zeros)");
        assert!(got[1].usage.is_some(), "reported usage must survive the round trip");
    }

    #[rstest]
    #[tokio::test]
    async fn transcript_separates_messages() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        for m in ordered_messages(&session) {
            db.append(&m).await.unwrap();
        }

        let chunks: Vec<String> = db.transcript(&session).try_collect().await.unwrap();
        assert!(chunks.iter().all(|c| c.ends_with('\n')), "each chunk must be newline-terminated");
        assert_eq!(chunks.concat().lines().count(), 3, "messages must not run together");
    }

    fn handle(harness: HarnessKind, id: &str) -> HarnessSession {
        HarnessSession {
            harness,
            session: NativeSessionId::from(id.to_owned()),
        }
    }

    fn message_with(
        session: &HarnessSession,
        index: i64,
        role: Role,
        content: Vec<Content>,
    ) -> Message {
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(session.clone())
            .source_id(SourceId::from(format!("source-{index}")))
            .timestamp(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(index))
            .role(role)
            .content(content)
            .build()
    }

    async fn search(db: &AiSessionDatabase, query: &str) -> Vec<SessionMatch> {
        db.search(query, None, 0).try_collect().await.unwrap()
    }

    #[rstest]
    #[tokio::test]
    async fn search_finds_a_message_by_its_text() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        db.append(&message_in(&session, 0, "the flaky nextest race")).await.unwrap();

        let hits = search(&db, "flaky race").await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.handle, session);
    }

    #[rstest]
    #[tokio::test]
    async fn search_matches_reasoning_tool_calls_results_and_metadata() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        let mut message = message_with(
            &session,
            0,
            Role::Assistant,
            vec![
                Content::Reasoning("weighing the tradeoffs".to_owned()),
                Content::ToolUse(ToolUse {
                    id: ToolCallId::from("call-1".to_owned()),
                    name: "execute_shell_command".to_owned(),
                    input: serde_json::json!({ "command": "cargo nextest run" }),
                }),
                Content::ToolResult(ToolResult {
                    call: ToolCallId::from("call-1".to_owned()),
                    output: serde_json::json!({ "stderr": "ENOSPC no space left" }),
                    error: true,
                }),
                Content::Other(serde_json::json!({ "note": "peculiar" })),
            ],
        );
        message.cwd = Some(std::path::PathBuf::from("/home/marko/atuin"));
        message.git_branch = Some("feat/ai-session-fts".to_owned());
        message.model = Some("claude-opus".to_owned());
        message.session_title = Some("Adding full-text search".to_owned());
        db.append(&message).await.unwrap();

        for query in [
            "tradeoffs",             // reasoning
            "execute_shell_command", // tool name
            "nextest",               // tool input
            "ENOSPC",                // tool result output
            "peculiar",              // Content::Other
            "atuin",                 // cwd
            "feat",                  // git branch
            "opus",                  // model
            "full-text",             // session title
        ] {
            assert_eq!(search(&db, query).await.len(), 1, "query {query:?} should match");
        }
    }

    #[rstest]
    #[tokio::test]
    async fn search_filters_by_harness() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let claude = handle(HarnessKind::ClaudeCode, "claude");
        let codex = handle(HarnessKind::Codex, "codex");
        db.append(&message_in(&claude, 0, "shared keyword")).await.unwrap();
        db.append(&message_in(&codex, 1, "shared keyword")).await.unwrap();

        let all: Vec<_> = db.search("shared", None, 0).try_collect().await.unwrap();
        assert_eq!(all.len(), 2);

        let only_codex: Vec<_> =
            db.search("shared", Some(HarnessKind::Codex), 0).try_collect().await.unwrap();
        assert_eq!(only_codex.len(), 1);
        assert_eq!(only_codex[0].session.handle, codex);
    }

    #[rstest]
    #[tokio::test]
    async fn search_limit_bounds_the_number_of_sessions() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for i in 0..5 {
            let session = handle(HarnessKind::ClaudeCode, &format!("session-{i}"));
            db.append(&message_in(&session, i, "common term")).await.unwrap();
        }

        let two: Vec<_> = db.search("common", None, 2).try_collect().await.unwrap();
        assert_eq!(two.len(), 2);
        let all: Vec<_> = db.search("common", None, 0).try_collect().await.unwrap();
        assert_eq!(all.len(), 5);
    }

    #[rstest]
    #[case::empty("")]
    #[case::whitespace("   \t\n ")]
    #[case::punctuation_only("()")]
    #[case::operator_soup("***")]
    #[tokio::test]
    async fn degenerate_queries_yield_no_results_without_error(#[case] query: &str) {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        db.append(&message_in(&sample_handle(), 0, "real content here")).await.unwrap();
        assert!(search(&db, query).await.is_empty());
    }

    #[rstest]
    #[tokio::test]
    async fn fts5_operators_in_the_query_are_matched_literally() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        db.append(&message_in(&sample_handle(), 0, "connection refused: retrying")).await.unwrap();
        // `refused:` is an FTS5 column filter raw; it must match the literal token, not error.
        assert_eq!(search(&db, "refused:").await.len(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn compressed_message_is_searchable() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        let long: String = (0..64).map(|i| format!("segment-{i:03}-needle ")).collect();
        assert!(long.len() >= COMPRESS_THRESHOLD);
        db.append(&message_in(&session, 0, &long)).await.unwrap();

        assert_eq!(search(&db, "needle").await.len(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn unicode_tokens_are_searchable() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        db.append(&message_in(&sample_handle(), 0, "café エラー logs")).await.unwrap();
        assert_eq!(search(&db, "cafe").await.len(), 1, "unicode61 folds diacritics");
        assert_eq!(search(&db, "エラー").await.len(), 1, "a whole CJK token is matchable");
    }

    #[rstest]
    #[tokio::test]
    async fn duplicate_append_does_not_double_index() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let message = message_in(&sample_handle(), 0, "idempotent needle");
        assert_eq!(db.append(&message).await.unwrap(), Appended::New);
        assert_eq!(db.append(&message).await.unwrap(), Appended::Duplicate);

        assert_eq!(search(&db, "needle").await.len(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn matches_collapse_to_one_hit_per_session() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        db.append(&message_in(&session, 0, "recurring topic here")).await.unwrap();
        db.append(&message_in(&session, 1, "recurring topic again")).await.unwrap();

        let hits = search(&db, "recurring").await;
        assert_eq!(hits.len(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn a_chatty_session_never_crowds_out_others() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let chatty = handle(HarnessKind::ClaudeCode, "chatty");
        for i in 0..40 {
            db.append(&message_in(&chatty, i, "shared keyword")).await.unwrap();
        }
        let quiet = handle(HarnessKind::ClaudeCode, "quiet");
        db.append(&message_in(&quiet, 100, "shared keyword")).await.unwrap();

        let two: Vec<_> = db.search("keyword", None, 2).try_collect().await.unwrap();
        assert_eq!(two.len(), 2, "ranking is per session: one chatty session takes one slot");
        assert!(
            two.iter().any(|m| m.session.handle == quiet),
            "the single-message session must not be dropped"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn search_preview_contains_the_matched_term() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        db.append(&message_in(&sample_handle(), 0, "the distinctive marker word")).await.unwrap();

        let hits = search(&db, "distinctive").await;
        assert_eq!(hits.len(), 1);
        assert!(hits[0].preview.to_plain().text.contains("distinctive"));
    }

    #[rstest]
    #[tokio::test]
    async fn search_preview_folds_diacritics_like_the_index() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let long: String = (0..200).map(|i| format!("filler-{i:03} ")).collect();
        db.append(&message_in(&sample_handle(), 0, &format!("{long}the café burned")))
            .await
            .unwrap();

        // unicode61 folds diacritics, so `cafe` matches the stored `café`; the preview window
        // must agree with the index and land on the match, not fall back to the leading words.
        let hits = search(&db, "cafe").await;
        assert_eq!(hits.len(), 1);
        let preview = hits[0].preview.to_plain().text.into_owned();
        assert!(preview.contains("café"), "the folded match must be in the window: {preview:?}");
        assert!(!preview.contains("filler-000"), "must not fall back to the leading window");
    }

    #[rstest]
    #[tokio::test]
    async fn search_preview_matches_whole_tokens_not_substrings() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let filler: String = (0..200).map(|i| format!("filler-{i:03} ")).collect();
        // `apple` contains `app` as a substring but is a different FTS token; the window must
        // land on the real token match at the end, not the substring false-positive up front.
        db.append(&message_in(&sample_handle(), 0, &format!("apple {filler}app end")))
            .await
            .unwrap();

        let hits = search(&db, "app").await;
        assert_eq!(hits.len(), 1);
        let preview = hits[0].preview.to_plain().text.into_owned();
        assert!(preview.contains("app end"), "the token match must be visible: {preview:?}");
        assert!(!preview.contains("apple"), "substring look-alikes must not anchor the window");
    }

    #[rstest]
    #[tokio::test]
    async fn search_preview_finds_phrases_across_words() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let filler: String = (0..200).map(|i| format!("filler-{i:03} ")).collect();
        // FTS tokenizes `foo-bar` as the phrase [foo, bar], which matches `foo bar` across a
        // space; the preview's matcher must agree.
        db.append(&message_in(&sample_handle(), 0, &format!("{filler}foo bar tail")))
            .await
            .unwrap();

        let hits = search(&db, "foo-bar").await;
        assert_eq!(hits.len(), 1);
        let preview = hits[0].preview.to_plain().text.into_owned();
        assert!(preview.contains("foo bar"), "the phrase match must be visible: {preview:?}");
        assert!(!preview.contains("filler-000"), "must not fall back to the leading window");
    }

    #[rstest]
    #[tokio::test]
    async fn search_preview_is_char_bounded() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        // A whitespace-free blob (minified output) is one "word"; the char budget must keep the
        // preview small and the match visible instead of returning the whole line.
        let blob = "x".repeat(5_000);
        db.append(&message_in(&sample_handle(), 0, &format!("{blob} needle here")))
            .await
            .unwrap();

        let hits = search(&db, "needle").await;
        assert_eq!(hits.len(), 1);
        let preview = hits[0].preview.to_plain().text.into_owned();
        assert!(preview.contains("needle"), "the match must survive the char cap: {preview:?}");
        assert!(preview.chars().count() < 450, "preview must be bounded: {} chars", preview.len());

        // Wide windows of normal words are char-capped too.
        let wide: String = (0..32).map(|i| format!("wordy-{i:02}-{} ", "y".repeat(40))).collect();
        db.append(&message_in(&sample_handle(), 1, &format!("target {wide}"))).await.unwrap();
        let hits = search(&db, "target").await;
        let preview = hits[0].preview.to_plain().text.into_owned();
        assert!(preview.starts_with("target"), "the match must lead the window: {preview:?}");
        assert!(preview.chars().count() < 450, "preview must be bounded: {} chars", preview.len());
        assert!(preview.ends_with('…'), "char-cap truncation must be marked: {preview:?}");
    }

    #[rstest]
    #[tokio::test]
    async fn search_preview_windows_around_a_late_match() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let long: String = (0..200).map(|i| format!("filler-{i:03} ")).collect();
        db.append(&message_in(&sample_handle(), 0, &format!("{long}buried-needle here")))
            .await
            .unwrap();

        let hits = search(&db, "buried-needle").await;
        assert_eq!(hits.len(), 1);
        let preview = hits[0].preview.to_plain().text.into_owned();
        assert!(preview.contains("buried-needle"), "the match must be in the window: {preview:?}");
        assert!(preview.starts_with('…'), "leading truncation must be marked: {preview:?}");
        assert!(!preview.contains("filler-000"), "the window must not span the whole body");
    }

    #[rstest]
    #[tokio::test]
    async fn reindex_backfills_messages_indexed_before_the_fts_row_existed() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        let long: String = (0..64).map(|i| format!("chunk-{i:03}-buried ")).collect();
        assert!(long.len() >= COMPRESS_THRESHOLD);
        db.append(&message_in(&session, 0, &long)).await.unwrap();
        db.append(&message_in(&session, 1, "plain buried token")).await.unwrap();

        // Simulate a sidecar upgraded to the FTS migration with no index rows yet.
        atuin_common::db::query("DELETE FROM messages_fts").execute(db.db.pool()).await.unwrap();
        assert!(search(&db, "buried").await.is_empty());

        db.reindex().await.unwrap();
        assert_eq!(search(&db, "buried").await.len(), 1);

        // A second pass must not duplicate the index or change results.
        db.reindex().await.unwrap();
        assert_eq!(search(&db, "buried").await.len(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn reindex_resumes_from_an_interrupted_backfill() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        db.append(&message_in(&session, 0, "alpha unique-a")).await.unwrap();
        db.append(&message_in(&session, 1, "beta unique-b")).await.unwrap();
        db.append(&message_in(&session, 2, "gamma unique-c")).await.unwrap();

        atuin_common::db::query(
            "DELETE FROM messages_fts WHERE rowid = (SELECT max(rowid) FROM messages_fts)",
        )
        .execute(db.db.pool())
        .await
        .unwrap();
        assert!(search(&db, "unique-c").await.is_empty(), "the highest-rowid row is unindexed");
        assert_eq!(search(&db, "unique-a").await.len(), 1, "the prefix stays indexed");

        db.reindex().await.unwrap();
        assert_eq!(search(&db, "unique-c").await.len(), 1, "reindex fills the suffix gap");
    }

    #[rstest]
    #[tokio::test]
    async fn a_renamed_session_is_searchable_by_its_new_title() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        let mut message = message_in(&session, 0, "unrelated body text");
        message.session_title = Some("AlphaTitle".to_owned());
        db.append(&message).await.unwrap();
        assert_eq!(search(&db, "AlphaTitle").await.len(), 1, "the original title is searchable");

        db.record_session_meta(
            &session,
            &SessionMeta {
                title: Some("BetaTitle".to_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(search(&db, "BetaTitle").await.len(), 1, "the new title becomes searchable");
        assert!(search(&db, "AlphaTitle").await.is_empty(), "the retired title no longer matches");
    }
}
