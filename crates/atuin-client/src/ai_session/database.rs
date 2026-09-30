use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atuin_common::db::sqlite::fts::{
    TextHighlighter, match_any_expression, match_expression, prefix_match_expression,
};
use atuin_common::db::sqlite::{Sqlite, SqliteOpenOrCreateError};
use atuin_common::db::{self};
use atuin_common::harnesstools::session::{
    Checkpoint, Content, ParentKind, Role, TitleChange, TitleSource, Usage,
};
use atuin_common::string::TruncateCharsExt;
use atuin_common::string::highlighted::HighlightedString;
use atuin_domain::record::{HostId, RecordId, RecordTag};
use futures::{Stream, StreamExt, TryStreamExt};
use sqlx::SqliteConnection;
use time::OffsetDateTime;
use tracing::warn;

use super::{
    HarnessKind, HarnessSession, MatchedSession, Message, NativeSessionId, SearchTerms, Session,
    SessionFilter, SessionMatch, SourceId,
};

mod watermark;
pub use watermark::{Generation, Watermark};

const COMPRESS_THRESHOLD: usize = 256;
const ZSTD_LEVEL: i32 = 3;
const REINDEX_CHUNK: i64 = 512;
const SNIPPET_TOKENS: usize = 32;

/// The newest migration this build knows: [`AiSessionDatabase::open_read_only`] reads only a
/// sidecar at exactly this version.
const SCHEMA_VERSION: i64 = 6;

/// The migrations this build runs, read for their versions and checksums only (they are run
/// through [`db::migrate!`]), to tell a sidecar another build migrated.
#[allow(clippy::disallowed_macros)]
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./src/ai_session/migrations");

/// How much more a title match weighs than a body match in bm25.
const TITLE_WEIGHT: f64 = 5.0;

/// A search hit's relevance is divided by `1 + age / RECENCY_DAYS`: one this many days older than
/// another ranks as if half as relevant.
const RECENCY_DAYS: f64 = 30.0;

const DAY_MILLIS: f64 = 86_400_000.0;

/// The shortest last term that matches as a prefix. A one-character prefix matches most of the
/// index (every row with a word starting `e`), which costs hundreds of milliseconds to rank for
/// nothing useful, so a lone character matches only as a whole token until the next keystroke.
const MIN_PREFIX_CHARS: usize = 2;

/// The `sessions` columns [`SessionRow`] reads, from a table aliased `s`. Every query also selects
/// `child_count` and `group_updated_at`.
macro_rules! session_columns {
    () => {
        "s.harness, s.session_id, s.parent_harness, s.parent_session_id, s.cwd, s.git_branch, \
         s.model, s.started_at, s.updated_at, s.message_count, s.usage_input, s.usage_output, \
         s.usage_cache_read, s.usage_cache_write, s.usage_reasoning, s.title, s.title_source, \
         s.preview, s.last_reply, s.parent_kind, s.host_id, s.root_harness, s.root_session_id, \
         s.copy_of_session_id"
    };
}

/// The `messages` columns [`MessageRow`] reads, from a table aliased `m`. A row's parent kind is
/// its session's: rows do not store one.
macro_rules! message_columns {
    () => {
        "m.id, m.harness, (SELECT session_id FROM sessions WHERE id = m.session) AS session_id, \
         (SELECT parent_kind FROM sessions WHERE id = m.session) AS parent_kind, m.source_id, \
         m.parent_harness, m.parent_session_id, m.parent_source_id, m.timestamp, m.role, \
         m.content, m.content_z, m.cwd, m.git_branch, m.model, m.usage_input, m.usage_output, \
         m.usage_cache_read, m.usage_cache_write, m.usage_reasoning, m.stop_reason, \
         m.usage_present, m.turn_id, m.title_change, m.host_id"
    };
}

/// Whether the session `c` is a subagent, or a child of unknown kind: what
/// [`Session::relation`] tells from its parent kind (`ParentKind` as [`parent_kind_repr`] stores
/// it), else from its harness and id. Such a session is grouped under its root but not counted in
/// its size.
///
/// [`parent_kind_repr`]: AiSessionDatabase::parent_kind_repr
macro_rules! subagent_like {
    () => {
        "(c.parent_session_id IS NOT NULL AND (c.parent_kind IS 0 OR (c.parent_kind IS NULL AND \
         c.parent_harness = c.harness AND (c.harness NOT IN (1, 5) OR (c.harness = 1 AND \
         substr(c.session_id, 1, 6) = 'agent-')))))"
    };
}

/// The group size and newest activity of a root `s`, for roots-only queries. The size counts the
/// sessions a person carried on ([`crate::ai_session::SessionRelation::carries_on`]): the forks and
/// continuations, not the subagents, whose activity still counts toward the group's.
macro_rules! group_columns {
    () => {
        concat!(
            "(SELECT count(*) FROM sessions c WHERE c.root_harness = s.harness AND \
             c.root_session_id = s.session_id AND NOT (c.harness = s.harness AND c.session_id = \
             s.session_id) AND NOT ",
            subagent_like!(),
            ") AS child_count, (SELECT max(c.updated_at) FROM sessions c WHERE c.root_harness = \
             s.harness AND c.root_session_id = s.session_id) AS group_updated_at"
        )
    };
}

/// Whether the row `m`'s `turn_id` is an id its harness gave the model call, which a copy of the
/// row keeps and nothing else holds: only such an id links a copy to its original (see
/// [`AiSessionDatabase::link_copies`]). Not one capture derived from the line's content, because
/// unrelated sessions can share that:
///
/// - Codex lines with no `response_id` are keyed by their token counts (`token_count:<total>`,
///   `token_count:<time>:<last>` and `thread:<total>`, harnesstools `codex::session`
///   `usage_turn`), which two fresh sessions sent the same first prompt report alike.
/// - Pi lines with no entry id are keyed by their kind, timestamps and token counts
///   (`line:...`, harnesstools `pi::session` `turn_id`).
/// - opencode assistant messages are keyed by their creation time and model (`<millis>:...`,
///   harnesstools `opencode::session` `turn_of` and `v2` `turn`). opencode records a fork's
///   original as its parent anyway.
///
/// Usage is still counted once per turn id whatever its form: this only keeps the id from
/// linking sessions.
macro_rules! linkable_turn {
    () => {
        "(m.turn_id IS NOT NULL AND NOT ((m.harness = 2 AND (m.turn_id GLOB 'token_count:*' OR \
         m.turn_id GLOB 'thread:*')) OR (m.harness = 5 AND m.turn_id GLOB 'line:*') OR (m.harness \
         = 4 AND m.turn_id GLOB '[0-9]*')))"
    };
}

/// [`group_columns`] for queries that do not group.
macro_rules! no_group_columns {
    () => {
        "0 AS child_count, NULL AS group_updated_at"
    };
}

#[derive(Debug, Clone)]
pub struct AiSessionDatabase {
    db: Sqlite,
    /// See [`Self::lock_local_projection`]. Shared by clones, so by everything in the one process
    /// (the daemon) that writes the sidecar.
    local_projection: Arc<tokio::sync::Mutex<()>>,
    /// See [`Self::lock_reprojection`]. Shared by clones, like `local_projection`.
    reprojection: Arc<tokio::sync::Mutex<()>>,
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
    /// No daemon has created the sidecar yet.
    #[error(
        "the AI session database has not been created yet: start the atuin daemon (`atuin daemon \
         start`) and try again"
    )]
    Uninitialized {
        expected: i64,
    },
    /// The sidecar is at an older schema than this build reads: the daemon that owns it is an
    /// older atuin still running since an upgrade, or is migrating it right now.
    #[error(
        "the AI session database is at schema version {found}, older than the version {expected} \
         this atuin reads: the atuin daemon upgrades it when it starts. If atuin was just \
         updated, restart the daemon (`atuin daemon restart`); if the daemon is starting, try \
         again in a moment"
    )]
    OutdatedSchema {
        found: i64,
        expected: i64,
    },
    /// The sidecar was migrated by a build whose migrations differ from this one's (a development
    /// build): the daemon rebuilds it from the record store when it next starts.
    #[error(
        "the AI session database was made by another build of atuin: the atuin daemon rebuilds it \
         when it starts. Restart the daemon (`atuin daemon restart`), or if it is starting, try \
         again in a moment"
    )]
    ForeignSchema,
    /// The sidecar could not be removed, to be rebuilt.
    #[error("failed to remove the ai-session sqlite database to rebuild it: {0}")]
    Remove(std::io::Error),
    /// The sidecar is at a newer schema than this build reads: a newer atuin's daemon owns it.
    #[error(
        "the AI session database is at schema version {found}, newer than the version {expected} \
         this atuin reads: the atuin daemon is newer than this atuin. Update atuin, or check that \
         the shell and the daemon run the same atuin binary"
    )]
    UnknownSchema {
        found: i64,
        expected: i64,
    },
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
    usage_reasoning: i64,
    title: Option<String>,
    title_source: Option<i64>,
    preview: Option<String>,
    last_reply: Option<String>,
    parent_kind: Option<i64>,
    host_id: Option<String>,
    root_harness: Option<i64>,
    root_session_id: Option<String>,
    copy_of_session_id: Option<String>,
    child_count: i64,
    group_updated_at: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: Vec<u8>,
    harness: i64,
    session_id: String,
    source_id: String,
    parent_harness: Option<i64>,
    parent_session_id: Option<String>,
    parent_kind: Option<i64>,
    parent_source_id: Option<String>,
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
    usage_reasoning: Option<i64>,
    stop_reason: Option<String>,
    usage_present: i64,
    turn_id: Option<String>,
    title_change: Option<String>,
    host_id: Option<String>,
}

/// Input, output, cache read, cache write, reasoning: the `usage_*` columns in order.
type Tokens = [i64; 5];

#[derive(sqlx::FromRow)]
struct SessionKey {
    started_at: i64,
    parent_harness: Option<i64>,
    parent_session_id: Option<String>,
    title: Option<String>,
}

impl SessionKey {
    /// What ranks the session's claim on a shared model call.
    fn rank(&self) -> (i64, Option<i64>, Option<&str>) {
        (self.started_at, self.parent_harness, self.parent_session_id.as_deref())
    }
}

/// A session holding rows of one model call, with the most usage any of them reported.
#[derive(sqlx::FromRow)]
struct Claimant {
    session_id: String,
    started_at: i64,
    parent_harness: Option<i64>,
    parent_session_id: Option<String>,
    usage_input: i64,
    usage_output: i64,
    usage_cache_read: i64,
    usage_cache_write: i64,
    usage_reasoning: i64,
}

impl Claimant {
    fn tokens(&self) -> Tokens {
        [
            self.usage_input,
            self.usage_output,
            self.usage_cache_read,
            self.usage_cache_write,
            self.usage_reasoning,
        ]
    }

    fn parent_of(&self, harness: i64) -> Option<String> {
        self.parent_session_id.clone().filter(|_| self.parent_harness == Some(harness))
    }
}

#[derive(sqlx::FromRow)]
struct CallRow {
    session_id: String,
    usage_input: i64,
    usage_output: i64,
    usage_cache_read: i64,
    usage_cache_write: i64,
    usage_reasoning: i64,
}

impl CallRow {
    fn tokens(&self) -> Tokens {
        [
            self.usage_input,
            self.usage_output,
            self.usage_cache_read,
            self.usage_cache_write,
            self.usage_reasoning,
        ]
    }
}

#[derive(sqlx::FromRow)]
struct SearchRow {
    #[sqlx(flatten)]
    session: SessionRow,
    match_content: String,
    match_content_z: Option<Vec<u8>>,
    match_index: i64,
    /// The session holding the best-matching message.
    match_harness: i64,
    match_session_id: String,
    match_title: Option<String>,
    score: f64,
}

#[derive(sqlx::FromRow)]
struct ReindexRow {
    rowid: i64,
    #[sqlx(flatten)]
    message: MessageRow,
    session_title: Option<String>,
}

/// Which migrations a sidecar has had, next to this build's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lineage {
    /// None: the file is new, or no daemon has migrated it yet.
    Missing,
    /// This build's migrations, up to the given version.
    Known(i64),
    /// This build's migrations and newer ones, up to the given version: a newer atuin's.
    Newer(i64),
    /// A migration at a version this build knows with other contents, a version this build skips,
    /// or one that failed: a development build's, whose numbering has since moved.
    Foreign,
}

impl AiSessionDatabase {
    /// Open the sidecar at `path` to write, creating and migrating it as needed.
    ///
    /// The sidecar is a projection of the record store, so one another build migrated (a
    /// migration at a version this build knows with other contents, one this build skips, or one
    /// that failed), which no migration of this build's can bring up to date, is deleted and
    /// created afresh: with no reproject watermarks, the daemon's next reprojection replays every
    /// record into it.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let path = path.as_ref();
        let mut sqlite = Sqlite::builder(path.as_os_str()).restrict_permissions().open().await?;
        let lineage = match Self::lineage(&sqlite).await {
            Ok(lineage) => lineage,
            Err(err) => {
                sqlite.close().await;
                return Err(err);
            }
        };
        if lineage == Lineage::Foreign {
            warn!(
                ?path,
                "the ai-session sidecar was migrated by another build of atuin; rebuilding it \
                 from the record store"
            );
            // Every connection, the WAL compactor's too, closed before the files go: Windows
            // refuses to delete a file that is open.
            sqlite.close().await;
            Self::remove(path)?;
            sqlite = Sqlite::builder(path.as_os_str()).restrict_permissions().open().await?;
        }
        let db = Self::from_sqlite(sqlite);
        db.migrate().await?;
        db.reindex().await?;
        Ok(db)
    }

    /// Open the sidecar to read beside the daemon that writes it (WAL keeps the two apart): no
    /// migration or reindex, and no writes. Fails when the file is missing, and with
    /// [`DbError::Uninitialized`], [`DbError::OutdatedSchema`] or [`DbError::UnknownSchema`]
    /// unless it is at exactly the schema this build reads.
    pub async fn open_read_only(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let db = Sqlite::builder(path.as_ref().as_os_str()).read_only().open().await?;
        let db = Self::from_sqlite(db);
        if let Err(err) = db.check_schema().await {
            // Not left to a drop, which closes it only eventually: the daemon may be about to
            // delete a sidecar refused here (see `open`), which Windows refuses while it is open.
            db.db.close().await;
            return Err(err);
        }
        Ok(db)
    }

    pub async fn in_memory() -> Result<Self, DbError> {
        let db = Sqlite::builder_in_memory().open().await?;
        let db = Self::from_sqlite(db);
        db.migrate().await?;
        Ok(db)
    }

    fn from_sqlite(db: Sqlite) -> Self {
        Self {
            db,
            local_projection: Arc::default(),
            reprojection: Arc::default(),
        }
    }

    /// Serialize projecting this host's records: live capture holds it from its dedup check
    /// until the message it pushed to the record store is appended here, and a reprojection
    /// holds it while replaying this host's record series. Without it, a reprojection could
    /// append a record capture has just pushed, before capture does, and capture would report
    /// its own message as a duplicate.
    pub async fn lock_local_projection(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.local_projection.clone().lock_owned().await
    }

    /// Serialize reprojections in this process (startup recovery, the sync worker's, a rebuild):
    /// two at once would each see the other's watermark moves as invalidations and start over,
    /// until one gave up with the sidecar short of the store.
    pub async fn lock_reprojection(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.reprojection.clone().lock_owned().await
    }

    async fn check_schema(&self) -> Result<(), DbError> {
        let expected = SCHEMA_VERSION;
        match Self::lineage(&self.db).await? {
            Lineage::Missing => Err(DbError::Uninitialized { expected }),
            Lineage::Foreign => Err(DbError::ForeignSchema),
            Lineage::Newer(found) => Err(DbError::UnknownSchema { found, expected }),
            Lineage::Known(found) if found < expected => {
                Err(DbError::OutdatedSchema { found, expected })
            }
            Lineage::Known(_) => Ok(()),
        }
    }

    /// Which migrations the sidecar has had, next to this build's (by version and checksum).
    async fn lineage(sqlite: &Sqlite) -> Result<Lineage, DbError> {
        let pool = sqlite.pool();
        let tracked: Option<i64> = db::query_scalar(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
        )
        .fetch_optional(pool)
        .await?;
        if tracked.is_none() {
            return Ok(Lineage::Missing);
        }
        let applied: Vec<(i64, Vec<u8>, bool)> = db::query_as(
            "SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(pool)
        .await?;
        Ok(Self::classify(&applied))
    }

    /// [`Self::lineage`] of the `(version, checksum, success)` rows of `_sqlx_migrations`.
    fn classify(applied: &[(i64, Vec<u8>, bool)]) -> Lineage {
        let newest_known = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
        let mut newest = None;
        let mut newer = false;
        for (version, checksum, success) in applied {
            if !success {
                return Lineage::Foreign;
            }
            match MIGRATOR.iter().find(|m| m.version == *version) {
                Some(known) if *known.checksum == **checksum => {}
                Some(_) => return Lineage::Foreign,
                None if *version > newest_known => newer = true,
                None => return Lineage::Foreign,
            }
            newest = newest.max(Some(*version));
        }
        let Some(newest) = newest else {
            return Lineage::Missing;
        };
        // Every migration of this build's up to the newest applied must be applied too.
        let skipped = MIGRATOR
            .iter()
            .any(|m| m.version <= newest && !applied.iter().any(|(v, ..)| *v == m.version));
        if skipped {
            Lineage::Foreign
        } else if newer {
            Lineage::Newer(newest)
        } else {
            Lineage::Known(newest)
        }
    }

    /// Delete the sidecar at `path`, with its WAL and shared-memory files.
    fn remove(path: &Path) -> Result<(), DbError> {
        for suffix in ["", "-wal", "-shm"] {
            let mut file = path.as_os_str().to_owned();
            file.push(suffix);
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(DbError::Remove(err)),
            }
        }
        Ok(())
    }

    async fn migrate(&self) -> Result<(), DbError> {
        let pool = self.db.pool();
        db::migrate!(pool, "./src/ai_session/migrations").await?;
        self.group_migrated().await
    }

    /// Link and group the sessions stored before the `incremental_sidecar` migration, which
    /// leaves them without a root: in Rust rather than in the migration, so it links copies by
    /// the same [`linkable_turn`] rule and groups them with the same query ([`Self::regroup_all`])
    /// as rows arriving later are. Every session stored since has a root, so this runs until it
    /// has committed once, and then never again.
    async fn group_migrated(&self) -> Result<(), DbError> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let ungrouped: Option<i64> =
            db::query_scalar("SELECT 1 FROM sessions WHERE root_harness IS NULL LIMIT 1")
                .fetch_optional(&mut *tx)
                .await?;
        if ungrouped.is_none() {
            return Ok(());
        }
        // Each parentless session links to the lowest-ranked other parentless session sharing a
        // call, when that ranks below it (see the migration), as `relink` does one at a time.
        db::query(concat!(
            "UPDATE sessions SET copy_of_session_id = (SELECT t.session_id FROM messages m CROSS \
             JOIN messages o ON o.harness = m.harness AND o.turn_id = m.turn_id AND o.session <> \
             m.session CROSS JOIN sessions t ON t.id = o.session WHERE m.session = sessions.id \
             AND ",
            linkable_turn!(),
            " AND t.parent_session_id IS NULL AND (t.started_at, t.session_id) < \
             (sessions.started_at, sessions.session_id) ORDER BY t.started_at, t.session_id LIMIT \
             1) WHERE parent_session_id IS NULL"
        ))
        .execute(&mut *tx)
        .await?;
        Self::regroup_all(&mut tx).await?;
        tx.commit().await?;
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
        let title_change = msg.title_change.as_ref().map(serde_json::to_string).transpose()?;
        let [usage_input, usage_output, usage_cache_read, usage_cache_write, _] =
            Self::fold_usage(msg.usage.as_ref());
        // Unlike the others, a row's reasoning stays NULL when unreported: most harnesses never
        // break it out, and zero would claim the call did not reason.
        let usage_reasoning =
            msg.usage.and_then(|u| u.reasoning).map(|n| i64::try_from(n).unwrap_or(i64::MAX));
        let cwd = msg.cwd.as_ref().map(|p| p.to_string_lossy().into_owned());
        let host = msg.host.map(Self::host_repr);

        let before = Self::session_key(&mut tx, harness, session_id).await?;

        // Messages reference their session by id, so it has to exist first. The upsert below
        // folds this row in exactly as it would a fresh insert: same timestamps, nothing counted.
        // A new session starts as its own root; regroup() below places it.
        db::query(
            "INSERT INTO sessions (harness, session_id, started_at, updated_at, root_harness,
                root_session_id)
            VALUES (?1, ?2, ?3, ?3, ?1, ?2)
            ON CONFLICT(harness, session_id) DO NOTHING",
        )
        .bind(harness)
        .bind(session_id)
        .bind(timestamp)
        .execute(&mut *tx)
        .await?;
        let session: i64 =
            db::query_scalar("SELECT id FROM sessions WHERE harness = ? AND session_id = ?")
                .bind(harness)
                .bind(session_id)
                .fetch_one(&mut *tx)
                .await?;

        let inserted = db::query(
            "INSERT INTO messages (
                id, harness, session, source_id, parent_harness, parent_session_id,
                parent_source_id, timestamp, role, content, content_z, cwd, git_branch, model,
                usage_input, usage_output, usage_cache_read, usage_cache_write,
                usage_reasoning, stop_reason, usage_present, turn_id, title_change, host_id
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
        .bind(usage_reasoning)
        .bind(stop_reason_json)
        .bind(i64::from(msg.usage.is_some()))
        .bind(msg.turn_id.as_deref())
        .bind(title_change)
        .bind(host.as_deref())
        .execute(&mut *tx)
        .await?;

        if inserted.rows_affected() == 0 {
            // A row stored before hosts were tracked learns its host when the reproject replays
            // its record (see the `incremental_sidecar` migration).
            if let Some(host) = &host {
                Self::backfill_host(&mut tx, session, source_id, host).await?;
            }
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
        // (which carry messages only). The session upsert below takes the newest row's, so a
        // cleared title clears; the capture pipeline stamps every row with the ranked title.
        let title = msg.session_title.as_deref();
        let preview = Self::preview_text(msg);
        let last_reply = Self::reply_text(msg);

        // Usage is not folded in here: it is attributed per model call below. A structural row
        // (usage, title, session context, a tree node with nothing to show) is no message.
        let counted = i64::from(!msg.content.is_empty());
        db::query(
            "INSERT INTO sessions (
                harness, session_id, parent_harness, parent_session_id, cwd, git_branch, model,
                started_at, updated_at, message_count, title, title_source, preview, last_reply,
                last_reply_at, parent_kind, host_id
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(harness, session_id) DO UPDATE SET
                host_id = COALESCE(sessions.host_id, excluded.host_id),
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
        .execute(&mut *tx)
        .await?;

        // A session is on the host of its earliest row, whatever order its rows arrive in (a
        // replay takes one host's series at a time). A row at or before the earliest so far may
        // be the new earliest, so the session's host is worked out again.
        if before.as_ref().is_some_and(|b| timestamp <= b.started_at) {
            Self::refresh_session_host(&mut tx, session).await?;
        }

        // A session is placed in its group when it first appears and when it learns its parent
        // (the parent link only ever goes from absent to present).
        let gained_parent =
            before.as_ref().is_some_and(|b| b.parent_session_id.is_none() && msg.parent.is_some());
        if before.is_none() || gained_parent {
            Self::regroup(&mut tx, harness, session_id).await?;
        }
        // A copy names no parent, so it is linked to its original through the calls they share
        // (see the `incremental_sidecar` migration). A link that moves other than by being made regroups
        // everything.
        let mut relinked = false;
        if let Some(before) = &before {
            if gained_parent {
                relinked |= Self::unlink(&mut tx, harness, session_id).await?;
            }
            let started_at: i64 = db::query_scalar(
                "SELECT started_at FROM sessions WHERE harness = ? AND session_id = ?",
            )
            .bind(harness)
            .bind(session_id)
            .fetch_one(&mut *tx)
            .await?;
            if started_at != before.started_at {
                relinked |= Self::relink_sharers(&mut tx, harness, session_id).await?;
            }
        }
        if let Some(turn) = &msg.turn_id {
            relinked |= Self::link_copies(&mut tx, harness, session_id, turn).await?;
        }
        if relinked {
            Self::regroup_all(&mut tx).await?;
        }

        let mut recount = BTreeSet::new();
        if let Some(usage) = &msg.usage {
            match &msg.turn_id {
                Some(turn) => {
                    recount.insert(turn.clone());
                }
                // A row outside any model call counts on its own.
                None => {
                    let tokens = Self::fold_usage(Some(usage));
                    Self::add_usage(&mut tx, harness, session_id, tokens, 1).await?;
                }
            }
        }
        // Which session owns a call shared with others depends on each claimant's start and
        // ancestry: when either moves, every call this session claims is attributed afresh.
        let existed = before.is_some();
        let previous_title = match before {
            Some(before) => {
                let after = Self::session_key(&mut tx, harness, session_id).await?;
                if after.as_ref().map(SessionKey::rank) != Some(before.rank()) {
                    let turns: Vec<String> = db::query_scalar(
                        "SELECT DISTINCT turn_id FROM messages WHERE session = ? AND \
                         usage_present = 1 AND turn_id IS NOT NULL",
                    )
                    .bind(session)
                    .fetch_all(&mut *tx)
                    .await?;
                    recount.extend(turns);
                }
                before.title
            }
            None => None,
        };
        for turn in &recount {
            Self::attribute_call(&mut tx, harness, turn).await?;
        }

        // A changed (or cleared) title has to reach the rows indexed before it. messages_fts is
        // contentless, so a single-column UPDATE is not supported: rewrite each of the session's
        // index rows, re-deriving the body from the stored content. Gated on the session's title
        // actually changing, since every replayed line of a titled session carries it.
        let current_title: Option<String> =
            db::query_scalar("SELECT title FROM sessions WHERE harness = ? AND session_id = ?")
                .bind(harness)
                .bind(session_id)
                .fetch_one(&mut *tx)
                .await?;
        if existed && current_title != previous_title {
            let title = current_title.as_deref().unwrap_or("");
            type BodyRow =
                (i64, String, Option<Vec<u8>>, Option<String>, Option<String>, Option<String>);
            let rows: Vec<BodyRow> = db::query_as(
                "SELECT rowid, content, content_z, cwd, git_branch, model FROM messages WHERE \
                 session = ?",
            )
            .bind(session)
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
        Ok(Appended::New)
    }

    pub async fn contains_message(
        &self,
        session: &HarnessSession,
        source_id: &SourceId,
    ) -> Result<bool, DbError> {
        let found: Option<(i64,)> = db::query_as(
            "SELECT 1 FROM messages WHERE session = (SELECT id FROM sessions WHERE harness = ? \
             AND session_id = ?) AND source_id = ? LIMIT 1",
        )
        .bind(session.harness as i64)
        .bind(session.session.as_ref())
        .bind(source_id.as_ref())
        .fetch_optional(self.db.pool())
        .await?;

        Ok(found.is_some())
    }

    /// Every title the session's lines set or cleared, oldest first: replayed, they give each
    /// source's current title again.
    pub async fn title_changes(
        &self,
        session: &HarnessSession,
    ) -> Result<Vec<TitleChange>, DbError> {
        let rows: Vec<String> = db::query_scalar(
            "SELECT title_change FROM messages WHERE session = (SELECT id FROM sessions WHERE \
             harness = ? AND session_id = ?) AND title_change IS NOT NULL ORDER BY timestamp, \
             rowid",
        )
        .bind(session.harness as i64)
        .bind(session.session.as_ref())
        .fetch_all(self.db.pool())
        .await?;
        Ok(rows.iter().filter_map(|json| serde_json::from_str(json).ok()).collect())
    }

    pub async fn source_ids_with_prefix(
        &self,
        session: &HarnessSession,
        prefix: &str,
    ) -> Result<Vec<SourceId>, DbError> {
        let ids: Vec<String> = db::query_scalar(
            "SELECT source_id FROM messages WHERE session = (SELECT id FROM sessions WHERE \
             harness = ? AND session_id = ?) AND substr(source_id, 1, length(?)) = ?",
        )
        .bind(session.harness as i64)
        .bind(session.session.as_ref())
        .bind(prefix)
        .bind(prefix)
        .fetch_all(self.db.pool())
        .await?;
        Ok(ids.into_iter().map(SourceId::from).collect())
    }

    /// Where capture resumes a session, if it checkpointed one with a digest; one without a
    /// digest reads as `None`, so the session is read again from its start.
    pub async fn checkpoint(
        &self,
        session: &HarnessSession,
    ) -> Result<Option<Checkpoint>, DbError> {
        let row: Option<(i64, Option<i64>)> = db::query_as(
            "SELECT \"offset\", digest FROM checkpoints WHERE harness = ? AND session_id = ?",
        )
        .bind(session.harness as i64)
        .bind(session.session.as_ref())
        .fetch_optional(self.db.pool())
        .await?;

        Ok(row.and_then(|(at, digest)| {
            Some(Checkpoint {
                at: u64::try_from(at).unwrap_or(0),
                digest: digest?.cast_unsigned(),
            })
        }))
    }

    pub async fn set_checkpoint(
        &self,
        session: &HarnessSession,
        checkpoint: Checkpoint,
    ) -> Result<(), DbError> {
        db::query(
            "INSERT OR REPLACE INTO checkpoints (harness, session_id, \"offset\", digest) VALUES \
             (?, ?, ?, ?)",
        )
        .bind(session.harness as i64)
        .bind(session.session.as_ref())
        .bind(i64::try_from(checkpoint.at).unwrap_or(i64::MAX))
        .bind(checkpoint.digest.cast_signed())
        .execute(self.db.pool())
        .await?;

        Ok(())
    }

    /// The newest stored message of a session, by timestamp then record id.
    pub async fn last_message(&self, session: &HarnessSession) -> Result<Option<Message>, DbError> {
        let row: Option<MessageRow> = db::query_as(concat!(
            "SELECT ",
            message_columns!(),
            " FROM messages m WHERE m.session = (SELECT id FROM sessions WHERE harness = ? AND \
             session_id = ?) ORDER BY m.timestamp DESC, m.id DESC LIMIT 1"
        ))
        .bind(session.harness as i64)
        .bind(session.session.as_ref())
        .fetch_optional(self.db.pool())
        .await?;

        row.map(Self::message_from_row).transpose()
    }

    pub async fn get_session(&self, session: &HarnessSession) -> Result<Option<Session>, DbError> {
        let row: Option<SessionRow> = db::query_as(concat!(
            "SELECT ",
            session_columns!(),
            ", ",
            no_group_columns!(),
            " FROM sessions s WHERE s.harness = ? AND s.session_id = ?"
        ))
        .bind(session.harness as i64)
        .bind(session.session.as_ref())
        .fetch_optional(self.db.pool())
        .await?;

        row.map(Self::session_from_row).transpose()
    }

    /// Every session passing `filter`, newest first. With [`SessionFilter::roots_only`], the
    /// roots of the groups with a session passing it, newest activity in the group first.
    pub async fn list_sessions(&self, filter: &SessionFilter) -> Result<Vec<Session>, DbError> {
        self.recent_sessions(filter, 0).await
    }

    /// Every session whose id starts with `prefix`, across harnesses, newest first (to find a
    /// session by an abbreviated id). Wildcards mean nothing here: the prefix is a byte range,
    /// not a pattern.
    pub async fn sessions_with_id_prefix(&self, prefix: &str) -> Result<Vec<Session>, DbError> {
        // Text compares by its UTF-8 bytes, which order as the code points do: the ids starting
        // with `prefix` are those at or past it and before its successor.
        let upper = prefix_successor(prefix);
        let upper_clause = if upper.is_some() {
            " AND s.session_id < ?"
        } else {
            ""
        };
        let sql = format!(
            "SELECT {}, {} FROM sessions s WHERE s.session_id >= ?{upper_clause} ORDER BY \
             s.updated_at DESC, s.session_id",
            session_columns!(),
            no_group_columns!(),
        );
        let mut query = db::query_as::<_, SessionRow>(sqlx::AssertSqlSafe(sql)).bind(prefix);
        if let Some(upper) = upper {
            query = query.bind(upper);
        }
        let rows: Vec<SessionRow> = query.fetch_all(self.db.pool()).await?;
        rows.into_iter().map(Self::session_from_row).collect()
    }

    /// The sessions grouped under the root `root` (not the root itself), newest first.
    pub async fn children(&self, root: &HarnessSession) -> Result<Vec<Session>, DbError> {
        let rows: Vec<SessionRow> = db::query_as(concat!(
            "SELECT ",
            session_columns!(),
            ", ",
            no_group_columns!(),
            " FROM sessions s WHERE s.root_harness = ? AND s.root_session_id = ? AND NOT \
             (s.harness = ? AND s.session_id = ?) ORDER BY s.updated_at DESC, s.session_id"
        ))
        .bind(root.harness as i64)
        .bind(root.session.as_ref())
        .bind(root.harness as i64)
        .bind(root.session.as_ref())
        .fetch_all(self.db.pool())
        .await?;
        rows.into_iter().map(Self::session_from_row).collect()
    }

    /// [`Self::list_sessions`], at most `limit` of them (0 is unbounded).
    async fn recent_sessions(
        &self,
        filter: &SessionFilter,
        limit: u32,
    ) -> Result<Vec<Session>, DbError> {
        let (clause, binds) = Self::filter_clause(filter);
        let limit_clause = if limit == 0 {
            ""
        } else {
            " LIMIT ?"
        };
        let sql = if filter.roots_only {
            format!(
                "WITH hits AS (SELECT DISTINCT s.root_harness AS gh, s.root_session_id AS gs FROM \
                 sessions s WHERE 1 = 1{clause}) SELECT {}, {} FROM hits JOIN sessions s ON \
                 s.harness = hits.gh AND s.session_id = hits.gs ORDER BY group_updated_at DESC, \
                 s.session_id{limit_clause}",
                session_columns!(),
                group_columns!(),
            )
        } else {
            format!(
                "SELECT {}, {} FROM sessions s WHERE 1 = 1{clause} ORDER BY s.updated_at DESC, \
                 s.session_id{limit_clause}",
                session_columns!(),
                no_group_columns!(),
            )
        };

        let mut query = db::query_as::<_, SessionRow>(sqlx::AssertSqlSafe(sql));
        for bind in binds {
            query = bind.apply_as(query);
        }
        if limit != 0 {
            query = query.bind(i64::from(limit));
        }

        let rows: Vec<SessionRow> = query.fetch_all(self.db.pool()).await?;
        rows.into_iter().map(Self::session_from_row).collect()
    }

    /// The SQL (each part led by ` AND `) and values selecting the sessions `s` passing `filter`,
    /// ignoring [`SessionFilter::roots_only`].
    fn filter_clause(filter: &SessionFilter) -> (String, Vec<Bind>) {
        let mut sql = String::new();
        let mut binds = Vec::new();
        if let Some(host) = filter.host {
            sql.push_str(" AND s.host_id = ?");
            binds.push(Bind::Text(Self::host_repr(host)));
        }
        if let Some(workspace) = &filter.workspace {
            // At the path or under it, whichever separator the recording machine used (`\` on
            // Windows). A root path leaves "", under which every absolute path is. The session's
            // directory, not each message's: some harnesses (Codex) record the cwd only on
            // metadata rows, never on the prompts and replies a search matches.
            let prefix = Self::path_repr(workspace);
            sql.push_str(
                " AND (s.cwd = ? OR substr(s.cwd, 1, length(?) + 1) IN (? || '/', ? || '\\'))",
            );
            binds.extend(std::iter::repeat_n(Bind::Text(prefix), 4));
        }
        if let Some(directory) = &filter.directory {
            sql.push_str(" AND s.cwd = ?");
            binds.push(Bind::Text(Self::path_repr(directory)));
        }
        if let Some(branch) = &filter.branch {
            sql.push_str(" AND s.git_branch = ?");
            binds.push(Bind::Text(branch.clone()));
        }
        if let Some(harness) = filter.harness {
            sql.push_str(" AND s.harness = ?");
            binds.push(Bind::Int(harness as i64));
        }
        if let Some(model) = &filter.model {
            sql.push_str(" AND instr(lower(s.model), lower(?)) > 0");
            binds.push(Bind::Text(model.clone()));
        }
        if let Some(since) = filter.updated_since {
            sql.push_str(" AND s.updated_at >= ?");
            binds.push(Bind::Int(Self::millis(since)));
        }
        (sql, binds)
    }

    /// A path as stored in `cwd`, without a trailing separator (`/`, or a Windows `\`).
    fn path_repr(path: &Path) -> String {
        let path = path.to_string_lossy();
        path.trim_end_matches(['/', '\\']).to_owned()
    }

    pub fn messages(
        &self,
        session: &HarnessSession,
    ) -> impl Stream<Item = Result<Message, DbError>> + Send + 'static {
        let pool = self.db.pool().clone();
        let harness = session.harness as i64;
        let session_id = session.session.as_ref().to_owned();

        async_stream::try_stream! {
            let mut rows = db::query_as::<_, MessageRow>(concat!(
                "SELECT ",
                message_columns!(),
                " FROM messages m WHERE m.session = (SELECT id FROM sessions WHERE harness = ? \
                 AND session_id = ?) ORDER BY m.timestamp, m.id"
            ))
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

    /// Sessions matching `query` (its terms matching as `terms` says) and passing `filter`, most
    /// relevant first, at most `limit` (0 is unbounded).
    ///
    /// Relevance is bm25 at the session's best message, a title match weighing five times a body
    /// one, divided by `1 + age_in_days / 30` so that equally relevant sessions rank newest
    /// first. With [`SessionFilter::roots_only`], a match anywhere in a group is its root's. A
    /// query with no terms returns the sessions [`Self::list_sessions`] does, newest first.
    ///
    /// Each match carries its title and a snippet of the best message, with the query's matches
    /// highlighted, and where that message is in its own session. When that session is not the
    /// one returned (a root, whose group it is in), the match names it
    /// ([`SessionMatch::matched`]), with its title highlighted instead of the root's.
    pub fn search(
        &self,
        query: &str,
        terms: SearchTerms,
        filter: &SessionFilter,
        limit: u32,
    ) -> impl Stream<Item = Result<SessionMatch, DbError>> + Send + 'static + use<> {
        let this = self.clone();
        let pool = self.db.pool().clone();
        let query = query.to_owned();
        let filter = filter.clone();

        async_stream::try_stream! {
            let highlighter = TextHighlighter::default();
            let expr = match terms {
                SearchTerms::All => match_expression(&query),
                SearchTerms::Typed if QueryTerms::prefixes_last(&query) => {
                    prefix_match_expression(&query)
                }
                SearchTerms::Typed => match_expression(&query),
                SearchTerms::Any => match_any_expression(&query),
            };
            let Some(expr) = expr else {
                for session in this.recent_sessions(&filter, limit).await? {
                    let title = session.title.clone().unwrap_or_default();
                    let preview = session.preview.clone().unwrap_or_default();
                    yield SessionMatch {
                        title: highlighter.as_highlighted(highlighter.sanitize(&title).into_owned()),
                        preview: highlighter
                            .as_highlighted(highlighter.sanitize(&preview).into_owned()),
                        session,
                        message_index: 0,
                        matched: None,
                        score: 0.0,
                    };
                }
                return;
            };

            let (filter_clause, binds) = Self::filter_clause(&filter);
            let (group_harness, group_session) = if filter.roots_only {
                ("s.root_harness", "s.root_session_id")
            } else {
                ("s.harness", "s.session_id")
            };
            let (group_select, recency) = if filter.roots_only {
                (
                    group_columns!(),
                    "(SELECT max(c.updated_at) FROM sessions c WHERE c.root_harness = g.harness \
                     AND c.root_session_id = g.session_id)",
                )
            } else {
                (no_group_columns!(), "g.updated_at")
            };
            let limit_clause = if limit == 0 { "" } else { " LIMIT ?" };

            // messages_fts is contentless: it can rank (bm25) but cannot render highlight() or
            // snippet(), so the query returns the best message's stored content and the marking
            // happens in Rust below. `best` takes the rowid of each group's top-scoring message
            // (SQLite fills bare columns from the max() row).
            let sql = format!(
                "WITH ranked AS MATERIALIZED (\
                 SELECT messages_fts.rowid AS rowid, {group_harness} AS gh, {group_session} AS gs, \
                 -bm25(messages_fts, {TITLE_WEIGHT:?}, 1.0) AS score \
                 FROM messages_fts JOIN messages m ON m.rowid = messages_fts.rowid \
                 JOIN sessions s ON s.id = m.session \
                 WHERE messages_fts MATCH ?{filter_clause}), \
                 best AS (SELECT rowid, gh, gs, max(score) AS score FROM ranked GROUP BY gh, gs), \
                 scored AS (SELECT best.rowid AS rowid, best.gh AS gh, best.gs AS gs, \
                 best.score / (1.0 + max(0, ? - {recency}) / {DAY_MILLIS:?} / {RECENCY_DAYS:?}) \
                 AS score FROM best JOIN sessions g ON g.harness = best.gh AND g.session_id = \
                 best.gs ORDER BY score DESC, g.updated_at DESC, g.session_id{limit_clause}) \
                 SELECT {}, {group_select}, m.content AS match_content, \
                 m.content_z AS match_content_z, \
                 (SELECT count(*) FROM messages p WHERE p.session = m.session \
                 AND (p.timestamp < m.timestamp \
                 OR (p.timestamp = m.timestamp AND p.id < m.id))) AS match_index, \
                 ms.harness AS match_harness, ms.session_id AS match_session_id, \
                 ms.title AS match_title, scored.score AS score FROM scored \
                 JOIN messages m ON m.rowid = scored.rowid \
                 JOIN sessions ms ON ms.id = m.session \
                 JOIN sessions s ON s.harness = scored.gh AND s.session_id = scored.gs \
                 ORDER BY scored.score DESC, s.updated_at DESC, s.session_id",
                session_columns!(),
            );

            let mut stmt = db::query_as::<_, SearchRow>(sqlx::AssertSqlSafe(sql)).bind(expr);
            for bind in binds {
                stmt = bind.apply_as(stmt);
            }
            stmt = stmt.bind(Self::millis(OffsetDateTime::now_utc()));
            if limit != 0 {
                stmt = stmt.bind(i64::from(limit));
            }

            let terms = QueryTerms::parse(&query, terms);
            let mut rows = stmt.fetch(&pool);
            while let Some(row) = rows.try_next().await? {
                let session = Self::session_from_row(row.session)?;
                let title = session.title.clone().unwrap_or_default();
                // Index the metadata (cwd, branch, model) but leave it out of the preview: callers
                // show it separately, and appended it only reads as noise at the end of a snippet.
                let body = Self::body_from_parts(row.match_content, row.match_content_z, None, None, None)
                .unwrap_or_else(|err| {
                    warn!(?err, "failed to decode matched ai-session message; empty preview");
                    String::new()
                });
                let preview = Self::preview_snippet(&body, &terms, SNIPPET_TOKENS);

                // A match in a session grouped under the root returned: the root's title is
                // highlighted only when it matches the query itself, and the matched session's
                // title is given with what matched in it.
                let matched_handle = HarnessSession {
                    harness: Self::harness_from_repr(row.match_harness)?,
                    session: NativeSessionId::from(row.match_session_id),
                };
                let (title, matched) = if matched_handle == session.handle {
                    (terms.highlight(highlighter, &title), None)
                } else {
                    let title = if terms.matched_by(&title) {
                        terms.highlight(highlighter, &title)
                    } else {
                        highlighter.as_highlighted(highlighter.sanitize(&title).into_owned())
                    };
                    let matched_title = row.match_title.unwrap_or_default();
                    (title, Some(MatchedSession {
                        handle: matched_handle,
                        title: terms.highlight(highlighter, &matched_title),
                    }))
                };

                yield SessionMatch {
                    session,
                    title,
                    preview: terms.highlight(highlighter, &preview),
                    message_index: u64::try_from(row.match_index).unwrap_or(0),
                    matched,
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
                "SELECT m.rowid AS rowid, m.id, m.harness, s.session_id, m.source_id, \
                 m.parent_harness, m.parent_session_id, s.parent_kind, m.parent_source_id, \
                 m.timestamp, m.role, m.content, m.content_z, m.cwd, m.git_branch, m.model, \
                 m.usage_input, m.usage_output, m.usage_cache_read, m.usage_cache_write, \
                 m.usage_reasoning, m.stop_reason, m.usage_present, m.turn_id, m.title_change, \
                 m.host_id, s.title AS session_title FROM messages m JOIN sessions s ON s.id = \
                 m.session WHERE m.rowid > ? ORDER BY m.rowid LIMIT ?",
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

    /// The index of the first whitespace word where a query term matches the way the FTS index
    /// matched it (see [`QueryTerms`]).
    fn preview_hit(words: &[&str], terms: &QueryTerms) -> Option<usize> {
        // (word index, folded token) stream over the whole body.
        let (word_of, tokens): (Vec<usize>, Vec<String>) =
            words.iter().enumerate().flat_map(|(i, w)| fts_tokens(w).map(move |t| (i, t))).unzip();
        terms.matches(&tokens).map(|(at, _)| at).min().map(|at| word_of[at])
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
    fn preview_snippet(body: &str, terms: &QueryTerms, max_tokens: usize) -> String {
        const MAX_CHARS: usize = 400;
        const LEAD_CHARS: usize = 80;

        let words: Vec<&str> = body.split_whitespace().collect();
        if words.is_empty() || max_tokens == 0 {
            return String::new();
        }

        let hit = Self::preview_hit(&words, terms).unwrap_or(0);

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

    /// The text of an assistant message, clipped for storage, or `None` for any other message.
    fn reply_text(msg: &Message) -> Option<String> {
        if msg.role != Role::Assistant {
            return None;
        }
        let text = msg
            .content
            .iter()
            .filter_map(|content| match content {
                Content::Text(text) => Some(text.trim()),
                _ => None,
            })
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        (!text.is_empty()).then(|| Self::clip_summary(&text))
    }

    fn preview_text(msg: &Message) -> Option<String> {
        if msg.role != Role::User {
            return None;
        }

        msg.content.iter().find_map(|content| match content {
            Content::Text(text) => Some(Self::clip_summary(text)),
            _ => None,
        })
    }

    /// A session summary field (`preview`, `last_reply`) clipped for storage: every session
    /// listing carries them, and displays show a line of each, so a pasted multi-kilobyte prompt
    /// kept whole would only weigh down every list.
    fn clip_summary(text: &str) -> String {
        const MAX_CHARS: usize = 600;
        let head = text.truncate_chars(MAX_CHARS);
        if head.len() < text.len() {
            format!("{head}…")
        } else {
            text.to_owned()
        }
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
            Content::Text(text)
            | Content::Reasoning(text)
            | Content::Summary(text)
            | Content::Error(text) => {
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

    fn fold_usage(usage: Option<&Usage>) -> Tokens {
        let Some(usage) = usage else {
            return [0; 5];
        };

        [usage.input, usage.output, usage.cache_read, usage.cache_write, usage.reasoning]
            .map(|n| i64::try_from(n.unwrap_or(0)).unwrap_or(i64::MAX))
    }

    /// What decides a session's claim on a shared model call (see [`Self::attribute_call`]),
    /// plus its title, read before and after an append.
    async fn session_key(
        conn: &mut SqliteConnection,
        harness: i64,
        session_id: &str,
    ) -> Result<Option<SessionKey>, DbError> {
        Ok(db::query_as(
            "SELECT started_at, parent_harness, parent_session_id, title FROM sessions WHERE \
             harness = ? AND session_id = ?",
        )
        .bind(harness)
        .bind(session_id)
        .fetch_optional(conn)
        .await?)
    }

    /// Delete every session holding a row `host` captured, whole, for a reprojection that replays
    /// that host's records from scratch because its record series was rewritten or deleted: what
    /// the old series projected may be gone from the store. Model calls the sessions claimed are
    /// attributed afresh among the claimants left, and groups they headed are regrouped.
    ///
    /// That is the sessions `host` captured (their first row's host), and the sessions other
    /// hosts captured that `host` added rows to: rows the old series projected there may be gone
    /// too, and a session's counts, usage, times, title, preview and last reply cannot be worked
    /// out again from the rows left (titles are only on the records). So they go whole, and the
    /// other hosts with rows in them are replayed: their records are still in the store, so their
    /// watermarks are forgotten along with `host`'s, and the next reprojection restores their rows
    /// exactly, without `host`'s but for what its series still holds. (A row of unknown host, from
    /// before hosts were tracked, forgets every watermark.) All of this is one invalidation: it
    /// bumps the [`Generation`].
    ///
    /// Returns whether it forgot the watermarks of hosts other than `host`, which a reprojection
    /// in progress must replay again.
    pub async fn forget_host(&self, host: HostId) -> Result<bool, DbError> {
        let host = Self::host_repr(host);
        let tag = RecordTag::AiSession.as_str();
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;

        // The sessions going, as a JSON array of their row ids for `json_each`.
        let going: String = db::query_scalar(
            "SELECT json_group_array(id) FROM sessions WHERE host_id = ?1 OR id IN (SELECT \
             session FROM messages WHERE host_id = ?1)",
        )
        .bind(&host)
        .fetch_one(&mut *tx)
        .await?;

        let turns: Vec<(i64, String)> = db::query_as(
            "SELECT DISTINCT m.harness, m.turn_id FROM messages m WHERE m.session IN (SELECT \
             value FROM json_each(?)) AND m.turn_id IS NOT NULL AND m.usage_present = 1",
        )
        .bind(&going)
        .fetch_all(&mut *tx)
        .await?;
        // Every other host with a row in the sessions going: NULL for a row of unknown host.
        let contributors: Vec<Option<String>> = db::query_scalar(
            "SELECT DISTINCT m.host_id FROM messages m WHERE m.session IN (SELECT value FROM \
             json_each(?1)) AND m.host_id IS NOT ?2",
        )
        .bind(&going)
        .bind(&host)
        .fetch_all(&mut *tx)
        .await?;

        for sql in [
            "DELETE FROM messages_fts WHERE rowid IN (SELECT rowid FROM messages WHERE session IN \
             (SELECT value FROM json_each(?)))",
            "DELETE FROM messages WHERE session IN (SELECT value FROM json_each(?))",
            "DELETE FROM calls WHERE (harness, session_id) IN (SELECT harness, session_id FROM \
             sessions WHERE id IN (SELECT value FROM json_each(?)))",
            "DELETE FROM sessions WHERE id IN (SELECT value FROM json_each(?))",
        ] {
            db::query(sql).bind(&going).execute(&mut *tx).await?;
        }
        // Sessions left grouped under a root that went.
        let orphaned: i64 = db::query_scalar(
            "SELECT count(*) FROM sessions c WHERE NOT EXISTS (SELECT 1 FROM sessions r WHERE \
             r.harness = c.root_harness AND r.session_id = c.root_session_id)",
        )
        .fetch_one(&mut *tx)
        .await?;

        for (harness, turn) in &turns {
            Self::attribute_call(&mut tx, *harness, turn).await?;
        }
        // Copies of a session that went link to the lowest-ranked original left, if any.
        let stranded: Vec<(i64, String)> = db::query_as(
            "SELECT harness, session_id FROM sessions s WHERE copy_of_session_id IS NOT NULL AND \
             NOT EXISTS (SELECT 1 FROM sessions o WHERE o.harness = s.harness AND o.session_id = \
             s.copy_of_session_id)",
        )
        .fetch_all(&mut *tx)
        .await?;
        let mut relinked = false;
        for (harness, session_id) in &stranded {
            relinked |= Self::relink(&mut tx, *harness, session_id).await?;
        }
        if orphaned > 0 || relinked {
            Self::regroup_all(&mut tx).await?;
        }

        if contributors.iter().any(Option::is_none) {
            db::query("DELETE FROM reproject_watermark WHERE tag = ?")
                .bind(tag)
                .execute(&mut *tx)
                .await?;
        } else {
            for forgotten in contributors.iter().flatten().chain([&host]) {
                db::query("DELETE FROM reproject_watermark WHERE host = ? AND tag = ?")
                    .bind(forgotten)
                    .bind(tag)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        Self::bump_generation(&mut tx).await?;

        tx.commit().await?;
        Ok(!contributors.is_empty())
    }

    /// Group every session afresh under its top-most stored ancestor, following the parent else
    /// the copy link: however deep the chain, and with a cycle headed by its least member (by
    /// harness, then id), whichever member a walk up entered it by.
    async fn regroup_all(conn: &mut SqliteConnection) -> Result<(), DbError> {
        db::query(
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
        .execute(conn)
        .await?;
        Ok(())
    }

    /// For a row of `session` holding call `turn`, just stored: offer each other session holding
    /// the call as the other's original. Each keeps the lowest-ranked original it is offered (see
    /// the `incremental_sidecar` migration), so this only ever lowers a link, and while no session's start or
    /// parent moves, the links end up the same whatever order the rows arrive in.
    ///
    /// A session linked for the first time was a root, and is placed like one learning its
    /// parent. Returns whether an existing link moved instead, which needs
    /// [`Self::regroup_all`].
    async fn link_copies(
        conn: &mut SqliteConnection,
        harness: i64,
        session_id: &str,
        turn: &str,
    ) -> Result<bool, DbError> {
        let others: Vec<String> = db::query_scalar(concat!(
            "SELECT DISTINCT s.session_id FROM messages m JOIN sessions s ON s.id = m.session \
             WHERE m.harness = ? AND m.turn_id = ? AND s.session_id <> ? AND ",
            linkable_turn!()
        ))
        .bind(harness)
        .bind(turn)
        .bind(session_id)
        .fetch_all(&mut *conn)
        .await?;

        let mut moved = false;
        for other in &others {
            for (copy, original) in [(session_id, other.as_str()), (other.as_str(), session_id)] {
                let previous: Option<String> = db::query_scalar(
                    "SELECT copy_of_session_id FROM sessions WHERE harness = ? AND session_id = ?",
                )
                .bind(harness)
                .bind(copy)
                .fetch_one(&mut *conn)
                .await?;
                // Only a parentless session links, only to a parentless one ranked below it, and
                // only when that ranks below the original it has.
                let linked = db::query(
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
                .execute(&mut *conn)
                .await?;
                if linked.rows_affected() == 0 {
                    continue;
                }
                match previous {
                    None => Self::regroup(&mut *conn, harness, copy).await?,
                    Some(_) => moved = true,
                }
            }
        }
        Ok(moved)
    }

    /// Link `session` to its original afresh from every call it shares, as
    /// [`Self::group_migrated`] does. Returns whether its link changed.
    async fn relink(
        conn: &mut SqliteConnection,
        harness: i64,
        session_id: &str,
    ) -> Result<bool, DbError> {
        let (previous, parent): (Option<String>, Option<String>) = db::query_as(
            "SELECT copy_of_session_id, parent_session_id FROM sessions WHERE harness = ? AND \
             session_id = ?",
        )
        .bind(harness)
        .bind(session_id)
        .fetch_one(&mut *conn)
        .await?;
        let original: Option<String> = match parent {
            Some(_) => None,
            // CROSS JOIN keeps this order: the session's own rows, then who else holds each call.
            None => {
                db::query_scalar(concat!(
                    "SELECT t.session_id FROM sessions s CROSS JOIN messages m ON m.session = \
                     s.id CROSS JOIN messages o ON o.harness = m.harness AND o.turn_id = \
                     m.turn_id AND o.session <> m.session CROSS JOIN sessions t ON t.id = \
                     o.session WHERE s.harness = ? AND s.session_id = ? AND ",
                    linkable_turn!(),
                    " AND t.parent_session_id IS NULL AND (t.started_at, t.session_id) < \
                     (s.started_at, s.session_id) ORDER BY t.started_at, t.session_id LIMIT 1"
                ))
                .bind(harness)
                .bind(session_id)
                .fetch_optional(&mut *conn)
                .await?
            }
        };
        if original == previous {
            return Ok(false);
        }
        db::query(
            "UPDATE sessions SET copy_of_session_id = ? WHERE harness = ? AND session_id = ?",
        )
        .bind(original)
        .bind(harness)
        .bind(session_id)
        .execute(conn)
        .await?;
        Ok(true)
    }

    /// `session`'s start moved: it may now rank below sessions sharing its calls, or they below
    /// it, so it and each of them is linked afresh. Returns whether any link changed.
    async fn relink_sharers(
        conn: &mut SqliteConnection,
        harness: i64,
        session_id: &str,
    ) -> Result<bool, DbError> {
        let sharers: Vec<String> = db::query_scalar(concat!(
            "SELECT DISTINCT t.session_id FROM messages m CROSS JOIN messages o ON o.harness = \
             m.harness AND o.turn_id = m.turn_id AND o.session <> m.session CROSS JOIN sessions t \
             ON t.id = o.session WHERE m.session = (SELECT id FROM sessions WHERE harness = ? AND \
             session_id = ?) AND ",
            linkable_turn!()
        ))
        .bind(harness)
        .bind(session_id)
        .fetch_all(&mut *conn)
        .await?;
        if sharers.is_empty() {
            return Ok(false);
        }

        let mut changed = Self::relink(&mut *conn, harness, session_id).await?;
        for sharer in &sharers {
            changed |= Self::relink(&mut *conn, harness, sharer).await?;
        }
        Ok(changed)
    }

    /// `session` learned its parent: it groups by that now, and can no longer be anyone's
    /// original, so its own link goes and the sessions linked to it are linked afresh. Returns
    /// whether any link changed.
    async fn unlink(
        conn: &mut SqliteConnection,
        harness: i64,
        session_id: &str,
    ) -> Result<bool, DbError> {
        let mut changed = Self::relink(&mut *conn, harness, session_id).await?;
        let copies: Vec<String> = db::query_scalar(
            "SELECT session_id FROM sessions WHERE harness = ? AND copy_of_session_id = ?",
        )
        .bind(harness)
        .bind(session_id)
        .fetch_all(&mut *conn)
        .await?;
        for copy in &copies {
            changed |= Self::relink(&mut *conn, harness, copy).await?;
        }
        Ok(changed)
    }

    /// Record `host` on a stored row that has none, and work its session's host out again.
    async fn backfill_host(
        conn: &mut SqliteConnection,
        session: i64,
        source_id: &str,
        host: &str,
    ) -> Result<(), DbError> {
        let filled = db::query(
            "UPDATE messages SET host_id = ? WHERE session = ? AND source_id = ? AND host_id IS \
             NULL",
        )
        .bind(host)
        .bind(session)
        .bind(source_id)
        .execute(&mut *conn)
        .await?
        .rows_affected()
            > 0;
        if filled {
            Self::refresh_session_host(conn, session).await?;
        }
        Ok(())
    }

    /// Set `session`'s host to its earliest row's: the one with the lowest timestamp, then the
    /// lowest record id, among the rows whose host is known. A session with no such row keeps
    /// whatever it has.
    async fn refresh_session_host(
        conn: &mut SqliteConnection,
        session: i64,
    ) -> Result<(), DbError> {
        db::query(
            "UPDATE sessions SET host_id = COALESCE((SELECT host_id FROM messages WHERE session = \
             ?1 AND host_id IS NOT NULL ORDER BY timestamp, id LIMIT 1), host_id) WHERE id = ?1",
        )
        .bind(session)
        .execute(conn)
        .await?;
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
    async fn regroup(
        conn: &mut SqliteConnection,
        harness: i64,
        session_id: &str,
    ) -> Result<(), DbError> {
        let (parent_harness, parent_session_id, copy_of): (
            Option<i64>,
            Option<String>,
            Option<String>,
        ) = db::query_as(
            "SELECT parent_harness, parent_session_id, copy_of_session_id FROM sessions WHERE \
             harness = ? AND session_id = ?",
        )
        .bind(harness)
        .bind(session_id)
        .fetch_one(&mut *conn)
        .await?;
        // A copy, which names no parent, groups under its original.
        let (parent_harness, parent_session_id) = match (parent_session_id, copy_of) {
            (None, Some(original)) => (Some(harness), Some(original)),
            (parent, _) => (parent_harness, parent),
        };

        let parent_root: Option<(i64, String)> = match (parent_harness, parent_session_id) {
            (Some(parent_harness), Some(parent_session_id)) => {
                db::query_as(
                    "SELECT root_harness, root_session_id FROM sessions WHERE harness = ? AND \
                     session_id = ? AND root_harness IS NOT NULL",
                )
                .bind(parent_harness)
                .bind(parent_session_id)
                .fetch_optional(&mut *conn)
                .await?
            }
            _ => None,
        };
        // The link closes a cycle when the parent is grouped under this very session, or the
        // root it is grouped under was waiting for this session as its own parent. A cycle has no
        // top: it is headed by its least member whichever order its sessions arrived in, which
        // `regroup_all` works out.
        if let Some((root_harness, root_session_id)) = &parent_root {
            let closes_cycle = (*root_harness, root_session_id.as_str()) == (harness, session_id)
                || db::query_scalar::<_, i64>(
                    "SELECT 1 FROM sessions WHERE harness = ?1 AND session_id = ?2 AND \
                     ((parent_harness = ?3 AND parent_session_id = ?4) OR (parent_session_id IS \
                     NULL AND harness = ?3 AND copy_of_session_id = ?4))",
                )
                .bind(*root_harness)
                .bind(root_session_id)
                .bind(harness)
                .bind(session_id)
                .fetch_optional(&mut *conn)
                .await?
                .is_some();
            if closes_cycle {
                return Self::regroup_all(conn).await;
            }
        }
        // A parent that is not stored leaves the session a root.
        let (root_harness, root_session_id) =
            parent_root.unwrap_or_else(|| (harness, session_id.to_owned()));

        if (root_harness, root_session_id.as_str()) != (harness, session_id) {
            db::query(
                "UPDATE sessions SET root_harness = ?, root_session_id = ? WHERE root_harness = ? \
                 AND root_session_id = ?",
            )
            .bind(root_harness)
            .bind(&root_session_id)
            .bind(harness)
            .bind(session_id)
            .execute(&mut *conn)
            .await?;
        }

        // Sessions stored before their parent (this one) arrived, still roots of their own.
        db::query(
            "UPDATE sessions SET root_harness = ?1, root_session_id = ?2 WHERE (root_harness, \
             root_session_id) IN (SELECT harness, session_id FROM sessions WHERE parent_harness = \
             ?3 AND parent_session_id = ?4 AND root_harness = harness AND root_session_id = \
             session_id AND NOT (harness = ?1 AND session_id = ?2))",
        )
        .bind(root_harness)
        .bind(&root_session_id)
        .bind(harness)
        .bind(session_id)
        .execute(conn)
        .await?;
        Ok(())
    }

    fn host_repr(host: HostId) -> String {
        host.as_hyphenated().to_string()
    }

    fn host_from_repr(host: Option<String>) -> Option<HostId> {
        // A malformed id reads as unknown rather than failing the whole row.
        host.and_then(|h| uuid::Uuid::parse_str(&h).ok()).map(HostId)
    }

    /// Add `sign * tokens` to a session's usage totals.
    async fn add_usage(
        conn: &mut SqliteConnection,
        harness: i64,
        session_id: &str,
        tokens: Tokens,
        sign: i64,
    ) -> Result<(), DbError> {
        let [input, output, cache_read, cache_write, reasoning] =
            tokens.map(|n| n.saturating_mul(sign));
        db::query(
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
        .execute(conn)
        .await?;
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
    async fn attribute_call(
        conn: &mut SqliteConnection,
        harness: i64,
        turn: &str,
    ) -> Result<(), DbError> {
        let claimants: Vec<Claimant> = db::query_as(
            "SELECT s.session_id AS session_id, s.started_at AS started_at, s.parent_harness AS \
             parent_harness, s.parent_session_id AS parent_session_id, MAX(m.usage_input) AS \
             usage_input, MAX(m.usage_output) AS usage_output, MAX(m.usage_cache_read) AS \
             usage_cache_read, MAX(m.usage_cache_write) AS usage_cache_write, \
             COALESCE(MAX(m.usage_reasoning), 0) AS usage_reasoning FROM messages m JOIN sessions \
             s ON s.id = m.session WHERE m.harness = ? AND m.turn_id = ? AND m.usage_present = 1 \
             GROUP BY m.session",
        )
        .bind(harness)
        .bind(turn)
        .fetch_all(&mut *conn)
        .await?;

        let tokens = claimants.iter().fold([0; 5], |acc: Tokens, c| {
            let row = c.tokens();
            std::array::from_fn(|i| acc[i].max(row[i]))
        });
        let ids: HashSet<&str> = claimants.iter().map(|c| c.session_id.as_str()).collect();
        let mut eligible = Vec::with_capacity(claimants.len());
        for claimant in &claimants {
            if claimants.len() == 1
                || !Self::descends_from(&mut *conn, harness, claimant, &ids).await?
            {
                eligible.push(claimant);
            }
        }
        // Only a parent cycle leaves nobody; fall back to the plain ranking.
        if eligible.is_empty() {
            eligible.extend(&claimants);
        }
        let Some(owner) = eligible
            .into_iter()
            .min_by(|a, b| (a.started_at, &a.session_id).cmp(&(b.started_at, &b.session_id)))
        else {
            return Ok(());
        };

        let previous: Option<CallRow> = db::query_as(
            "SELECT session_id, usage_input, usage_output, usage_cache_read, usage_cache_write, \
             usage_reasoning FROM calls WHERE harness = ? AND turn_id = ?",
        )
        .bind(harness)
        .bind(turn)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(previous) = &previous {
            if previous.session_id == owner.session_id && previous.tokens() == tokens {
                return Ok(());
            }
            Self::add_usage(&mut *conn, harness, &previous.session_id, previous.tokens(), -1)
                .await?;
        }
        Self::add_usage(&mut *conn, harness, &owner.session_id, tokens, 1).await?;

        let [input, output, cache_read, cache_write, reasoning] = tokens;
        db::query(
            "INSERT OR REPLACE INTO calls (harness, turn_id, session_id, usage_input, \
             usage_output, usage_cache_read, usage_cache_write, usage_reasoning) VALUES (?, ?, ?, \
             ?, ?, ?, ?, ?)",
        )
        .bind(harness)
        .bind(turn)
        .bind(&owner.session_id)
        .bind(input)
        .bind(output)
        .bind(cache_read)
        .bind(cache_write)
        .bind(reasoning)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Whether another of `claimants` is an ancestor of `claimant`, following stored parent
    /// links within the harness.
    async fn descends_from(
        conn: &mut SqliteConnection,
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
            next = Self::session_key(&mut *conn, harness, &parent).await?.and_then(|key| {
                key.parent_session_id.filter(|_| key.parent_harness == Some(harness))
            });
        }
        Ok(false)
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
        let parent_kind =
            parent.as_ref().and(row.parent_kind).and_then(Self::parent_kind_from_repr);
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
            .parent_kind(parent_kind)
            .parent_source_id(row.parent_source_id.map(SourceId::from))
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
                reasoning: row.usage_reasoning.map(|n| u64::try_from(n).unwrap_or(0)),
            }))
            .stop_reason(stop_reason)
            .turn_id(row.turn_id)
            .title_change(
                row.title_change.as_deref().and_then(|json| serde_json::from_str(json).ok()),
            )
            .host(Self::host_from_repr(row.host_id))
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
                    Some(atuin_common::harnesstools::session::model::reasoning_label(
                        tokens.or(message.usage.and_then(|u| u.reasoning)),
                    ))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Trailing newline: chunks are concatenated verbatim by consumers, so the separator has
        // to live in the chunk or every message would run together on one line.
        format!("{role}: {body}\n")
    }

    const fn parent_kind_repr(kind: ParentKind) -> i64 {
        match kind {
            ParentKind::Subagent => 0,
            ParentKind::Fork => 1,
            ParentKind::Continuation => 2,
        }
    }

    const fn parent_kind_from_repr(n: i64) -> Option<ParentKind> {
        Some(match n {
            0 => ParentKind::Subagent,
            1 => ParentKind::Fork,
            2 => ParentKind::Continuation,
            _ => return None,
        })
    }

    const fn title_source_repr(source: TitleSource) -> i64 {
        match source {
            TitleSource::Summary => 0,
            TitleSource::Generated => 1,
            TitleSource::Named => 2,
            TitleSource::Agent => 3,
        }
    }

    const fn title_source_from_repr(n: i64) -> Option<TitleSource> {
        Some(match n {
            0 => TitleSource::Summary,
            1 => TitleSource::Generated,
            2 => TitleSource::Named,
            3 => TitleSource::Agent,
            _ => return None,
        })
    }

    fn session_from_row(row: SessionRow) -> Result<Session, DbError> {
        let harness = Self::harness_from_repr(row.harness)?;
        let parent = Self::optional_session(row.parent_harness, row.parent_session_id)?;
        let root = Self::optional_session(row.root_harness, row.root_session_id)?
            .filter(|root| root.harness != harness || root.session.as_ref() != row.session_id);
        let group_updated_at = row.group_updated_at.map(Self::time_from_millis).transpose()?;
        let copy_of = row.copy_of_session_id.map(|session| HarnessSession {
            harness,
            session: NativeSessionId::from(session),
        });

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
                reasoning: Some(u64::try_from(row.usage_reasoning).unwrap_or(0)),
            })
            .title(row.title)
            .title_source(row.title_source.and_then(Self::title_source_from_repr))
            .preview(row.preview)
            .last_reply(row.last_reply)
            .parent_kind(row.parent_kind.and_then(Self::parent_kind_from_repr))
            .host(Self::host_from_repr(row.host_id))
            .root(root)
            .copy_of(copy_of)
            .child_count(u64::try_from(row.child_count).unwrap_or(0))
            .group_updated_at(group_updated_at)
            .build())
    }
}

/// A value bound into a query built at runtime.
#[derive(Clone)]
enum Bind {
    Text(String),
    Int(i64),
}

type QueryAs<'q, O> =
    sqlx::query::QueryAs<'q, sqlx::Sqlite, O, <sqlx::Sqlite as sqlx::Database>::Arguments>;

impl Bind {
    fn apply_as<O>(self, query: QueryAs<'_, O>) -> QueryAs<'_, O> {
        match self {
            Self::Text(text) => query.bind(text),
            Self::Int(n) => query.bind(n),
        }
    }
}

/// The smallest string greater than every string starting with `prefix`, or `None` when there is
/// none (an empty prefix, or one of only `char::MAX`).
fn prefix_successor(prefix: &str) -> Option<String> {
    let mut chars: Vec<char> = prefix.chars().collect();
    while let Some(last) = chars.pop() {
        // The next code point, skipping the surrogates, which no string holds.
        let next = (u32::from(last) + 1..=u32::from(char::MAX)).find_map(char::from_u32);
        if let Some(next) = next {
            chars.push(next);
            return Some(chars.into_iter().collect());
        }
    }
    None
}

/// Fold text the way the index's `unicode61` tokenizer does — lowercase with combining marks
/// stripped — so preview placement and highlights agree with what FTS5 actually matched (e.g. a
/// query for `cafe` matches a stored `café`).
fn fts_fold(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization as _;
    use unicode_normalization::char::is_combining_mark;
    text.nfd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
}

/// The folded `unicode61`-style tokens (alphanumeric runs) of `text`.
fn fts_tokens(text: &str) -> impl Iterator<Item = String> + use<> {
    fts_fold(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>()
        .into_iter()
}

/// The byte ranges of `text`'s tokens, each with its folded form.
fn fts_token_spans(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
    use unicode_normalization::char::is_combining_mark;
    let is_token = |c: char| c.is_alphanumeric() || is_combining_mark(c);
    let mut spans = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        match (start, is_token(c)) {
            (None, true) => start = Some(i),
            (Some(from), false) => {
                start = None;
                let folded = fts_fold(&text[from..i]);
                if !folded.is_empty() {
                    spans.push((from..i, folded));
                }
            }
            _ => {}
        }
    }
    spans
}

/// A search query's terms as the index matches them (see [`SearchTerms`]): each
/// whitespace-separated term is a phrase of folded tokens that must appear consecutively (so
/// `app` matches the token `app`, not the word `apple`, and `foo-bar` matches `foo bar` across
/// words). A phrase's last token also matches as a prefix: every phrase's for
/// [`SearchTerms::Any`], and the last term's for [`SearchTerms::Typed`] when
/// [`Self::prefixes_last`].
struct QueryTerms {
    /// Each phrase, and whether its last token is a prefix.
    phrases: Vec<(Vec<String>, bool)>,
    /// Whether a row matches holding any phrase ([`SearchTerms::Any`]), rather than every one.
    any: bool,
}

impl QueryTerms {
    fn parse(query: &str, mode: SearchTerms) -> Self {
        let terms: Vec<&str> = query.split_whitespace().collect();
        let prefix = |i: usize| match mode {
            SearchTerms::All => false,
            SearchTerms::Typed => i + 1 == terms.len() && Self::prefixes_last(query),
            SearchTerms::Any => true,
        };
        let phrases = terms
            .iter()
            .enumerate()
            .map(|(i, term)| (fts_tokens(term).collect::<Vec<_>>(), prefix(i)))
            .filter(|(phrase, _)| !phrase.is_empty())
            .collect();
        Self {
            phrases,
            any: mode == SearchTerms::Any,
        }
    }

    /// Whether the last term of `query` matches as a prefix: it is still being typed (no
    /// whitespace after it), and is at least [`MIN_PREFIX_CHARS`] long.
    fn prefixes_last(query: &str) -> bool {
        !query.ends_with(char::is_whitespace)
            && query
                .split_whitespace()
                .last()
                .is_some_and(|t| t.chars().count() >= MIN_PREFIX_CHARS)
    }

    /// Where a phrase matches in `tokens`: its first token's index and its length.
    fn matches<'a>(&'a self, tokens: &'a [String]) -> impl Iterator<Item = (usize, usize)> + 'a {
        self.phrases
            .iter()
            .flat_map(move |(phrase, prefix)| Self::phrase_matches(phrase, *prefix, tokens))
    }

    /// Where `phrase` matches in `tokens`: its first token's index and its length.
    fn phrase_matches<'a>(
        phrase: &'a [String],
        prefix: bool,
        tokens: &'a [String],
    ) -> impl Iterator<Item = (usize, usize)> + 'a {
        (0..=tokens.len().saturating_sub(phrase.len()))
            .filter(move |&at| {
                tokens.len() >= phrase.len()
                    && phrase.iter().zip(&tokens[at..]).enumerate().all(|(i, (p, t))| {
                        if prefix && i + 1 == phrase.len() {
                            t.starts_with(p.as_str())
                        } else {
                            p == t
                        }
                    })
            })
            .map(move |at| (at, phrase.len()))
    }

    /// Whether `text` alone holds what a row must to match the query, as the index tells it:
    /// every phrase, or with [`SearchTerms::Any`] any of them.
    fn matched_by(&self, text: &str) -> bool {
        let tokens: Vec<String> = fts_tokens(text).collect();
        let mut found = self.phrases.iter().map(|(phrase, prefix)| {
            Self::phrase_matches(phrase, *prefix, &tokens).next().is_some()
        });
        if self.any {
            found.any(|f| f)
        } else {
            !self.phrases.is_empty() && found.all(|f| f)
        }
    }

    /// `text` with every token the query matches marked by `highlighter`.
    fn highlight(&self, highlighter: TextHighlighter, text: &str) -> HighlightedString {
        let text = highlighter.sanitize(text);
        let spans = fts_token_spans(&text);
        let tokens: Vec<String> = spans.iter().map(|(_, t)| t.clone()).collect();
        let mut marked = vec![false; tokens.len()];
        for (at, len) in self.matches(&tokens) {
            marked[at..at + len].fill(true);
        }

        let [open, close] = highlighter.markers();
        let mut out = String::with_capacity(text.len());
        let mut copied = 0;
        for ((range, _), _) in spans.iter().zip(&marked).filter(|(_, m)| **m) {
            out.push_str(&text[copied..range.start]);
            out.push(open);
            out.push_str(&text[range.clone()]);
            out.push(close);
            copied = range.end;
        }
        out.push_str(&text[copied..]);
        highlighter.as_highlighted(out)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use atuin_common::db;
    use atuin_common::db::sqlite::Sqlite;
    use atuin_common::db::sqlite::fts::TextHighlighter;
    use atuin_common::harnesstools::session::{
        Checkpoint, Content, ParentKind, Role, ToolCallId, ToolResult, ToolUse, Usage,
    };
    use atuin_domain::record::{HostId, RecordId};
    use futures::TryStreamExt;
    use rstest::{fixture, rstest};
    use time::OffsetDateTime;

    use super::{
        AiSessionDatabase, Appended, COMPRESS_THRESHOLD, DbError, Lineage, MIGRATOR, QueryTerms,
        SCHEMA_VERSION, TitleSource, prefix_successor,
    };
    use crate::ai_session::{
        HarnessKind, HarnessSession, Message, NativeSessionId, SearchTerms, Session, SessionFilter,
        SessionMatch, SessionRelation, SourceId,
    };

    fn harness_filter(harness: HarnessKind) -> SessionFilter {
        SessionFilter {
            harness: Some(harness),
            ..SessionFilter::default()
        }
    }

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

    /// Upgrading keeps each message's rowid, which its contentless messages_fts row is keyed by.
    #[rstest]
    #[tokio::test]
    async fn interning_sessions_keeps_the_search_index_aligned() {
        let sqlite = atuin_common::db::sqlite::Sqlite::builder_in_memory().open().await.unwrap();
        let pool = sqlite.pool();
        sqlx::raw_sql(include_str!("migrations/0001_init.sql")).execute(pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO sessions (harness, session_id, started_at, updated_at) VALUES (1, 's', \
             0, 0);
            INSERT INTO messages (rowid, id, harness, session_id, source_id, timestamp, role, \
             content)
                VALUES (7, x'01', 1, 's', 'a', 0, '\"User\"', '[{\"Text\":\"hello\"}]');
            INSERT INTO messages_fts (rowid, title, body) VALUES (7, '', 'hello');",
        )
        .execute(pool)
        .await
        .unwrap();

        sqlx::raw_sql(include_str!("migrations/0002_intern_sessions.sql"))
            .execute(pool)
            .await
            .unwrap();
        for migration in [
            include_str!("migrations/0003_last_reply.sql"),
            include_str!("migrations/0004_parent_kind.sql"),
            include_str!("migrations/0005_sessions_updated_at.sql"),
            include_str!("migrations/0006_incremental_sidecar.sql"),
        ] {
            sqlx::raw_sql(migration).execute(pool).await.unwrap();
        }

        let db = AiSessionDatabase::from_sqlite(sqlite);
        let hits: Vec<SessionMatch> = db
            .search("hello", SearchTerms::Typed, &SessionFilter::default(), 0)
            .try_collect()
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.handle.session.as_ref(), "s");
    }

    /// Rows that carry only usage, a title or session context are stored, but are no messages.
    #[rstest]
    #[tokio::test]
    async fn structural_rows_are_not_counted_as_messages() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let spoken = sample_message();
        let mut usage_only = sample_message();
        usage_only.source_id = SourceId::from("usage".to_owned());
        usage_only.content = Vec::new();
        usage_only.usage = Some(Usage {
            input: Some(1),
            ..Usage::default()
        });
        let mut titled = sample_message();
        titled.source_id = SourceId::from("title".to_owned());
        titled.content = Vec::new();
        titled.session_title = Some("a title".to_owned());
        for m in [&spoken, &usage_only, &titled] {
            assert_eq!(db.append(m).await.unwrap(), Appended::New);
        }

        let s = db.get_session(&spoken.session).await.unwrap().unwrap();
        assert_eq!(s.message_count, 1);
        assert_eq!(s.usage.input, Some(1));
        assert_eq!(s.title.as_deref(), Some("a title"));
    }

    #[rstest]
    #[tokio::test]
    async fn checkpoint_round_trips_and_the_latest_write_wins() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        assert_eq!(db.checkpoint(&session).await.unwrap(), None);
        db.set_checkpoint(&session, Checkpoint { at: 42, digest: 1 }).await.unwrap();
        // A digest with its top bit set survives the signed column.
        let latest = Checkpoint {
            at: 4096,
            digest: u64::MAX - 1,
        };
        db.set_checkpoint(&session, latest).await.unwrap();
        assert_eq!(db.checkpoint(&session).await.unwrap(), Some(latest));
    }

    #[rstest]
    #[tokio::test]
    async fn a_checkpoint_without_a_digest_reads_as_none() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        db::query("INSERT INTO checkpoints (harness, session_id, \"offset\") VALUES (?, ?, 42)")
            .bind(session.harness as i64)
            .bind(session.session.as_ref())
            .execute(db.db.pool())
            .await
            .unwrap();
        assert_eq!(db.checkpoint(&session).await.unwrap(), None);
    }

    #[rstest]
    #[tokio::test]
    async fn last_message_is_the_newest_by_timestamp() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        assert!(db.last_message(&session).await.unwrap().is_none());
        for index in [2, 0, 1] {
            db.append(&message_in(&session, index, "x")).await.unwrap();
        }
        let last = db.last_message(&session).await.unwrap().unwrap();
        assert_eq!(last.source_id, SourceId::from("source-2".to_owned()));
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

        let sessions = db.list_sessions(&SessionFilter::default()).await.unwrap();
        assert_eq!(sessions.len(), 3);
        assert!(sessions.windows(2).all(|w| w[0].updated_at >= w[1].updated_at));
    }

    #[rstest]
    #[case::all(0, &["session-2", "session-1", "session-0"])]
    #[case::inclusive(1, &["session-2", "session-1"])]
    #[case::none(3, &[])]
    #[tokio::test]
    async fn list_filters_by_activity_since(#[case] since: i64, #[case] want: &[&str]) {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for m in three_sessions_oldest_first() {
            db.append(&m).await.unwrap();
        }

        let since = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(since);
        let sessions = db
            .list_sessions(&SessionFilter {
                updated_since: Some(since),
                ..SessionFilter::default()
            })
            .await
            .unwrap();
        let ids: Vec<_> = sessions.iter().map(|s| s.handle.session.as_ref()).collect();
        assert_eq!(ids, want);
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
        // A rediscovered old session replays its historical messages; updated_at must reflect
        // the newest message, not the wall-clock capture instant.
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
                reasoning: None,
            }))
            .build();
        db.append(&with_usage).await.unwrap();

        let got: Vec<_> = db.messages(&session).try_collect().await.unwrap();
        assert_eq!(got[0].usage, None, "absent usage must not become Some(zeros)");
        assert!(got[1].usage.is_some(), "reported usage must survive the round trip");
    }

    /// A row of model call `turn` (or none) in `session`, reporting `output` tokens.
    fn call_row(
        session: &str,
        parent: Option<&str>,
        source: &str,
        seconds: i64,
        turn: Option<&str>,
        output: u64,
    ) -> Message {
        let mut m = message_in(&handle(HarnessKind::Pi, session), seconds, "x");
        m.source_id = SourceId::from(source.to_owned());
        m.parent = parent.map(|p| handle(HarnessKind::Pi, p));
        m.turn_id = turn.map(str::to_owned);
        m.usage = Some(Usage {
            input: Some(1),
            output: Some(output),
            cache_read: Some(0),
            cache_write: Some(0),
            reasoning: None,
        });
        m
    }

    async fn output_of(db: &AiSessionDatabase, session: &str) -> u64 {
        let row = db.get_session(&handle(HarnessKind::Pi, session)).await.unwrap().unwrap();
        row.usage.output.unwrap()
    }

    /// Rows of one call count once, at the most any of them reported; rows outside any call
    /// count individually. Stored rows keep what they reported.
    #[rstest]
    #[tokio::test]
    async fn a_call_counts_once_at_its_largest_usage() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for m in [
            call_row("s", None, "a1", 0, Some("A"), 25),
            call_row("s", None, "b1", 1, Some("B"), 5),
            call_row("s", None, "a2", 2, Some("A"), 250),
            call_row("s", None, "a3", 3, Some("A"), 250),
            call_row("s", None, "x1", 4, None, 7),
            call_row("s", None, "x2", 5, None, 7),
        ] {
            db.append(&m).await.unwrap();
        }
        assert_eq!(output_of(&db, "s").await, 250 + 5 + 7 + 7);
        let rows: Vec<_> = db.messages(&handle(HarnessKind::Pi, "s")).try_collect().await.unwrap();
        let reported: Vec<_> = rows.iter().map(|m| m.usage.unwrap().output.unwrap()).collect();
        assert_eq!(reported, vec![25, 5, 250, 250, 7, 7]);
    }

    /// The rows of a parent, a fork copying its call (with the parent's own timestamps, so
    /// start times tie), a fork of the fork, and an unrelated session that happens to share the
    /// call -- in every append order, as sync or import may deliver them.
    fn shared_call_rows() -> Vec<Message> {
        vec![
            call_row("parent", None, "p1", 10, Some("A"), 10),
            call_row("parent", None, "p2", 11, Some("A"), 40),
            call_row("fork", Some("parent"), "p1", 10, Some("A"), 10),
            call_row("fork", Some("parent"), "f1", 20, Some("B"), 3),
            call_row("forkfork", Some("fork"), "p2", 11, Some("A"), 40),
            call_row("forkfork", Some("fork"), "f1", 20, Some("B"), 3),
            call_row("forkfork", Some("fork"), "g1", 30, Some("C"), 1),
        ]
    }

    #[rstest]
    fn shared_calls_are_attributed_the_same_in_any_order() {
        let shuffled = proptest::strategy::Strategy::prop_shuffle(proptest::strategy::Just(
            shared_call_rows(),
        ));
        proptest::proptest!(|(rows in shuffled)| {
            let totals = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let db = AiSessionDatabase::in_memory().await.unwrap();
                    for m in &rows {
                        db.append(m).await.unwrap();
                    }
                    [
                        output_of(&db, "parent").await,
                        output_of(&db, "fork").await,
                        output_of(&db, "forkfork").await,
                    ]
                });
            // The fork starts at the parent's copied timestamp and "fork" < "parent", so only
            // ancestry keeps call A with the parent.
            proptest::prop_assert_eq!(totals, [40, 3, 1]);
        });
    }

    /// Without ancestry, the earliest-started claimant owns the call, then the smallest id.
    #[rstest]
    #[case::earlier_start_wins(0, 10, 0)]
    #[case::tie_goes_to_the_smaller_id(9, 0, 10)]
    #[tokio::test]
    async fn unrelated_claimants_rank_by_start_then_id(
        #[case] a_start: i64,
        #[case] a_expected: u64,
        #[case] b_expected: u64,
    ) {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        db.append(&call_row("c", None, "c", 5, Some("A"), 10)).await.unwrap();
        db.append(&call_row("b", None, "b", 5, Some("A"), 10)).await.unwrap();
        db.append(&call_row("a", None, "a", a_start + 10, Some("A"), 10)).await.unwrap();
        // A row outside the call moves "a"'s start earlier, which re-ranks its claim.
        db.append(&call_row("a", None, "a0", a_start, None, 0)).await.unwrap();
        assert_eq!(output_of(&db, "a").await, a_expected);
        assert_eq!(output_of(&db, "b").await, b_expected);
        assert_eq!(output_of(&db, "c").await, 0);
    }

    #[rstest]
    #[tokio::test]
    async fn source_ids_with_prefix_lists_only_matching_rows() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        for (i, source) in ["syn-1", "syn-1-1", "native"].into_iter().enumerate() {
            let mut m = message_in(&session, i64::try_from(i).unwrap(), "x");
            m.source_id = SourceId::from(source.to_owned());
            db.append(&m).await.unwrap();
        }
        let mut ids = db.source_ids_with_prefix(&session, "syn-").await.unwrap();
        ids.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
        assert_eq!(ids, vec![
            SourceId::from("syn-1".to_owned()),
            SourceId::from("syn-1-1".to_owned())
        ]);
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

    async fn search_with(
        db: &AiSessionDatabase,
        query: &str,
        filter: &SessionFilter,
        limit: u32,
    ) -> Vec<SessionMatch> {
        db.search(query, SearchTerms::Typed, filter, limit).try_collect().await.unwrap()
    }

    /// A search as the MCP tools make one: every term as a whole word, or with `any_term` any
    /// term as a prefix, in the sessions at or under `cwd` when given.
    fn search_in(
        db: &AiSessionDatabase,
        query: &str,
        cwd: Option<&str>,
        any_term: bool,
    ) -> impl futures::Stream<Item = Result<SessionMatch, DbError>> + use<> {
        let terms = if any_term {
            SearchTerms::Any
        } else {
            SearchTerms::All
        };
        let filter = SessionFilter {
            workspace: cwd.map(std::path::PathBuf::from),
            ..SessionFilter::default()
        };
        db.search(query, terms, &filter, 0)
    }

    async fn search(db: &AiSessionDatabase, query: &str) -> Vec<SessionMatch> {
        db.search(query, SearchTerms::Typed, &SessionFilter::default(), 0)
            .try_collect()
            .await
            .unwrap()
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
        let mut message = message_with(&session, 0, Role::Assistant, vec![
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
        ]);
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

        let all: Vec<_> = db
            .search("shared", SearchTerms::Typed, &SessionFilter::default(), 0)
            .try_collect()
            .await
            .unwrap();
        assert_eq!(all.len(), 2);

        let only_codex: Vec<_> = db
            .search("shared", SearchTerms::Typed, &harness_filter(HarnessKind::Codex), 0)
            .try_collect()
            .await
            .unwrap();
        assert_eq!(only_codex.len(), 1);
        assert_eq!(only_codex[0].session.handle, codex);
    }

    fn reply_in(session: &HarnessSession, index: i64, text: &str) -> Message {
        let mut msg = message_in(session, index, text);
        msg.role = Role::Assistant;
        msg
    }

    #[rstest]
    #[tokio::test]
    async fn last_reply_is_the_newest_assistant_text_whatever_the_arrival_order() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        db.append(&reply_in(&session, 2, "all tests pass now")).await.unwrap();
        db.append(&reply_in(&session, 1, "looking into it")).await.unwrap();
        db.append(&message_in(&session, 3, "thanks")).await.unwrap();

        let s = db.get_session(&session).await.unwrap().unwrap();
        assert_eq!(s.last_reply.as_deref(), Some("all tests pass now"));
    }

    #[rstest]
    #[tokio::test]
    async fn a_session_keeps_how_it_relates_to_its_parent() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        let parent = handle(HarnessKind::ClaudeCode, "parent");
        let mut first = message_in(&session, 0, "copied from the parent");
        first.parent = Some(parent.clone());
        first.parent_kind = Some(ParentKind::Fork);
        db.append(&first).await.unwrap();
        // A record from an older build carries the parent but not the kind: it must not erase it.
        let mut older = message_in(&session, 1, "carried on");
        older.parent = Some(parent);
        db.append(&older).await.unwrap();

        let s = db.get_session(&session).await.unwrap().unwrap();
        assert_eq!(s.parent_kind, Some(ParentKind::Fork));
        let listed = db.list_sessions(&SessionFilter::default()).await.unwrap();
        assert_eq!(listed[0].parent_kind, Some(ParentKind::Fork));
    }

    #[rstest]
    #[tokio::test]
    async fn session_preview_and_last_reply_are_clipped() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        db.append(&message_in(&session, 0, &"p".repeat(5_000))).await.unwrap();
        db.append(&reply_in(&session, 1, &"r".repeat(5_000))).await.unwrap();

        let s = db.get_session(&session).await.unwrap().unwrap();
        assert_eq!(s.preview.unwrap(), format!("{}…", "p".repeat(600)));
        assert_eq!(s.last_reply.unwrap(), format!("{}…", "r".repeat(600)));
    }

    #[rstest]
    #[tokio::test]
    async fn any_term_preview_windows_around_a_late_prefix_match() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let filler = "lorem ".repeat(200);
        db.append(&message_in(&sample_handle(), 0, &format!("{filler}the deployment failed")))
            .await
            .unwrap();

        let hits: Vec<_> = search_in(&db, "deploy", None, true).try_collect().await.unwrap();
        let preview = hits[0].preview.to_plain().text.into_owned();
        assert!(preview.contains("deployment failed"), "{preview}");
    }

    #[rstest]
    #[tokio::test]
    async fn any_term_search_matches_some_words_and_prefixes() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        db.append(&message_in(&sample_handle(), 0, "the deployment failed")).await.unwrap();

        let strict: Vec<_> =
            search_in(&db, "deploy broken", None, false).try_collect().await.unwrap();
        assert!(strict.is_empty(), "no message has both whole words");
        let loose: Vec<_> =
            search_in(&db, "deploy broken", None, true).try_collect().await.unwrap();
        assert_eq!(loose.len(), 1, "`deploy` matches `deployment` as a prefix");
    }

    #[rstest]
    #[tokio::test]
    async fn search_in_filters_by_directory_and_its_children_only() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let in_dir = |id: &str, cwd: &str| {
            let mut msg = message_in(&handle(HarnessKind::ClaudeCode, id), 0, "shared keyword");
            msg.cwd = Some(std::path::PathBuf::from(cwd));
            msg
        };
        for msg in [
            in_dir("root", "/work/atuin"),
            in_dir("child", "/work/atuin/crates"),
            in_dir("sibling", "/work/atuin.sh"),
        ] {
            db.append(&msg).await.unwrap();
        }

        let mut found: Vec<String> = search_in(&db, "shared", Some("/work/atuin/"), false)
            .map_ok(|m| m.session.handle.session.as_ref().to_owned())
            .try_collect()
            .await
            .unwrap();
        found.sort();
        assert_eq!(found, ["child", "root"]);
    }

    #[rstest]
    #[tokio::test]
    async fn search_in_matches_windows_subdirectories() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for (id, cwd) in [("sub", r"C:\work\atuin\crates"), ("sibling", r"C:\work\atuin.sh")] {
            let mut msg = message_in(&handle(HarnessKind::Codex, id), 0, "shared keyword");
            msg.cwd = Some(std::path::PathBuf::from(cwd));
            db.append(&msg).await.unwrap();
        }

        let found: Vec<String> = search_in(&db, "shared", Some(r"C:\work\atuin\"), false)
            .map_ok(|m| m.session.handle.session.as_ref().to_owned())
            .try_collect()
            .await
            .unwrap();
        assert_eq!(found, ["sub"]);
    }

    #[rstest]
    #[tokio::test]
    async fn search_in_uses_the_session_directory_not_the_message_one() {
        // Codex records cwd on a metadata row; the prompt that matches carries none.
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = handle(HarnessKind::Codex, "codex");
        let mut meta = message_in(&session, 0, "");
        meta.cwd = Some(std::path::PathBuf::from("/work/terminal"));
        db.append(&meta).await.unwrap();
        db.append(&message_in(&session, 1, "osc hyperlink bug")).await.unwrap();

        let hits: Vec<_> =
            search_in(&db, "hyperlink", Some("/work/terminal"), false).try_collect().await.unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn search_reports_the_matched_message_index() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = handle(HarnessKind::ClaudeCode, "s");
        for (index, text) in (0..).zip(["first", "second", "needle here", "last"]) {
            db.append(&message_in(&session, index, text)).await.unwrap();
        }

        let hits = search(&db, "needle").await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].message_index, 2);
    }

    #[rstest]
    #[tokio::test]
    async fn search_preview_leaves_out_message_metadata() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let mut msg = message_in(&sample_handle(), 0, "the needle");
        msg.cwd = Some(std::path::PathBuf::from("/some/project"));
        db.append(&msg).await.unwrap();

        let hit = &search(&db, "needle").await[0];
        assert!(!hit.preview.to_plain().text.contains("/some/project"));
        // The metadata is still searchable.
        assert_eq!(search(&db, "project").await.len(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn search_limit_bounds_the_number_of_sessions() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for i in 0..5 {
            let session = handle(HarnessKind::ClaudeCode, &format!("session-{i}"));
            db.append(&message_in(&session, i, "common term")).await.unwrap();
        }

        let two: Vec<_> = db
            .search("common", SearchTerms::Typed, &SessionFilter::default(), 2)
            .try_collect()
            .await
            .unwrap();
        assert_eq!(two.len(), 2);
        let all: Vec<_> = db
            .search("common", SearchTerms::Typed, &SessionFilter::default(), 0)
            .try_collect()
            .await
            .unwrap();
        assert_eq!(all.len(), 5);
    }

    #[rstest]
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

        let two: Vec<_> = db
            .search("keyword", SearchTerms::Typed, &SessionFilter::default(), 2)
            .try_collect()
            .await
            .unwrap();
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

        // A trailing space finishes the term, so it is not a prefix of `apple`.
        let hits = search(&db, "app ").await;
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
        db.append(&message_in(&sample_handle(), 0, &format!("{blob} needle here"))).await.unwrap();

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

        let mut renamed = message_in(&session, 1, "more body text");
        renamed.session_title = Some("BetaTitle".to_owned());
        db.append(&renamed).await.unwrap();

        assert_eq!(search(&db, "BetaTitle").await.len(), 1, "the new title becomes searchable");
        assert!(search(&db, "AlphaTitle").await.is_empty(), "the retired title no longer matches");
    }

    /// The newest row decides the title: a cleared title clears it (and its index), while a
    /// row older than the newest one, arriving late, changes nothing.
    #[rstest]
    #[tokio::test]
    async fn the_newest_row_decides_the_title() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        let title_of = async |db: &AiSessionDatabase| {
            let s = db.get_session(&session).await.unwrap().unwrap();
            (s.title, s.title_source)
        };

        let mut named = message_in(&session, 1, "body");
        named.session_title = Some("GammaTitle".to_owned());
        named.session_title_source = Some(TitleSource::Named);
        db.append(&named).await.unwrap();
        assert_eq!(title_of(&db).await, (Some("GammaTitle".to_owned()), Some(TitleSource::Named)));

        let late = message_in(&session, 0, "an older row, captured late");
        db.append(&late).await.unwrap();
        assert_eq!(title_of(&db).await.0.as_deref(), Some("GammaTitle"), "an older row is ignored");

        let cleared = message_in(&session, 2, "after the name was cleared");
        db.append(&cleared).await.unwrap();
        assert_eq!(title_of(&db).await, (None, None));
        assert!(search(&db, "GammaTitle").await.is_empty(), "the cleared title no longer matches");
    }

    /// A title reaches the rows indexed before it arrived, and an unchanged title on later rows
    /// leaves the index as it is.
    #[rstest]
    #[tokio::test]
    async fn a_title_propagates_to_earlier_rows_only_when_it_changes() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = sample_handle();
        let indexed_with_title = |title: &'static str| {
            let pool = db.db.pool().clone();
            async move {
                atuin_common::db::query_scalar::<_, i64>(
                    "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH ?",
                )
                .bind(format!("title:{title}"))
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };

        db.append(&message_in(&session, 0, "untitled opener")).await.unwrap();
        assert_eq!(indexed_with_title("GammaTitle").await, 0);

        let mut titled = message_in(&session, 1, "first titled row");
        titled.session_title = Some("GammaTitle".to_owned());
        db.append(&titled).await.unwrap();
        assert_eq!(indexed_with_title("GammaTitle").await, 2, "the opener is re-indexed");

        let mut again = message_in(&session, 2, "same title again");
        again.session_title = Some("GammaTitle".to_owned());
        db.append(&again).await.unwrap();
        assert_eq!(indexed_with_title("GammaTitle").await, 3);
        assert_eq!(search(&db, "GammaTitle").await.len(), 1);
    }

    // --- search as you type, highlights, ranking -----------------------------------------------

    /// A query without terms lists sessions, newest first.
    #[rstest]
    #[case::empty("")]
    #[case::whitespace("   \t\n ")]
    #[tokio::test]
    async fn an_empty_query_lists_sessions_newest_first(#[case] query: &str) {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for m in three_sessions_oldest_first() {
            db.append(&m).await.unwrap();
        }
        let hits = search(&db, query).await;
        let ids: Vec<_> = hits.iter().map(|m| m.session.handle.session.to_string()).collect();
        assert_eq!(ids, ["session-2", "session-1", "session-0"]);
        assert!(hits.iter().all(|m| !m.title.has_match() && !m.preview.has_match()));
        assert_eq!(search_with(&db, query, &SessionFilter::default(), 2).await.len(), 2);
    }

    /// The last term matches as a prefix while it is being typed, and whole once finished.
    #[rstest]
    #[case::partial_last_term("refac", 1)]
    #[case::partial_after_a_whole_term("cargo tes", 1)]
    #[case::finished_term("refac ", 0)]
    #[case::only_the_last_term_is_a_prefix("carg test", 0)]
    #[case::partial_phrase("foo-ba", 1)]
    #[case::a_lone_character_is_a_whole_token("r", 0)]
    #[case::two_characters_are_a_prefix("re", 1)]
    #[tokio::test]
    async fn the_last_term_matches_as_a_prefix(#[case] query: &str, #[case] hits: usize) {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        db.append(&message_in(&sample_handle(), 0, "refactor the cargo testsuite foo-bar"))
            .await
            .unwrap();
        assert_eq!(search(&db, query).await.len(), hits, "query {query:?}");
    }

    /// The highlighted text of `h`, with matches in brackets.
    fn marked(h: &atuin_common::string::highlighted::HighlightedString) -> String {
        h.display_subs(['[', ']']).to_string()
    }

    #[rstest]
    #[case::whole_token("error", "An Error and an error.", "An [Error] and an [error].")]
    #[case::prefix_marks_the_whole_token("refac", "Refactoring it", "[Refactoring] it")]
    #[case::no_substring_matches("app ", "apple app", "apple [app]")]
    #[case::folded_like_the_index("cafe ", "the café opens", "the [café] opens")]
    #[case::phrase_across_words("foo-bar ", "foo bar foo baz", "[foo] [bar] foo baz")]
    #[case::every_term("build fail", "the build failed", "the [build] [failed]")]
    #[case::stray_markers_are_stripped("x ", "a\u{E000}b x", "ab [x]")]
    fn highlights_mark_what_the_index_matched(
        #[case] query: &str,
        #[case] text: &str,
        #[case] expected: &str,
    ) {
        let highlighted = QueryTerms::parse(query, SearchTerms::Typed)
            .highlight(TextHighlighter::default(), text);
        assert_eq!(marked(&highlighted), expected);
    }

    #[rstest]
    #[tokio::test]
    async fn search_highlights_the_title_and_snippet() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let filler: String = (0..200).map(|i| format!("filler-{i:03} ")).collect();
        let mut m = message_in(&sample_handle(), 0, &format!("{filler}we refactored the parser"));
        m.session_title = Some("Parser refactor".to_owned());
        db.append(&m).await.unwrap();

        let hits = search(&db, "refac").await;
        assert_eq!(hits.len(), 1);
        assert_eq!(marked(&hits[0].title), "Parser [refactor]");
        let preview = marked(&hits[0].preview);
        assert!(preview.contains("we [refactored] the parser"), "{preview:?}");
        assert!(!preview.contains("filler-000"), "{preview:?}");
    }

    #[rstest]
    #[tokio::test]
    async fn a_title_match_outranks_a_body_match() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let body = handle(HarnessKind::ClaudeCode, "body");
        let titled = handle(HarnessKind::ClaudeCode, "titled");
        db.append(&message_in(&body, 0, "we talked about the tokenizer at length")).await.unwrap();
        let mut m = message_in(&titled, 0, "unrelated words entirely here");
        m.session_title = Some("tokenizer".to_owned());
        db.append(&m).await.unwrap();

        let hits = search(&db, "tokenizer").await;
        assert_eq!(hits[0].session.handle, titled);
    }

    /// Equally relevant sessions rank newest first.
    #[rstest]
    #[tokio::test]
    async fn recency_breaks_equal_relevance() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let now = OffsetDateTime::now_utc().unix_timestamp();
        for (id, age_days) in [("old", 90), ("new", 1), ("mid", 30)] {
            let mut m = message_in(&handle(HarnessKind::Pi, id), 0, "same words here");
            m.timestamp = OffsetDateTime::from_unix_timestamp(now - age_days * 86_400).unwrap();
            db.append(&m).await.unwrap();
        }
        let ids: Vec<_> = search(&db, "words")
            .await
            .iter()
            .map(|m| m.session.handle.session.to_string())
            .collect();
        assert_eq!(ids, ["new", "mid", "old"]);
    }

    // --- hosts ----------------------------------------------------------------------------------

    fn host(n: u128) -> HostId {
        HostId(uuid::Uuid::from_u128(n))
    }

    #[rstest]
    #[tokio::test]
    async fn the_capturing_host_is_kept_on_rows_and_sessions() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let mut m = sample_message();
        m.host = Some(host(1));
        db.append(&m).await.unwrap();

        let session = db.get_session(&m.session).await.unwrap().unwrap();
        assert_eq!(session.host, Some(host(1)));
        let rows: Vec<_> = db.messages(&m.session).try_collect().await.unwrap();
        assert_eq!(rows[0].host, Some(host(1)));
    }

    /// A row stored before hosts were tracked learns its host when its record is replayed, and
    /// a known host is never overwritten.
    #[rstest]
    #[tokio::test]
    async fn a_replayed_row_fills_in_a_missing_host() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let m = sample_message();
        db.append(&m).await.unwrap();
        assert_eq!(db.get_session(&m.session).await.unwrap().unwrap().host, None);

        let mut replayed = m.clone();
        replayed.host = Some(host(1));
        assert_eq!(db.append(&replayed).await.unwrap(), Appended::Duplicate);
        replayed.host = Some(host(2));
        db.append(&replayed).await.unwrap();

        assert_eq!(db.get_session(&m.session).await.unwrap().unwrap().host, Some(host(1)));
        let rows: Vec<_> = db.messages(&m.session).try_collect().await.unwrap();
        assert_eq!(rows[0].host, Some(host(1)));
    }

    /// Forgetting a host removes its sessions, index rows and usage claims, and nothing else:
    /// a call it shared goes to the claimant left, and a group it headed regroups.
    #[rstest]
    #[tokio::test]
    async fn forgetting_a_host_removes_only_its_sessions() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let on = |mut m: Message, n| {
            m.host = Some(host(n));
            m
        };
        // `gone` (host 1) heads a group holding `kept-child` (host 2); both claim call A.
        db.append(&on(call_row("gone", None, "g1", 0, Some("A"), 10), 1)).await.unwrap();
        db.append(&on(call_row("kept-child", Some("gone"), "k1", 1, Some("A"), 10), 2))
            .await
            .unwrap();
        db.append(&on(call_row("kept", None, "k2", 2, None, 5), 2)).await.unwrap();
        let pi = |id: &str| handle(HarnessKind::Pi, id);
        assert_eq!(output_of(&db, "kept-child").await, 0, "the parent owns the shared call");
        assert_eq!(
            db.get_session(&pi("kept-child")).await.unwrap().unwrap().root,
            Some(pi("gone"))
        );

        db.forget_host(host(1)).await.unwrap();

        assert!(db.get_session(&pi("gone")).await.unwrap().is_none());
        let rows: Vec<_> = db.messages(&pi("gone")).try_collect().await.unwrap();
        assert!(rows.is_empty());
        assert_eq!(output_of(&db, "kept-child").await, 10, "the call moves to the claimant left");
        assert_eq!(output_of(&db, "kept").await, 5);
        let child = db.get_session(&pi("kept-child")).await.unwrap().unwrap();
        assert!(child.is_root(), "a group whose root went regroups");
        assert_eq!(search(&db, "x").await.len(), 2, "only the forgotten rows leave the index");

        // Replaying the host's records restores it.
        db.append(&on(call_row("gone", None, "g1", 0, Some("A"), 10), 1)).await.unwrap();
        assert_eq!(output_of(&db, "gone").await, 10);
        assert_eq!(output_of(&db, "kept-child").await, 0);
        let child = db.get_session(&pi("kept-child")).await.unwrap().unwrap();
        assert_eq!(child.root, Some(pi("gone")));
    }

    // --- grouping -------------------------------------------------------------------------------

    /// A row of session `id` with parent `parent`, at `seconds`, saying `text`.
    fn tree_row(id: &str, parent: Option<&str>, seconds: i64, text: &str) -> Message {
        let mut m = message_in(&handle(HarnessKind::ClaudeCode, id), seconds, text);
        m.source_id = SourceId::from(format!("{id}-{seconds}"));
        m.parent = parent.map(|p| handle(HarnessKind::ClaudeCode, p));
        m
    }

    /// Two trees and an orphan whose parent is never stored: `root` with a fork, a subagent
    /// and the fork's own subagent; `other` with a fork; `orphan` naming a missing `ghost`, with a
    /// child of its own. Every session's first row names its parent except `late`, which only
    /// learns it on its second row.
    fn forest() -> Vec<Message> {
        vec![
            tree_row("root", None, 0, "root words"),
            tree_row("fork", Some("root"), 10, "fork words"),
            tree_row("agent-a", Some("root"), 11, "subagent words"),
            tree_row("agent-b", Some("fork"), 12, "nested needle words"),
            tree_row("late", None, 13, "late words"),
            tree_row("late", Some("fork"), 14, "late words again"),
            tree_row("other", None, 20, "other words"),
            tree_row("other-fork", Some("other"), 21, "other fork words"),
            tree_row("orphan", Some("ghost"), 30, "orphan words"),
            tree_row("orphan-child", Some("orphan"), 31, "orphan child words"),
        ]
    }

    async fn roots_of(db: &AiSessionDatabase) -> Vec<(String, String)> {
        let mut roots: Vec<_> = db
            .list_sessions(&SessionFilter::default())
            .await
            .unwrap()
            .into_iter()
            .map(|s| (s.handle.session.to_string(), s.group().session.to_string()))
            .collect();
        roots.sort();
        roots
    }

    /// The root every session of [`forest`] groups under, sorted.
    fn forest_roots() -> Vec<(String, String)> {
        [
            ("agent-a", "root"),
            ("agent-b", "root"),
            ("fork", "root"),
            ("late", "root"),
            ("orphan", "orphan"),
            ("orphan-child", "orphan"),
            ("other", "other"),
            ("other-fork", "other"),
            ("root", "root"),
        ]
        .map(|(id, root)| (id.to_owned(), root.to_owned()))
        .to_vec()
    }

    /// Every session groups under its top-most stored ancestor whatever order its rows (and
    /// its ancestors') arrive in: children before parents, parents learned late.
    #[rstest]
    fn grouping_does_not_depend_on_arrival_order() {
        let shuffled =
            proptest::strategy::Strategy::prop_shuffle(proptest::strategy::Just(forest()));
        proptest::proptest!(|(rows in shuffled)| {
            let roots = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let db = AiSessionDatabase::in_memory().await.unwrap();
                    for m in &rows {
                        db.append(m).await.unwrap();
                    }
                    roots_of(&db).await
                });
            proptest::prop_assert_eq!(roots, forest_roots());
        });
    }

    /// The missing parent arriving at last adopts the group that waited for it.
    #[rstest]
    #[tokio::test]
    async fn a_late_parent_adopts_its_waiting_children() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        db.append(&tree_row("child", Some("parent"), 1, "x")).await.unwrap();
        db.append(&tree_row("grandchild", Some("child"), 2, "x")).await.unwrap();
        let child = db.get_session(&handle(HarnessKind::ClaudeCode, "child")).await.unwrap();
        assert!(child.unwrap().is_root(), "a session whose parent is not stored is a root");

        db.append(&tree_row("parent", Some("grandparent"), 0, "x")).await.unwrap();
        db.append(&tree_row("grandparent", None, 0, "x")).await.unwrap();
        let roots = roots_of(&db).await;
        assert!(roots.iter().all(|(_, root)| root == "grandparent"), "{roots:?}");
    }

    /// Sessions continued in other harnesses group under the session
    /// they continue, in whichever order their rows arrive (as a reprojection from the synced
    /// records replays them), and are its continuations: by the kind capture recorded, else
    /// (in records from before kinds were) because they name another harness's session.
    #[rstest]
    #[tokio::test]
    async fn continuations_in_other_harnesses_group_under_the_original(
        #[values(false, true)] reversed: bool,
        #[values(None, Some(ParentKind::Continuation))] kind: Option<ParentKind>,
    ) {
        let row = |harness, id: &str, parent: Option<(HarnessKind, &str)>, seconds| {
            let mut m = message_in(&handle(harness, id), seconds, "words");
            m.source_id = SourceId::from(format!("{id}-{seconds}"));
            m.parent = parent.map(|(h, p)| handle(h, p));
            m.parent_kind = m.parent.as_ref().and(kind);
            m
        };
        let mut rows = vec![
            row(HarnessKind::Codex, "orig", None, 0),
            // The parent is named after a line naming nothing (Pi's header, Codex's
            // session_meta).
            row(HarnessKind::ClaudeCode, "in-claude", None, 10),
            row(HarnessKind::ClaudeCode, "in-claude", Some((HarnessKind::Codex, "orig")), 11),
            row(HarnessKind::Pi, "in-pi", Some((HarnessKind::ClaudeCode, "in-claude")), 20),
        ];
        if reversed {
            rows.reverse();
        }
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for m in &rows {
            db.append(m).await.unwrap();
        }
        let original = handle(HarnessKind::Codex, "orig");
        for (harness, id) in [(HarnessKind::ClaudeCode, "in-claude"), (HarnessKind::Pi, "in-pi")] {
            let s = db.get_session(&handle(harness, id)).await.unwrap().unwrap();
            assert_eq!(s.group(), &original, "{id}");
            assert_eq!(s.parent_kind, kind, "{id}");
            assert_eq!(s.relation(), SessionRelation::Continuation, "{id}");
        }
        let children: Vec<String> = db
            .children(&original)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.handle.session.to_string())
            .collect();
        assert_eq!(children, ["in-pi", "in-claude"]);
    }

    /// A cycle of parent links (`a` → `b` → `c` → `a`, with `x` hanging off it) cannot loop, and
    /// has no top: it and what hangs off it group under its least member, whatever order its
    /// rows arrive in, and as a regroup of everything decides.
    #[rstest]
    #[tokio::test]
    async fn a_parent_cycle_groups_under_its_least_member(
        #[values([0, 1, 2, 3], [3, 2, 1, 0], [1, 3, 0, 2], [2, 0, 3, 1])] order: [usize; 4],
    ) {
        let rows = [
            tree_row("x", Some("a"), 0, "x"),
            tree_row("a", Some("b"), 1, "x"),
            tree_row("b", Some("c"), 2, "x"),
            tree_row("c", Some("a"), 3, "x"),
        ];
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for i in order {
            db.append(&rows[i]).await.unwrap();
        }
        let expected: Vec<_> =
            ["a", "b", "c", "x"].map(|id| (id.to_owned(), "a".to_owned())).to_vec();
        assert_eq!(roots_of(&db).await, expected);

        let mut conn = db.db.pool().acquire().await.unwrap();
        AiSessionDatabase::regroup_all(&mut conn).await.unwrap();
        drop(conn);
        assert_eq!(roots_of(&db).await, expected);
    }

    /// `s000` ← `s001` ← … ← `s{n-1}`: a chain of parents `n` long.
    fn chain_ids(n: usize) -> Vec<(String, Option<String>)> {
        (0..n).map(|i| (format!("s{i:03}"), i.checked_sub(1).map(|p| format!("s{p:03}")))).collect()
    }

    /// However long a chain of forks, it is one group, whether placed as its rows arrive or
    /// regrouped all at once.
    #[rstest]
    #[tokio::test]
    async fn a_deep_chain_is_one_group() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for (i, (id, parent)) in chain_ids(100).iter().enumerate() {
            let seconds = i64::try_from(i).unwrap();
            db.append(&tree_row(id, parent.as_deref(), seconds, "x")).await.unwrap();
        }
        let one_group = |roots: Vec<(String, String)>| roots.iter().all(|(_, r)| r == "s000");
        assert!(one_group(roots_of(&db).await));

        let mut conn = db.db.pool().acquire().await.unwrap();
        AiSessionDatabase::regroup_all(&mut conn).await.unwrap();
        drop(conn);
        assert!(one_group(roots_of(&db).await), "{:?}", roots_of(&db).await);
    }

    // --- copies ---------------------------------------------------------------------------------

    /// A line of Claude Code session `id`: its own uuid `source`, and for an assistant line the
    /// API message id `turn`, with the call's usage.
    fn claude_row(
        id: &str,
        parent: Option<&str>,
        source: &str,
        seconds: i64,
        turn: Option<&str>,
        output: u64,
    ) -> Message {
        let mut m = tree_row(id, parent, seconds, "words");
        m.source_id = SourceId::from(source.to_owned());
        if let Some(turn) = turn {
            m.role = Role::Assistant;
            m.turn_id = Some(turn.to_owned());
            m.usage = Some(Usage {
                input: Some(1),
                output: Some(output),
                cache_read: Some(0),
                cache_write: Some(0),
                reasoning: None,
            });
        }
        m
    }

    /// Sessions as `claude --resume <id> --fork-session` (or a `--resume` Claude Code turns into
    /// a fork) leave them. `resumed` starts with a copy of `original`'s lines (their uuids,
    /// timestamps and API message ids) under its own session id, naming `original` nowhere, then
    /// goes on; `original` goes on too. `twice` is a copy of `resumed` in turn. `agent-x` is a
    /// subagent of `resumed` and `branch` a `/branch` fork of `original`, both naming their
    /// parent, and `zeta` shares nothing.
    fn copies() -> Vec<Message> {
        let history = [
            ("u1", 0, None, 0),
            ("a1", 1, Some("msg_A"), 10),
            ("u2", 2, None, 0),
            ("a2", 3, Some("msg_B"), 20),
        ];
        let mut rows = Vec::new();
        for id in ["original", "resumed", "twice"] {
            for (source, seconds, turn, output) in history {
                rows.push(claude_row(id, None, source, seconds, turn, output));
            }
        }
        for id in ["resumed", "twice"] {
            rows.push(claude_row(id, None, "u3", 100, None, 0));
            rows.push(claude_row(id, None, "a3", 101, Some("msg_C"), 7));
        }
        rows.extend([
            claude_row("original", None, "a9", 50, Some("msg_D"), 5),
            claude_row("twice", None, "a4", 200, Some("msg_G"), 3),
            claude_row("agent-x", Some("resumed"), "x1", 102, Some("msg_E"), 2),
            claude_row("branch", Some("original"), "u1", 0, None, 0),
            claude_row("branch", Some("original"), "a1", 1, Some("msg_A"), 10),
            claude_row("branch", Some("original"), "b1", 60, Some("msg_F"), 4),
            claude_row("zeta", None, "z1", 0, Some("msg_Z"), 1),
        ]);
        rows
    }

    /// Every session's group, and the session it was found to be a copy of.
    async fn copy_links(db: &AiSessionDatabase) -> Vec<(String, String, Option<String>)> {
        let mut links: Vec<_> = db
            .list_sessions(&SessionFilter::default())
            .await
            .unwrap()
            .into_iter()
            .map(|s| {
                let copy_of = s.copy_of.as_ref().map(|c| c.session.to_string());
                (s.handle.session.to_string(), s.group().session.to_string(), copy_of)
            })
            .collect();
        links.sort();
        links
    }

    fn copies_links() -> Vec<(String, String, Option<String>)> {
        [
            ("agent-x", "original", None),
            ("branch", "original", None),
            ("original", "original", None),
            ("resumed", "original", Some("original")),
            ("twice", "original", Some("original")),
            ("zeta", "zeta", None),
        ]
        .map(|(id, root, copy_of)| (id.to_owned(), root.to_owned(), copy_of.map(str::to_owned)))
        .to_vec()
    }

    /// Each session's attributed output, from [`copies`].
    async fn copies_output(db: &AiSessionDatabase) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        for s in db.list_sessions(&SessionFilter::default()).await.unwrap() {
            out.push((s.handle.session.to_string(), s.usage.output.unwrap()));
        }
        out.sort();
        out
    }

    /// A copy groups under the session it was copied from, which names it nowhere, whatever
    /// order the rows arrive in: copied lines before the original's, a start that moves
    /// earlier, a copy of a copy. Each shared call still counts once, with the group's root.
    #[rstest]
    fn copies_group_under_their_original_in_any_order() {
        let shuffled =
            proptest::strategy::Strategy::prop_shuffle(proptest::strategy::Just(copies()));
        proptest::proptest!(|(rows in shuffled)| {
            let (links, output) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let db = AiSessionDatabase::in_memory().await.unwrap();
                    for m in &rows {
                        db.append(m).await.unwrap();
                    }
                    (copy_links(&db).await, copies_output(&db).await)
                });
            proptest::prop_assert_eq!(links, copies_links());
            let expected: Vec<_> = [
                ("agent-x", 2),
                ("branch", 4),
                ("original", 35),
                ("resumed", 7),
                ("twice", 3),
                ("zeta", 1),
            ]
            .map(|(id, out)| (id.to_owned(), out))
            .to_vec();
            proptest::prop_assert_eq!(output, expected);
        });
    }

    /// An assistant row of `harness` session `id` holding model call `turn`.
    fn turn_row(harness: HarnessKind, id: &str, seconds: i64, turn: &str) -> Message {
        let mut m = message_in(&handle(harness, id), seconds, "words");
        m.source_id = SourceId::from(format!("{id}-{seconds}"));
        m.role = Role::Assistant;
        m.turn_id = Some(turn.to_owned());
        m
    }

    /// The turn ids of each harness's form: whether the harness gave it, so that two sessions
    /// holding it were copied one from the other, or capture derived it from the line's content,
    /// which unrelated sessions can share.
    fn turn_forms() -> Vec<(HarnessKind, &'static str, bool)> {
        vec![
            (HarnessKind::ClaudeCode, "msg_01ABC", true),
            (HarnessKind::Codex, "resp_0123abcd", true),
            (HarnessKind::Codex, "token_count:12000.0.40.0.12040", false),
            (HarnessKind::Codex, "token_count:2026-09-18T10:00:00.000Z:12000.0.40.0.12040", false),
            (HarnessKind::Codex, "thread:12000.0.40.0.12040", false),
            (HarnessKind::Pi, "entry:a1b2c3d4:2026-09-18T10:00:00.000Z", true),
            (HarnessKind::Pi, "line:message:2026-09-18T10:00:00.000Z:1758189600000:10:5::", false),
            (HarnessKind::Opencode, "1758189600000:anthropic/claude-sonnet-4", false),
            (HarnessKind::Opencode, "1758189600000:anthropic/claude#10/5/0/0/0", false),
        ]
    }

    /// Two parentless sessions holding one call are linked, the later a copy of the earlier,
    /// only when the harness gave the call its id.
    #[rstest]
    #[tokio::test]
    async fn only_a_harness_given_turn_id_links_copies() {
        for (harness, turn, links) in turn_forms() {
            let db = AiSessionDatabase::in_memory().await.unwrap();
            db.append(&turn_row(harness, "first", 0, turn)).await.unwrap();
            db.append(&turn_row(harness, "second", 10, turn)).await.unwrap();
            let second = db.get_session(&handle(harness, "second")).await.unwrap().unwrap();
            let expected = links.then(|| handle(harness, "first"));
            assert_eq!(second.copy_of, expected, "{harness:?} {turn}");
            assert_eq!(second.is_root(), !links, "{harness:?} {turn}");
        }
    }

    /// Migrating links the copies stored before by the same rule as rows arriving later.
    #[rstest]
    #[tokio::test]
    async fn migrating_links_only_by_harness_given_turn_ids() {
        let forms = turn_forms();
        let db = sidecar_at(BEFORE_INCREMENTAL, &[]).await;
        for (i, (harness, turn, _)) in forms.iter().enumerate() {
            for (n, copy) in ["first", "second"].into_iter().enumerate() {
                let id = format!("{copy}-{i}");
                let at = i64::try_from(n).unwrap();
                db::query(
                    "INSERT INTO sessions (harness, session_id, started_at, updated_at) VALUES \
                     (?, ?, ?, ?)",
                )
                .bind(*harness as i64)
                .bind(&id)
                .bind(at)
                .bind(at)
                .execute(db.db.pool())
                .await
                .unwrap();
                db::query(
                    "INSERT INTO messages (id, harness, session, source_id, timestamp, role, \
                     content, turn_id) VALUES (randomblob(16), ?1, (SELECT id FROM sessions WHERE \
                     harness = ?1 AND session_id = ?2), 'a1', ?3, '\"Assistant\"', '[]', ?4)",
                )
                .bind(*harness as i64)
                .bind(&id)
                .bind(at)
                .bind(*turn)
                .execute(db.db.pool())
                .await
                .unwrap();
            }
        }

        db.migrate().await.unwrap();

        for (i, (harness, turn, links)) in forms.into_iter().enumerate() {
            let second = handle(harness, &format!("second-{i}"));
            let second = db.get_session(&second).await.unwrap().unwrap();
            let expected = links.then(|| handle(harness, &format!("first-{i}")));
            assert_eq!(second.copy_of, expected, "{harness:?} {turn}");
            assert_eq!(
                second.group().session.as_ref(),
                if links {
                    format!("first-{i}")
                } else {
                    format!("second-{i}")
                }
            );
        }
    }

    /// One roots-only row for the lot, with the copies counted as its children and shown as
    /// forks (the copy's subagent is grouped under it too, but is no child a person carried on).
    #[rstest]
    #[tokio::test]
    async fn a_copy_is_listed_under_its_original() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for m in copies() {
            db.append(&m).await.unwrap();
        }
        let groups: Vec<_> = db
            .list_sessions(&roots_only())
            .await
            .unwrap()
            .into_iter()
            .map(|s| (s.handle.session.to_string(), s.child_count))
            .collect();
        assert_eq!(groups, [("original".to_owned(), 3), ("zeta".to_owned(), 0)]);
        let resumed = db.get_session(&handle(HarnessKind::ClaudeCode, "resumed")).await.unwrap();
        assert_eq!(resumed.unwrap().relation(), crate::ai_session::SessionRelation::Fork);
    }

    /// A copy that learns its parent after all groups by it, and its own copies find another
    /// original.
    #[rstest]
    #[tokio::test]
    async fn a_parent_learned_late_outranks_a_copy_link() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        // The copies take up from the original's second line, so start after it.
        db.append(&claude_row("original", None, "u1", 0, None, 0)).await.unwrap();
        db.append(&claude_row("original", None, "a1", 1, Some("msg_A"), 1)).await.unwrap();
        db.append(&claude_row("copy", None, "a1", 1, Some("msg_A"), 1)).await.unwrap();
        db.append(&claude_row("copy2", None, "a1", 1, Some("msg_A"), 1)).await.unwrap();
        db.append(&claude_row("other", None, "o1", 0, None, 0)).await.unwrap();
        assert!(
            copy_links(&db).await.iter().all(|(id, root, _)| id == "other" || root == "original")
        );

        db.append(&claude_row("original", Some("other"), "a2", 5, None, 0)).await.unwrap();

        let links = copy_links(&db).await;
        assert_eq!(
            links,
            [
                ("copy", "copy", None),
                ("copy2", "copy", Some("copy")),
                ("original", "other", None),
                ("other", "other", None),
            ]
            .map(|(id, root, copy_of): (&str, &str, Option<&str>)| {
                (id.to_owned(), root.to_owned(), copy_of.map(str::to_owned))
            })
        );
    }

    /// Forgetting the host that captured an original leaves its copies linked to the best
    /// original left.
    #[rstest]
    #[tokio::test]
    async fn forgetting_an_original_relinks_its_copies() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for (id, n) in [("original", 1), ("resumed", 2), ("twice", 2)] {
            let mut m = claude_row(id, None, "a1", 0, Some("msg_A"), 1);
            m.host = Some(host(n));
            db.append(&m).await.unwrap();
        }
        db.forget_host(host(1)).await.unwrap();

        let links = copy_links(&db).await;
        assert_eq!(links, [
            ("resumed".to_owned(), "resumed".to_owned(), None),
            ("twice".to_owned(), "resumed".to_owned(), Some("resumed".to_owned())),
        ]);
    }

    fn roots_only() -> SessionFilter {
        SessionFilter {
            roots_only: true,
            ..SessionFilter::default()
        }
    }

    #[rstest]
    #[tokio::test]
    async fn roots_only_lists_each_group_once_with_its_size() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for m in forest() {
            db.append(&m).await.unwrap();
        }
        let groups: Vec<_> = db
            .list_sessions(&roots_only())
            .await
            .unwrap()
            .into_iter()
            .map(|s| {
                assert!(s.is_root());
                (s.handle.session.to_string(), s.child_count, s.group_updated_at.unwrap())
            })
            .collect();
        let at = |seconds| OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(seconds);
        // Newest activity in the group first: `root`'s newest is `late`, at 14. Its size counts
        // `fork` and `late`, not the subagents grouped under it.
        assert_eq!(groups, [
            ("orphan".to_owned(), 1, at(31)),
            ("other".to_owned(), 1, at(21)),
            ("root".to_owned(), 2, at(14)),
        ]);
    }

    /// Where capture recorded how a child relates to its parent, that decides, whatever the
    /// harness or id suggest: a Codex subagent is not counted in its root's group size, a Codex
    /// fork is, and so is a Claude Code child named like a subagent that is a fork.
    #[rstest]
    #[case::codex_subagent(
        HarnessKind::Codex,
        "t2",
        ParentKind::Subagent,
        SessionRelation::Subagent
    )]
    #[case::codex_fork(HarnessKind::Codex, "t2", ParentKind::Fork, SessionRelation::Fork)]
    #[case::opencode_fork(HarnessKind::Opencode, "ses_2", ParentKind::Fork, SessionRelation::Fork)]
    #[case::claude_fork(
        HarnessKind::ClaudeCode,
        "agent-z",
        ParentKind::Fork,
        SessionRelation::Fork
    )]
    #[case::claude_subagent(
        HarnessKind::ClaudeCode,
        "sub",
        ParentKind::Subagent,
        SessionRelation::Subagent
    )]
    #[case::codex_continuation(
        HarnessKind::Codex,
        "t2",
        ParentKind::Continuation,
        SessionRelation::Continuation
    )]
    #[tokio::test]
    async fn a_recorded_parent_kind_decides_the_relation_and_the_group_size(
        #[case] harness: HarnessKind,
        #[case] child: &str,
        #[case] kind: ParentKind,
        #[case] expected: SessionRelation,
    ) {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        db.append(&message_in(&handle(harness, "root"), 0, "root words")).await.unwrap();
        let mut m = message_in(&handle(harness, child), 1, "child words");
        m.parent = Some(handle(harness, "root"));
        m.parent_kind = Some(kind);
        db.append(&m).await.unwrap();

        let stored = db.get_session(&handle(harness, child)).await.unwrap().unwrap();
        assert_eq!(stored.relation(), expected);
        let groups = db.list_sessions(&roots_only()).await.unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].child_count, u64::from(expected.carries_on()));
    }

    /// A match in a nested child is its root's, snippet included.
    #[rstest]
    #[tokio::test]
    async fn a_childs_match_counts_toward_its_root() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for m in forest() {
            db.append(&m).await.unwrap();
        }
        let hits = search_with(&db, "needle", &roots_only(), 0).await;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.handle, handle(HarnessKind::ClaudeCode, "root"));
        assert_eq!(hits[0].session.child_count, 2);
        assert!(marked(&hits[0].preview).contains("[needle]"));

        // Every session of a group matching counts once, as its root.
        let hits = search_with(&db, "words", &roots_only(), 0).await;
        assert_eq!(hits.len(), 3);
        let all = search(&db, "words").await;
        assert_eq!(all.len(), 9);
    }

    /// A root whose group matches only in a child names that child, with the matched message's
    /// place in it and the child's title highlighted; the root's title, which only shares a word
    /// with the query, is not highlighted as if it matched.
    #[rstest]
    #[tokio::test]
    async fn a_match_in_a_child_names_the_child() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let titled = |mut m: Message, title: &str| {
            m.session_title = Some(title.to_owned());
            m
        };
        for m in [
            titled(tree_row("root", None, 0, "root words"), "haystack plans"),
            titled(tree_row("fork", Some("root"), 10, "fork words"), "needle hunt"),
            titled(tree_row("fork", Some("root"), 11, "a haystack in the fork"), "needle hunt"),
        ] {
            db.append(&m).await.unwrap();
        }

        let hits = search_with(&db, "needle haystack", &roots_only(), 0).await;
        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.session.handle, handle(HarnessKind::ClaudeCode, "root"));
        let matched = hit.matched.as_ref().expect("the match is the child's");
        assert_eq!(matched.handle, handle(HarnessKind::ClaudeCode, "fork"));
        assert_eq!(marked(&matched.title), "[needle] hunt");
        assert_eq!(marked(&hit.title), "haystack plans", "the root's title did not match");
        assert_eq!(hit.message_index, 1, "the message's place in the child");
        assert!(marked(&hit.preview).contains("[haystack]"));

        // Without grouping, the child is returned itself, and names nothing else.
        let hits = search_with(&db, "needle haystack", &SessionFilter::default(), 0).await;
        assert_eq!(hits[0].session.handle, handle(HarnessKind::ClaudeCode, "fork"));
        assert!(hits[0].matched.is_none());
        assert_eq!(hits[0].message_index, 1);
    }

    /// Whether a title alone matches the query as the index would: every term, or any with
    /// [`SearchTerms::Any`], the last as a prefix while typing.
    #[rstest]
    #[case::every_term("needle haystack", SearchTerms::All, "a haystack needle", true)]
    #[case::one_term_short("needle haystack", SearchTerms::All, "haystack plans", false)]
    #[case::any_term("needle haystack", SearchTerms::Any, "haystack plans", true)]
    #[case::typed_prefix("needle hay", SearchTerms::Typed, "needle haystack", true)]
    #[case::whole_word("needle hay", SearchTerms::All, "needle haystack", false)]
    fn a_title_matches_the_query_alone(
        #[case] query: &str,
        #[case] mode: SearchTerms,
        #[case] title: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(QueryTerms::parse(query, mode).matched_by(title), expected);
    }

    // --- filters --------------------------------------------------------------------------------

    fn filtered_row(id: &str, cwd: &str, branch: &str, model: &str, host_n: u128) -> Message {
        let mut m = message_in(&handle(HarnessKind::Codex, id), 0, "shared words");
        m.cwd = Some(std::path::PathBuf::from(cwd));
        m.git_branch = Some(branch.to_owned());
        m.model = Some(model.to_owned());
        m.host = Some(host(host_n));
        m
    }

    #[rstest]
    #[case::host(SessionFilter { host: Some(host(2)), ..SessionFilter::default() }, &["b"])]
    #[case::workspace_is_a_path_prefix(
        SessionFilter { workspace: Some("/work/atuin/".into()), ..SessionFilter::default() },
        &["a", "b"]
    )]
    #[case::root_workspace(
        SessionFilter { workspace: Some("/".into()), ..SessionFilter::default() },
        &["a", "b", "c", "d"]
    )]
    #[case::directory_is_exact(
        SessionFilter { directory: Some("/work/atuin".into()), ..SessionFilter::default() },
        &["a"]
    )]
    #[case::branch(SessionFilter { branch: Some("main".into()), ..SessionFilter::default() }, &["a", "c"])]
    #[case::model_substring_any_case(
        SessionFilter { model: Some("OPUS".into()), ..SessionFilter::default() },
        &["a", "d"]
    )]
    #[case::harness(harness_filter(HarnessKind::Pi), &[])]
    #[case::all_must_hold(
        SessionFilter {
            workspace: Some("/work/atuin".into()),
            branch: Some("main".into()),
            ..SessionFilter::default()
        },
        &["a"]
    )]
    #[tokio::test]
    async fn filters_select_sessions(#[case] filter: SessionFilter, #[case] expected: &[&str]) {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for m in [
            filtered_row("a", "/work/atuin", "main", "claude-opus-4", 1),
            filtered_row("b", "/work/atuin/crates/x", "feat", "gpt-5", 2),
            // A sibling sharing the workspace's name as a string prefix is not under it.
            filtered_row("c", "/work/atuin2", "main", "gpt-5", 1),
            filtered_row("d", "/home/me", "dev", "opus", 1),
        ] {
            db.append(&m).await.unwrap();
        }

        let ids = |sessions: Vec<Session>| {
            let mut ids: Vec<_> = sessions.iter().map(|s| s.handle.session.to_string()).collect();
            ids.sort();
            ids
        };
        assert_eq!(ids(db.list_sessions(&filter).await.unwrap()), expected, "listing");
        let hits = search_with(&db, "shared", &filter, 0).await;
        assert_eq!(ids(hits.into_iter().map(|m| m.session).collect()), expected, "search");
    }

    /// Sessions from any of several hosts (a host name standing for several ids), filtered before
    /// the limit, and alongside the other filters.
    /// Sessions with no recorded host (`e`) count only when the set says so.
    /// With roots only, a group passes when any of its sessions does, and shows as its root.
    #[rstest]
    #[tokio::test]
    async fn a_child_passing_the_filter_brings_its_root() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let mut root = tree_row("root", None, 0, "words");
        root.model = Some("claude-opus".to_owned());
        let mut agent = tree_row("agent-a", Some("root"), 1, "words");
        agent.model = Some("claude-haiku".to_owned());
        db.append(&root).await.unwrap();
        db.append(&agent).await.unwrap();

        let filter = SessionFilter {
            model: Some("haiku".to_owned()),
            roots_only: true,
            ..SessionFilter::default()
        };
        let listed = db.list_sessions(&filter).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].handle, handle(HarnessKind::ClaudeCode, "root"));
        assert_eq!(search_with(&db, "words", &filter, 0).await.len(), 1);
    }

    // --- lookups by id and group ----------------------------------------------------------------

    #[rstest]
    #[case::plain("ab", &["ab", "ab%c", "ab_c", "abc", "abxc"])]
    #[case::percent_is_literal("ab%", &["ab%c"])]
    #[case::underscore_is_literal("ab_", &["ab_c"])]
    #[case::exact("abc", &["abc"])]
    #[case::case_matters("AB", &["AB"])]
    #[case::nothing("abz", &[])]
    #[case::empty("", &["AB", "ab", "ab%c", "ab_c", "abc", "abxc", "ac", "é\u{10FFFF}x"])]
    #[case::the_last_code_point("é\u{10FFFF}", &["é\u{10FFFF}x"])]
    #[tokio::test]
    async fn an_id_prefix_is_not_a_pattern(#[case] prefix: &str, #[case] expected: &[&str]) {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let ids = ["ab", "ab%c", "ab_c", "abc", "abxc", "ac", "AB", "é\u{10FFFF}x"];
        for (i, id) in ids.into_iter().enumerate() {
            // Across harnesses: an id prefix doesn't say which.
            let harness = if i % 2 == 0 {
                HarnessKind::ClaudeCode
            } else {
                HarnessKind::Codex
            };
            db.append(&message_in(&handle(harness, id), 0, "words")).await.unwrap();
        }

        let mut found: Vec<String> = db
            .sessions_with_id_prefix(prefix)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.handle.session.to_string())
            .collect();
        found.sort();
        assert_eq!(found, expected);
    }

    #[rstest]
    #[case::ascii("ab", Some("ac"))]
    #[case::empty("", None)]
    #[case::last_code_point("a\u{10FFFF}", Some("b"))]
    #[case::only_the_last_code_point("\u{10FFFF}", None)]
    #[case::skips_surrogates("\u{D7FF}", Some("\u{E000}"))]
    fn prefix_successors(#[case] prefix: &str, #[case] expected: Option<&str>) {
        assert_eq!(prefix_successor(prefix).as_deref(), expected);
    }

    #[rstest]
    #[tokio::test]
    async fn children_are_the_group_under_a_root() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for m in forest() {
            db.append(&m).await.unwrap();
        }
        let children = |id: &'static str| {
            let db = db.clone();
            async move {
                db.children(&handle(HarnessKind::ClaudeCode, id))
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|s| s.handle.session.to_string())
                    .collect::<Vec<_>>()
            }
        };
        // Newest first, nested ones included, the root itself not.
        assert_eq!(children("root").await, ["late", "agent-b", "agent-a", "fork"]);
        assert_eq!(children("other").await, ["other-fork"]);
        // Only roots have groups.
        assert!(children("fork").await.is_empty());
        assert!(children("missing").await.is_empty());
    }

    // --- schema ---------------------------------------------------------------------------------

    /// [`SCHEMA_VERSION`] is the newest migration's version.
    #[rstest]
    fn the_schema_version_is_the_newest_migration() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ai_session/migrations");
        let newest = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| {
                let name = entry.unwrap().file_name().into_string().unwrap();
                name.split('_').next()?.parse::<i64>().ok()
            })
            .max();
        assert_eq!(newest, Some(SCHEMA_VERSION));
    }

    /// A sidecar at `version`, holding `sessions`, (harness, id, parent) rows written as that
    /// version stored them.
    async fn sidecar_at(version: i64, sessions: &[(&str, Option<&str>)]) -> AiSessionDatabase {
        let db = Sqlite::builder_in_memory().open().await.unwrap();
        #[allow(clippy::disallowed_macros)]
        let mut migrator = sqlx::migrate!("./src/ai_session/migrations");
        migrator.migrations =
            migrator.migrations.iter().filter(|m| m.version <= version).cloned().collect();
        migrator.run(db.pool()).await.unwrap();
        for (i, (id, parent)) in sessions.iter().enumerate() {
            db::query(
                "INSERT INTO sessions (harness, session_id, parent_harness, parent_session_id, \
                 started_at, updated_at) VALUES (1, ?, ?, ?, ?, ?)",
            )
            .bind(*id)
            .bind(parent.map(|_| 1))
            .bind(*parent)
            .bind(i64::try_from(i).unwrap())
            .bind(i64::try_from(i).unwrap())
            .execute(db.pool())
            .await
            .unwrap();
        }
        AiSessionDatabase::from_sqlite(db)
    }

    /// The version before the `incremental_sidecar` migration: a released sidecar.
    const BEFORE_INCREMENTAL: i64 = 5;

    /// Store a row of `session` holding model call `turn` in a sidecar at
    /// [`BEFORE_INCREMENTAL`], as that version stored them.
    async fn old_row(db: &AiSessionDatabase, i: usize, session: &str, turn: &str) {
        db::query(
            "INSERT INTO messages (id, harness, session, source_id, timestamp, role, content, \
             turn_id) VALUES (?, 1, (SELECT id FROM sessions WHERE harness = 1 AND session_id = \
             ?), 'a1', ?, '\"Assistant\"', '[]', ?)",
        )
        .bind(vec![u8::try_from(i).unwrap(); 16])
        .bind(session)
        .bind(i64::try_from(i).unwrap())
        .bind(turn)
        .execute(db.db.pool())
        .await
        .unwrap();
    }

    /// Migrating groups the sessions already stored, and starts without reproject watermarks so
    /// the next daemon start replays every record (filling in hosts).
    #[rstest]
    #[tokio::test]
    async fn migrating_groups_stored_sessions_and_forces_a_reproject() {
        let db = sidecar_at(BEFORE_INCREMENTAL, &[
            ("agent-b", Some("fork")),
            ("fork", Some("root")),
            ("root", None),
            ("orphan", Some("ghost")),
            ("a", Some("b")),
            ("b", Some("a")),
        ])
        .await;

        db.migrate().await.unwrap();

        let roots = roots_of(&db).await;
        let root_of = |id: &str| roots.iter().find(|(s, _)| s == id).unwrap().1.clone();
        assert_eq!(root_of("agent-b"), "root");
        assert_eq!(root_of("fork"), "root");
        assert_eq!(root_of("root"), "root");
        assert_eq!(root_of("orphan"), "orphan");
        // A parent cycle has no top: its least member heads it.
        assert_eq!(root_of("a"), "a");
        assert_eq!(root_of("b"), "a");
        let watermarks: i64 = db::query_scalar("SELECT count(*) FROM reproject_watermark")
            .fetch_one(db.db.pool())
            .await
            .unwrap();
        assert_eq!(watermarks, 0);

        // Grouping carries on incrementally from the migrated state.
        db.append(&tree_row("agent-c", Some("agent-b"), 50, "x")).await.unwrap();
        assert_eq!(roots_of(&db).await.iter().find(|(s, _)| s == "agent-c").unwrap().1, "root");
    }

    /// Migrating groups a chain of any length under its top.
    #[rstest]
    #[tokio::test]
    async fn migrating_groups_a_deep_chain_under_its_top() {
        let ids = chain_ids(100);
        let sessions: Vec<_> = ids.iter().map(|(id, p)| (id.as_str(), p.as_deref())).collect();
        let db = sidecar_at(BEFORE_INCREMENTAL, &sessions).await;

        db.migrate().await.unwrap();

        let roots = roots_of(&db).await;
        assert!(roots.iter().all(|(_, root)| root == "s000"), "{roots:?}");
    }

    /// Migrating links the copies already stored to their originals from the calls they share,
    /// and regroups them, from the rows alone.
    #[rstest]
    #[tokio::test]
    async fn migrating_links_stored_copies() {
        let db = sidecar_at(BEFORE_INCREMENTAL, &[
            ("original", None),
            ("resumed", None),
            ("agent-x", Some("resumed")),
            ("zeta", None),
        ])
        .await;
        for (i, (session, turn)) in
            [("original", "msg_A"), ("resumed", "msg_A"), ("zeta", "msg_Z")].into_iter().enumerate()
        {
            old_row(&db, i, session, turn).await;
        }

        db.migrate().await.unwrap();

        let links = copy_links(&db).await;
        let link = |id: &str| links.iter().find(|(s, ..)| s == id).unwrap().clone();
        assert_eq!(link("resumed").1, "original");
        assert_eq!(link("resumed").2.as_deref(), Some("original"));
        assert_eq!(link("agent-x").1, "original");
        assert_eq!(link("zeta").1, "zeta");

        // Linking carries on incrementally from the migrated state.
        db.append(&claude_row("twice", None, "a1", 5, Some("msg_A"), 1)).await.unwrap();
        let links = copy_links(&db).await;
        assert_eq!(links.iter().find(|(s, ..)| s == "twice").unwrap().1, "original");
    }

    // --- read-only open -------------------------------------------------------------------------

    #[fixture]
    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[rstest]
    #[tokio::test]
    async fn a_read_only_open_reads_beside_the_writer(dir: tempfile::TempDir) {
        let path = dir.path().join("sidecar.db");
        let writer = AiSessionDatabase::open(&path).await.unwrap();
        writer.append(&message_in(&sample_handle(), 0, "first words")).await.unwrap();

        let reader = AiSessionDatabase::open_read_only(&path).await.unwrap();
        assert_eq!(reader.list_sessions(&SessionFilter::default()).await.unwrap().len(), 1);

        // The writer keeps writing, and the reader sees it.
        writer.append(&message_in(&sample_handle(), 1, "second words")).await.unwrap();
        assert_eq!(search(&reader, "second").await.len(), 1);

        assert!(
            reader.append(&message_in(&sample_handle(), 2, "x")).await.is_err(),
            "a read-only sidecar refuses writes"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn a_read_only_open_never_creates_the_file(dir: tempfile::TempDir) {
        let path = dir.path().join("missing.db");
        assert!(matches!(AiSessionDatabase::open_read_only(&path).await, Err(DbError::Open(_))));
        assert!(!path.exists());
    }

    #[rstest]
    #[case::uninitialized(None)]
    #[case::outdated(Some(1))]
    #[case::one_behind(Some(SCHEMA_VERSION - 1))]
    #[case::unknown(Some(99))]
    #[tokio::test]
    async fn a_read_only_open_refuses_another_schema(
        dir: tempfile::TempDir,
        #[case] version: Option<i64>,
    ) {
        let path = dir.path().join("sidecar.db");
        {
            let db = Sqlite::builder(path.as_os_str()).open().await.unwrap();
            if let Some(version) = version {
                #[allow(clippy::disallowed_macros)]
                let mut migrator = sqlx::migrate!("./src/ai_session/migrations");
                migrator.migrations =
                    migrator.migrations.iter().filter(|m| m.version <= version).cloned().collect();
                migrator.run(db.pool()).await.unwrap();
                if version > SCHEMA_VERSION {
                    db::query(
                        "INSERT INTO _sqlx_migrations (version, description, success, checksum, \
                         execution_time) VALUES (?, 'future', 1, x'00', 0)",
                    )
                    .bind(version)
                    .execute(db.pool())
                    .await
                    .unwrap();
                }
            }
            db.close().await;
        }

        let err = AiSessionDatabase::open_read_only(&path).await.expect_err("must refuse");
        match version {
            None => assert!(matches!(err, DbError::Uninitialized {
                expected: SCHEMA_VERSION
            })),
            Some(99) => assert!(matches!(err, DbError::UnknownSchema { found: 99, .. })),
            Some(found) => {
                assert!(matches!(err, DbError::OutdatedSchema { found: f, .. } if f == found));
            }
        }
        // Each says what to do about it, which always involves the daemon.
        assert!(err.to_string().contains("atuin daemon"), "{err}");
    }

    // --- sidecars from other builds -------------------------------------------------------------

    /// `_sqlx_migrations` rows for this build's migrations up to `version`, as sqlx writes them.
    fn applied_up_to(version: i64) -> Vec<(i64, Vec<u8>, bool)> {
        MIGRATOR
            .iter()
            .filter(|m| m.version <= version)
            .map(|m| (m.version, m.checksum.to_vec(), true))
            .collect()
    }

    #[rstest]
    #[case::none(vec![], Lineage::Missing)]
    #[case::the_first(applied_up_to(1), Lineage::Known(1))]
    #[case::every_one(applied_up_to(SCHEMA_VERSION), Lineage::Known(SCHEMA_VERSION))]
    #[case::mains(applied_up_to(BEFORE_INCREMENTAL), Lineage::Known(BEFORE_INCREMENTAL))]
    #[case::a_development_build(
        [applied_up_to(2), vec![(3, vec![1], true), (4, vec![2], true)]].concat(),
        Lineage::Foreign
    )]
    #[case::a_newer_build(
        [applied_up_to(SCHEMA_VERSION), vec![(99, vec![0], true)]].concat(),
        Lineage::Newer(99)
    )]
    #[case::other_contents(
        [applied_up_to(1), vec![(2, vec![0], true)]].concat(),
        Lineage::Foreign
    )]
    #[case::one_skipped(
        applied_up_to(SCHEMA_VERSION).into_iter().filter(|(v, ..)| *v != 2).collect(),
        Lineage::Foreign
    )]
    #[case::one_failed(
        applied_up_to(SCHEMA_VERSION)
            .into_iter()
            .map(|(v, c, _)| (v, c, v != SCHEMA_VERSION))
            .collect(),
        Lineage::Foreign
    )]
    fn a_sidecars_lineage_is_told_from_its_migrations(
        #[case] applied: Vec<(i64, Vec<u8>, bool)>,
        #[case] expected: Lineage,
    ) {
        assert_eq!(AiSessionDatabase::classify(&applied), expected);
    }

    /// A sidecar a development build migrated with its own numbering: this build's first
    /// migration, then others at versions this build uses for different ones.
    async fn dev_build_sidecar(path: &Path) {
        let db = Sqlite::builder(path.as_os_str()).open().await.unwrap();
        #[allow(clippy::disallowed_macros)]
        let mut migrator = sqlx::migrate!("./src/ai_session/migrations");
        migrator.migrations =
            migrator.migrations.iter().filter(|m| m.version == 1).cloned().collect();
        migrator.run(db.pool()).await.unwrap();
        for (version, description) in [(2, "reproject watermark"), (3, "hosts and roots")] {
            db::query(
                "INSERT INTO _sqlx_migrations (version, description, success, checksum, \
                 execution_time) VALUES (?, ?, 1, x'00', 0)",
            )
            .bind(version)
            .bind(description)
            .execute(db.pool())
            .await
            .unwrap();
        }
        db::query(
            "INSERT INTO sessions (harness, session_id, started_at, updated_at) VALUES (1, 'old', \
             0, 0)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        db.close().await;
    }

    /// A sidecar another build migrated is rebuilt: deleted, then created afresh without
    /// watermarks, so the daemon's reprojection replays every record into it. A reader is told to
    /// wait for the daemon until then.
    #[rstest]
    #[tokio::test]
    async fn a_sidecar_from_another_build_is_rebuilt(dir: tempfile::TempDir) {
        let path = dir.path().join("sidecar.db");
        dev_build_sidecar(&path).await;

        let err = AiSessionDatabase::open_read_only(&path).await.expect_err("must refuse");
        assert!(matches!(err, DbError::ForeignSchema), "{err}");
        assert!(err.to_string().contains("atuin daemon"), "{err}");

        let db = AiSessionDatabase::open(&path).await.unwrap();
        assert_eq!(
            AiSessionDatabase::lineage(&db.db).await.unwrap(),
            Lineage::Known(SCHEMA_VERSION)
        );
        assert!(db.list_sessions(&SessionFilter::default()).await.unwrap().is_empty());
        assert!(db.reproject_watermarks().await.unwrap().is_empty());
        db.append(&message_in(&sample_handle(), 0, "after the rebuild")).await.unwrap();

        let reader = AiSessionDatabase::open_read_only(&path).await.unwrap();
        assert_eq!(search(&reader, "rebuild").await.len(), 1);
    }

    /// A sidecar a released daemon left (every migration before `incremental_sidecar`) is
    /// migrated in place, what
    /// it holds kept: only a sidecar of another lineage is rebuilt.
    #[rstest]
    #[tokio::test]
    async fn a_sidecar_from_main_is_migrated_in_place(dir: tempfile::TempDir) {
        let path = dir.path().join("sidecar.db");
        {
            let db = Sqlite::builder(path.as_os_str()).open().await.unwrap();
            #[allow(clippy::disallowed_macros)]
            let mut migrator = sqlx::migrate!("./src/ai_session/migrations");
            migrator.migrations = migrator
                .migrations
                .iter()
                .filter(|m| m.version <= BEFORE_INCREMENTAL)
                .cloned()
                .collect();
            migrator.run(db.pool()).await.unwrap();
            db::query(
                "INSERT INTO sessions (harness, session_id, parent_harness, parent_session_id, \
                 parent_kind, started_at, updated_at) VALUES (1, 'kept', 1, 'root', 1, 0, 0)",
            )
            .execute(db.pool())
            .await
            .unwrap();
            db.close().await;
        }

        let db = AiSessionDatabase::open(&path).await.unwrap();
        assert_eq!(
            AiSessionDatabase::lineage(&db.db).await.unwrap(),
            Lineage::Known(SCHEMA_VERSION)
        );
        let kept = db.get_session(&handle(HarnessKind::ClaudeCode, "kept")).await.unwrap().unwrap();
        assert_eq!(kept.parent_kind, Some(ParentKind::Fork));
        assert_eq!(kept.relation(), SessionRelation::Fork);
    }

    /// A newer build's sidecar is left alone: that build owns it, and this one reports it.
    #[rstest]
    #[tokio::test]
    async fn a_newer_builds_sidecar_is_kept(dir: tempfile::TempDir) {
        let path = dir.path().join("sidecar.db");
        {
            let db = AiSessionDatabase::open(&path).await.unwrap();
            db.append(&message_in(&sample_handle(), 0, "kept")).await.unwrap();
            db::query(
                "INSERT INTO _sqlx_migrations (version, description, success, checksum, \
                 execution_time) VALUES (99, 'future', 1, x'00', 0)",
            )
            .execute(db.db.pool())
            .await
            .unwrap();
            db.db.close().await;
        }

        assert!(AiSessionDatabase::open(&path).await.is_err());
        let reader = Sqlite::builder(path.as_os_str()).read_only().open().await.unwrap();
        let sessions: i64 = db::query_scalar("SELECT count(*) FROM sessions")
            .fetch_one(reader.pool())
            .await
            .unwrap();
        assert_eq!(sessions, 1);
    }
}
