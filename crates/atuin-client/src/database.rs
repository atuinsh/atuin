use std::env;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::time::Duration;

use atuin_common::filter::{self, OrFilter};
use atuin_common::sqlite::{Sqlite as CommonSqlite, SqliteBuilder};
use atuin_common::time::OffsetDateTimeExt;
use atuin_common::utils;
use atuin_domain::record::{CmdOrigin, UNKNOWN_USER};
use itertools::Itertools;
use sql_builder::bind::Bind;
use sql_builder::{SqlBuilder, SqlName, esc, quote};
use sqlx::sqlite::{SqlitePool, SqliteRow};
use sqlx::{Result, Row};
use time::OffsetDateTime;
use tracing::instrument;
use uuid::Uuid;

use super::history::History;
use super::ordering;
use super::settings::{FilterMode, SearchMode, Settings};
use crate::history::{AuthorKind, AuthorPattern, HistoryId, HistoryStats, KNOWN_AGENTS};

#[derive(Clone)]
pub struct Context {
    pub session: String,
    pub cwd: String,
    pub cmd_origin: CmdOrigin,
    pub host_id: String,
    pub git_root: Option<PathBuf>,
}

#[derive(Clone, Copy, Default)]
pub struct OptFilters<'a> {
    pub exit: Option<i64>,
    pub exclude_exit: Option<i64>,
    /// Only commands that recorded a non-zero exit. Unlike `exclude_exit: 0`,
    /// this also skips the `exit = -1` sentinel rows for commands still
    /// running (or whose end hook never fired).
    pub only_failed: bool,
    pub cwd: Option<&'a str>,
    pub exclude_cwd: Option<&'a str>,
    pub before: Option<&'a str>,
    pub after: Option<&'a str>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub reverse: bool,
    pub include_duplicates: bool,
    /// Author filter.
    pub authors: OrFilter<&'a [AuthorPattern]>,
    /// Shell filter. The empty string matches commands that have no recorded shell.
    pub shells: OrFilter<&'a [String]>,
}

/// Build a query [`Context`] without requiring a live shell session.
///
/// Outside of an atuin-hooked shell (e.g. when running as an MCP server),
/// `ATUIN_SESSION` is unset; the session is left empty so session-scoped
/// filters simply match nothing.
#[instrument(level = "trace", skip_all, err)]
pub async fn query_context() -> eyre::Result<Context> {
    let session = env::var("ATUIN_SESSION").unwrap_or_default();
    let cmd_origin = CmdOrigin::probe_current();
    let cwd = utils::get_current_dir();
    let host_id = Settings::host_id().await?;
    let git_root = utils::in_git_repo(cwd.as_str());

    Ok(Context {
        session,
        cmd_origin,
        cwd,
        git_root,
        host_id: host_id.0.as_simple().to_string(),
    })
}

#[instrument(level = "trace", skip_all, err)]
pub async fn current_context() -> eyre::Result<Context> {
    if env::var("ATUIN_SESSION").is_err() {
        return Err(eyre::eyre!(
            "Failed to find $ATUIN_SESSION in the environment. Check that you have correctly set \
             up your shell."
        ));
    }

    query_context().await
}

impl Context {
    #[must_use]
    pub fn from_history(entry: &History) -> Self {
        Self {
            session: entry.session.to_string(),
            cwd: entry.cwd.to_string(),
            cmd_origin: entry.cmd_origin.clone(),
            host_id: String::new(),
            git_root: utils::in_git_repo(entry.cwd.as_str()),
        }
    }
}

/// Each entry is OR'd: [`AuthorPattern::AllUser`] → not an agent, [`AuthorPattern::AllAgent`] → an
/// agent, [`AuthorPattern::Name`] → exact match.
fn apply_author_filter(sql: &mut SqlBuilder, authors: OrFilter<&[AuthorPattern]>) {
    let authors = match authors.items() {
        filter::Items::All => return,
        filter::Items::Some(a) => a,
    };

    // The username half of `hostname`, which is what `author` falls back to when nothing set it.
    let user_expr = "CASE WHEN instr(hostname, ':') > 0 THEN substr(hostname, instr(hostname, \
                     ':') + 1) ELSE hostname END";

    let author_expr = std::fmt::from_fn(|f| {
        write!(f, "CASE WHEN author IS NULL OR trim(author) = '' THEN {user_expr} ELSE author END")
    });

    let defaulted_expr = format!(
        "CASE WHEN instr(hostname, ':') = 0 THEN hostname WHEN substr(hostname, instr(hostname, \
         ':') + 1) = {unknown} THEN substr(hostname, 1, instr(hostname, ':') - 1) ELSE \
         substr(hostname, instr(hostname, ':') + 1) END",
        unknown = quote(UNKNOWN_USER),
    );

    // Mirrors [`History::is_agent`]: a recorded kind wins, and without one a known agent name means
    // an agent, unless the author is only the name it defaulted to — a NULL/blank author *is* only
    // that name, so it is never an agent. A kind we don't recognise (written by a newer version)
    // falls through to the name heuristic, exactly like [`AuthorKind::from_repr`] mapping it to
    // `None` — and so does a NULL kind, because `NULL IN (...)` is not true.
    let is_agent = || {
        format!(
            "CASE WHEN author_kind IN ({kinds}) THEN author_kind = {agent} WHEN author IS NULL OR \
             trim(author) = '' THEN 0 ELSE author IN ({names}) AND author <> {defaulted_expr} END",
            kinds = AuthorKind::VARIANTS.iter().map(|kind| kind.as_u8()).join(", "),
            agent = AuthorKind::Agent.as_u8(),
            names = KNOWN_AGENTS.iter().map(quote).join(", "),
        )
    };

    let mut conditions = authors.iter().map(|author| match author {
        AuthorPattern::AllUser => format!("NOT ({})", is_agent()),
        AuthorPattern::AllAgent => is_agent(),
        AuthorPattern::Name(name) => {
            format!("{author_expr} = {}", quote(name))
        }
    });

    // Note: `conditions` cannot be empty; `OrFilter::items` is always non-empty.
    sql.and_where(format!("({})", conditions.join(" OR ")));
}

fn apply_shell_filter(sql: &mut SqlBuilder, shells: OrFilter<&[String]>) {
    let shells = match shells.items() {
        filter::Items::All => return,
        filter::Items::Some(s) => s,
    };

    let mut include_null = false;
    let nonempty_shells = shells.iter().filter(|s| {
        let is_empty = s.is_empty();
        if is_empty {
            include_null = true;
        }
        !is_empty
    });

    let shell_list = nonempty_shells.map(quote).join(", ");
    let mut cond = (!shell_list.is_empty()).then(|| format!("shell in ({shell_list})"));

    if include_null {
        // `SqlBuilder::and_where` wraps the whole expression in parentheses; we don't need to add
        // them here.
        cond = Some(cond.map_or_else(String::new, |s| s + " OR ") + "shell IS NULL");
    }

    // `OrFilter::items` is always non-empty.
    sql.and_where(cond.expect("nonempty list of shells must result in at least one condition"));
}

fn get_session_start_time(session_id: &str) -> Option<i64> {
    // A session id is not guaranteed to be one of our UUIDv7s: ATUIN_SESSION comes from the
    // environment, and a stray value whose version nibble reads as v1/v6/v7 can carry a timestamp
    // far outside the unix-nanos range. Treat such a session as having no start time rather than
    // overflowing.
    let uuid = Uuid::parse_str(session_id).ok()?;
    let (seconds, nanos) = uuid.get_timestamp()?.to_unix();
    i64::try_from(seconds).ok()?.checked_mul(1_000_000_000)?.checked_add(i64::from(nanos))
}

/// SQL predicate to match for a [`CmdOrigin`].
fn origin_sql_filter(origin: &CmdOrigin) -> String {
    // This helper implements logic to support host-only matching against a combined host:user pair.
    // Normally, you'd think we could just do
    // `lower(hostname) = {origin.host().into_inner().to_lowercase()}`, but that does not work.
    //
    // Normally, `CmdOrigin` is intended to be serialized as a string "<host>:<user>", but, certain
    // parsing logic we historically had would parse a string "<host>", rather than
    // "<host>:unknown-user" (which other logic did). An example offender is the `nushell` importer.
    //
    // The database column `hostname` is a misnomer -- it actually refers to the `cmd_origin`. Some
    // importers, have parsed it as "<host>:<user>", others as "<host>" and others yet as "<host>:".
    //
    // In effect, there's this crappy data in the database. Another case for writing
    // application-specific types.
    //
    // You'd think that we could apply a migration and call it a day, but unfortunately that doesn't
    // work -- the `history` table is actually a cache over the `records` table, and the records
    // table only holds this information in encrypted, synced form. If we want to get it, we need to
    // decrypt all the local `records` rows, patch them up, re-encrypt them and then sync that to
    // the cloud. Recipe for disaster.
    //
    // So this function exists.
    let host = origin.host().into_inner().to_lowercase();
    format!(
        "(lower(hostname) = {eq} OR (lower(hostname) >= {lo} AND lower(hostname) < {hi}))",
        eq = quote(&host),
        lo = quote(format!("{host}:")),
        hi = quote(format!("{host};")),
    )
}

/// Controls the type of search [`Sqlite::search`] performs.
///
/// This is a narrower set of modes than [`SearchMode`], which also contains modes that apply only
/// to interactive searches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbSearchMode {
    Prefix,
    FullText,
    Fuzzy,
}

impl From<DbSearchMode> for SearchMode {
    fn from(mode: DbSearchMode) -> Self {
        match mode {
            DbSearchMode::Prefix => Self::Prefix,
            DbSearchMode::FullText => Self::FullText,
            DbSearchMode::Fuzzy => Self::Fuzzy,
        }
    }
}

// This impl is here rather than in settings.rs where `SearchMode` is defined to avoid having
// modules that cyclicly import each other, which isn't strictly disallowed but could be confusing.
impl SearchMode {
    /// Get the [`DbSearchMode`] that most closely matches this [`SearchMode`].
    ///
    /// This maps [`SearchMode::DaemonFuzzy`], which is interactive-only, to
    /// [`DbSearchMode::Fuzzy`].
    #[must_use]
    pub fn closest_db_mode(self) -> DbSearchMode {
        match self {
            Self::Prefix => DbSearchMode::Prefix,
            Self::FullText => DbSearchMode::FullText,
            Self::Fuzzy | Self::DaemonFuzzy => DbSearchMode::Fuzzy,
        }
    }
}

// Intended for use on a developer machine and not a sync server.
// TODO: implement IntoIterator
#[derive(Debug, Clone)]
pub struct Sqlite {
    sqlite: CommonSqlite,
}

impl From<CommonSqlite> for Sqlite {
    fn from(value: CommonSqlite) -> Self {
        Self { sqlite: value }
    }
}

impl<'r> ::sqlx::FromRow<'r, SqliteRow> for History {
    fn from_row(row: &'r SqliteRow) -> ::sqlx::Result<Self> {
        let deleted_at: Option<i64> = row.try_get("deleted_at")?;
        let hostname: String = row.try_get("hostname")?;
        let author: Option<String> = row.try_get("author").ok().flatten();
        let author = author.filter(|author| !author.trim().is_empty()).unwrap_or_else(|| {
            CmdOrigin::try_from(hostname.clone())
                .map_or_else(|err| err.0, |origin| origin.user().into_inner().to_owned())
        });
        let intent: Option<String> = row.try_get("intent").ok().flatten();
        let intent = intent.filter(|intent| !intent.trim().is_empty());
        let shell: Option<String> = row.try_get("shell").ok().flatten();
        let author_kind: Option<i64> = row.try_get("author_kind").ok().flatten();
        let author_kind =
            author_kind.and_then(|kind| u8::try_from(kind).ok()).and_then(AuthorKind::from_repr);

        Ok(Self::from_db()
            .id(row.try_get("id")?)
            .timestamp(OffsetDateTime::from_unix_nanos_i64(row.try_get("timestamp")?))
            .duration(row.try_get("duration")?)
            .exit(row.try_get("exit")?)
            .command(row.try_get("command")?)
            .cwd(row.try_get("cwd")?)
            .session(row.try_get("session")?)
            .hostname(hostname)
            .author(author)
            .intent(intent)
            .deleted_at(deleted_at.map(OffsetDateTime::from_unix_nanos_i64))
            .shell(shell)
            .author_kind(author_kind)
            .build()
            .into())
    }
}

/// A grouped history row plus its aggregate `count(*)`, used by the deduplicated
/// list/search query. `#[sqlx(flatten)]` reuses `History`'s `FromRow` impl.
#[derive(sqlx::FromRow)]
struct HistoryWithCount {
    #[sqlx(flatten)]
    history: History,
    count: i32,
}

impl Sqlite {
    #[instrument(level = "trace", skip_all, fields(timeout = ?timeout), err)]
    pub async fn new(path: impl AsRef<OsStr>, timeout: Duration) -> eyre::Result<Self> {
        let path = path.as_ref();
        debug!("opening sqlite database at {path:?}");

        Self::from_builder(CommonSqlite::builder(path), timeout).await
    }

    pub async fn in_memory(timeout: Duration) -> eyre::Result<Self> {
        Self::from_builder(CommonSqlite::builder_in_memory(), timeout).await
    }

    async fn from_builder(builder: SqliteBuilder<'_>, timeout: Duration) -> eyre::Result<Self> {
        let sqlite = builder.timeout(timeout).regexp().open().await?;

        Self::setup_db(sqlite.pool()).await?;

        Ok(Self { sqlite })
    }

    /// Close the underlying connection pool. Test-only: used to force query errors.
    #[cfg(test)]
    pub(crate) async fn close(&self) {
        self.sqlite.pool().close().await;
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn sqlite_version(&self) -> eyre::Result<semver::Version> {
        Ok(self.sqlite.info().await.version?)
    }

    #[instrument(level = "trace", skip_all, err)]
    async fn setup_db(pool: &SqlitePool) -> Result<()> {
        debug!("running sqlite database setup");

        sqlx::migrate!("./migrations").run(pool).await?;

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = ?h.id), err)]
    async fn save_raw(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, h: &History) -> Result<()> {
        sqlx::query(
            "insert or ignore into history(
                id, timestamp, duration, exit, command, cwd, session, hostname, author, intent,
                deleted_at, shell, author_kind
            ) values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )
        .bind(h.id.0.as_str())
        .bind(h.timestamp.unix_timestamp_nanos() as i64)
        .bind(h.duration)
        .bind(h.exit)
        .bind(h.command.as_str())
        .bind(h.cwd.as_str())
        .bind(h.session.as_str())
        .bind(h.cmd_origin.as_str())
        .bind(h.author.as_str())
        .bind(h.intent.as_deref())
        .bind(h.deleted_at.map(|t| t.unix_timestamp_nanos() as i64))
        .bind(h.shell.as_deref())
        .bind(h.author_kind.map(|kind| i64::from(kind.as_u8())))
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = ?id), err)]
    async fn delete_row_raw(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        id: HistoryId,
    ) -> Result<()> {
        sqlx::query("delete from history where id = ?1")
            .bind(id.0.as_str())
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = ?h.id), err)]
    pub async fn save(&self, h: &History) -> Result<()> {
        debug!("saving history to sqlite");
        let mut tx = self.sqlite.pool().begin().await?;
        Self::save_raw(&mut tx, h).await?;
        tx.commit().await?;

        Ok(())
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn save_bulk<'a>(&self, h: impl IntoIterator<Item = &'a History>) -> Result<()> {
        let mut h = h.into_iter().peekable();
        if h.peek().is_none() {
            return Ok(());
        }

        debug!("saving history to sqlite");

        const HISTORY_INSERT_COLUMNS: usize = 13;
        let rows_per_insert =
            (self.sqlite.info().await.variable_number_limit() / HISTORY_INSERT_COLUMNS).max(1);

        let mut tx = self.sqlite.pool().begin().await?;

        while h.peek().is_some() {
            let mut builder = sqlx::QueryBuilder::new(
                "insert or ignore into history(
                    id, timestamp, duration, exit, command, cwd, session, hostname, author, intent,
                    deleted_at, shell, author_kind
                ) ",
            );

            builder.push_values(h.by_ref().take(rows_per_insert), |mut b, h| {
                b.push_bind(h.id.0.as_str())
                    .push_bind(h.timestamp.unix_timestamp_nanos() as i64)
                    .push_bind(h.duration)
                    .push_bind(h.exit)
                    .push_bind(h.command.as_str())
                    .push_bind(h.cwd.as_str())
                    .push_bind(h.session.as_str())
                    .push_bind(h.cmd_origin.as_str())
                    .push_bind(h.author.as_str())
                    .push_bind(h.intent.as_deref())
                    .push_bind(h.deleted_at.map(|t| t.unix_timestamp_nanos() as i64))
                    .push_bind(h.shell.as_deref())
                    .push_bind(h.author_kind.map(|kind| i64::from(kind.as_u8())));
            });

            builder.build().execute(&mut *tx).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = ?id), err)]
    pub async fn load(&self, id: &str) -> Result<Option<History>> {
        debug!("loading history item {}", id);

        let res = sqlx::query_as::<_, History>("select * from history where id = ?1")
            .bind(id)
            .fetch_optional(self.sqlite.pool())
            .await?;

        Ok(res)
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn load_active(
        &self,
        ids: impl IntoIterator<Item = HistoryId>,
    ) -> Result<Vec<History>> {
        let mut iter_ids = ids.into_iter();
        let size_hint = iter_ids.size_hint();

        // sqlite caps bound parameters per statement (SQLITE_MAX_VARIABLE_NUMBER, as low as 999).
        // Chunk well under that.
        const CHUNK: usize = 500;

        if let Some(upper) = size_hint.1
            && size_hint.0 == upper
        {
            debug!("loading {} history items", size_hint.0);
        } else {
            debug!("loading somewhere around {} history items", size_hint.0);
        }

        let mut out = Vec::with_capacity(size_hint.0);

        // Buffer reused across multiple chunks to avoid reallocating.
        let mut chunk: Vec<HistoryId> = Vec::with_capacity(CHUNK);

        loop {
            chunk.clear();
            chunk.extend(iter_ids.by_ref().take(CHUNK));
            if chunk.is_empty() {
                break;
            }

            let placeholders = ["?"].repeat(chunk.len()).join(",");
            let sql = format!(
                "select * from history where id in ({placeholders}) and deleted_at is null"
            );

            let mut query = sqlx::query_as::<_, History>(sqlx::AssertSqlSafe(sql));
            for id in &chunk {
                query = query.bind(id.0.as_str());
            }

            let rows = query.fetch_all(self.sqlite.pool()).await?;
            out.extend(rows);
        }

        Ok(out)
    }

    #[instrument(level = "trace", skip_all, fields(id = ?h.id), err)]
    pub async fn update(&self, h: &History) -> Result<()> {
        debug!("updating sqlite history");

        sqlx::query(
            "update history
                set timestamp = ?2, duration = ?3, exit = ?4, command = ?5, cwd = ?6, session = \
             ?7, hostname = ?8, author = ?9, intent = ?10, deleted_at = ?11, author_kind = ?12
                where id = ?1",
        )
        .bind(h.id.0.as_str())
        .bind(h.timestamp.unix_timestamp_nanos() as i64)
        .bind(h.duration)
        .bind(h.exit)
        .bind(h.command.as_str())
        .bind(h.cwd.as_str())
        .bind(h.session.as_str())
        .bind(h.cmd_origin.as_str())
        .bind(h.author.as_str())
        .bind(h.intent.as_deref())
        .bind(h.deleted_at.map(|t| t.unix_timestamp_nanos() as i64))
        .bind(h.author_kind.map(|kind| i64::from(kind.as_u8())))
        .execute(self.sqlite.pool())
        .await?;

        Ok(())
    }

    // make a unique list, that only shows the *newest* version of things
    #[instrument(level = "trace", skip_all, fields(unique, include_deleted), err)]
    pub async fn list(
        &self,
        filters: impl IntoIterator<Item = FilterMode>,
        context: &Context,
        max: Option<usize>,
        unique: bool,
        include_deleted: bool,
        range: Option<(OffsetDateTime, OffsetDateTime)>,
    ) -> Result<Vec<History>> {
        debug!("listing history");

        let mut query = SqlBuilder::select_from(SqlName::new("history").alias("h").baquoted());
        query.field("*").order_desc("timestamp");
        if !include_deleted {
            query.and_where_is_null("deleted_at");
        }

        let git_root = if let Some(git_root) = context.git_root.clone() {
            git_root.to_str().unwrap_or("/").to_string()
        } else {
            context.cwd.clone()
        };

        let session_start = get_session_start_time(&context.session);

        for filter in filters {
            match filter {
                FilterMode::Global => &mut query,
                FilterMode::Host => query.and_where(origin_sql_filter(&context.cmd_origin)),
                FilterMode::Session => query.and_where_eq("session", quote(&context.session)),
                FilterMode::SessionPreload => {
                    query.and_where_eq("session", quote(&context.session));
                    if let Some(session_start) = session_start {
                        query.or_where_lt("timestamp", session_start);
                    }
                    &mut query
                }
                FilterMode::Directory => query.and_where_eq("cwd", quote(&context.cwd)),
                FilterMode::Workspace => query.and_where_like_left("cwd", &git_root),
            };
        }

        if unique {
            query.group_by("command").having("max(timestamp)");
        }

        if let Some(max) = max {
            query.limit(max);
        }

        // Inclusive on both ends, matching `range()`. `stats` relies on this to count a
        // command recorded exactly on a period boundary (e.g. at midnight).
        if let Some((from, to)) = range {
            query.and_where_ge("timestamp", from.unix_timestamp_nanos() as i64);
            query.and_where_le("timestamp", to.unix_timestamp_nanos() as i64);
        }

        let query = query.sql().expect("bug in list query. please report");

        let res = sqlx::query_as::<_, History>(sqlx::AssertSqlSafe(query))
            .fetch_all(self.sqlite.pool())
            .await?;

        Ok(res)
    }

    #[instrument(level = "trace", skip_all, fields(from = ?from, to = ?to), err)]
    pub async fn range(&self, from: OffsetDateTime, to: OffsetDateTime) -> Result<Vec<History>> {
        debug!("listing history from {:?} to {:?}", from, to);

        let res = sqlx::query_as::<_, History>(
            "select * from history where timestamp >= ?1 and timestamp <= ?2 order by timestamp \
             asc",
        )
        .bind(from.unix_timestamp_nanos() as i64)
        .bind(to.unix_timestamp_nanos() as i64)
        .fetch_all(self.sqlite.pool())
        .await?;

        Ok(res)
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn last(&self) -> Result<Option<History>> {
        let res = sqlx::query_as::<_, History>(
            "select * from history where duration >= 0 order by timestamp desc limit 1",
        )
        .fetch_optional(self.sqlite.pool())
        .await?;

        Ok(res)
    }

    #[instrument(level = "trace", skip_all, fields(count), err)]
    pub async fn before(&self, timestamp: OffsetDateTime, count: i64) -> Result<Vec<History>> {
        let res = sqlx::query_as::<_, History>(
            "select * from history where timestamp < ?1 order by timestamp desc limit ?2",
        )
        .bind(timestamp.unix_timestamp_nanos() as i64)
        .bind(count)
        .fetch_all(self.sqlite.pool())
        .await?;

        Ok(res)
    }

    #[instrument(level = "trace", skip_all, fields(include_deleted), err)]
    pub async fn history_count(&self, include_deleted: bool) -> Result<i64> {
        let query = if include_deleted {
            "select count(1) from history"
        } else {
            "select count(1) from history where deleted_at is null"
        };

        let res: (i64,) = sqlx::query_as(query).fetch_one(self.sqlite.pool()).await?;
        Ok(res.0)
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn search(
        &self,
        search_mode: DbSearchMode,
        filter: FilterMode,
        context: &Context,
        query: &str,
        filter_options: OptFilters<'_>,
    ) -> Result<Vec<History>> {
        // Build the inner query holding all of the user's filters (filter mode,
        // fuzzy/regex command matches, exit/cwd/date filters, author, deleted_at).
        // Deduplication, ordering and limiting are applied by the outer query
        // built below, so that the timestamp-ordered scan can early-terminate.
        let mut sql = SqlBuilder::select_from("history");

        let git_root = if let Some(git_root) = context.git_root.clone() {
            git_root.to_str().unwrap_or("/").to_string()
        } else {
            context.cwd.clone()
        };

        let session_start = get_session_start_time(&context.session);

        match filter {
            FilterMode::Global => &mut sql,
            FilterMode::Host => sql.and_where(origin_sql_filter(&context.cmd_origin)),
            FilterMode::Session => sql.and_where_eq("session", quote(&context.session)),
            FilterMode::SessionPreload => {
                sql.and_where_eq("session", quote(&context.session));
                if let Some(session_start) = session_start {
                    sql.or_where_lt("timestamp", session_start);
                }
                &mut sql
            }
            FilterMode::Directory => sql.and_where_eq("cwd", quote(&context.cwd)),
            FilterMode::Workspace => sql.and_where_like_left("cwd", git_root),
        };

        let orig_query = query;

        let mut regexes = Vec::new();
        match search_mode {
            DbSearchMode::Prefix => sql.and_where_like_left("command", query.replace('*', "%")),
            DbSearchMode::FullText | DbSearchMode::Fuzzy => {
                let mut is_or = false;
                for token in QueryTokenizer::new(query) {
                    // TODO smart case mode could be made configurable like in fzf
                    let (is_glob, glob) = if token.has_uppercase() {
                        (true, "*")
                    } else {
                        (false, "%")
                    };
                    let param = match token {
                        QueryToken::Regex(r) => {
                            regexes.push(String::from(r));
                            continue;
                        }
                        QueryToken::Or => {
                            if !is_or {
                                is_or = true;
                                continue;
                            } else {
                                format!("{glob}|{glob}")
                            }
                        }
                        QueryToken::MatchStart(term, _) => {
                            format!("{term}{glob}")
                        }
                        QueryToken::MatchEnd(term, _) => {
                            format!("{glob}{term}")
                        }
                        QueryToken::MatchFull(term, _) => {
                            format!("{glob}{term}{glob}")
                        }
                        QueryToken::Match(term, _) => {
                            if search_mode == DbSearchMode::FullText {
                                format!("{glob}{term}{glob}")
                            } else {
                                term.split("").join(glob)
                            }
                        }
                    };

                    sql.fuzzy_condition("command", param, token.is_inverse(), is_glob, is_or);
                    is_or = false;
                }

                &mut sql
            }
        };

        for regex in regexes {
            sql.and_where("command regexp ?".bind(&regex));
        }

        filter_options.exit.map(|exit| sql.and_where_eq("exit", exit));

        filter_options.exclude_exit.map(|exclude_exit| sql.and_where_ne("exit", exclude_exit));

        if filter_options.only_failed {
            sql.and_where("exit != 0 AND exit != -1");
        }

        filter_options.cwd.map(|cwd| sql.and_where_eq("cwd", quote(cwd)));

        filter_options.exclude_cwd.map(|exclude_cwd| sql.and_where_ne("cwd", quote(exclude_cwd)));

        if let Some(before) = filter_options.before {
            let parsed =
                interim::parse_date_string(before, OffsetDateTime::now_utc(), interim::Dialect::Uk)
                    .map_err(|e| {
                        sqlx::Error::Decode(
                            format!("invalid `before` filter {before:?}: {e}").into(),
                        )
                    })?;
            sql.and_where_lt("timestamp", quote(parsed.unix_timestamp_nanos() as i64));
        }

        if let Some(after) = filter_options.after {
            let parsed =
                interim::parse_date_string(after, OffsetDateTime::now_utc(), interim::Dialect::Uk)
                    .map_err(|e| {
                        sqlx::Error::Decode(format!("invalid `after` filter {after:?}: {e}").into())
                    })?;
            sql.and_where_gt("timestamp", quote(parsed.unix_timestamp_nanos() as i64));
        }

        apply_author_filter(&mut sql, filter_options.authors);
        apply_shell_filter(&mut sql, filter_options.shells);

        sql.and_where_is_null("deleted_at");

        // sql_builder inlines every bound value, so the inner query carries no
        // positional parameters and is safe to embed (twice) as a derived table.
        let inner = sql.sql().expect("bug in search query. please report");
        let inner = inner.trim().trim_end_matches(';');

        let order = if filter_options.reverse {
            "ASC"
        } else {
            "DESC"
        };

        let tail = match (filter_options.limit, filter_options.offset) {
            (Some(limit), Some(offset)) => format!(" LIMIT {limit} OFFSET {offset}"),
            (Some(limit), None) => format!(" LIMIT {limit}"),
            // SQLite requires a LIMIT before OFFSET; -1 means "no limit".
            (None, Some(offset)) => format!(" LIMIT -1 OFFSET {offset}"),
            (None, None) => String::new(),
        };

        // Deduplicate by keeping, for each command, only its most recent entry
        // within the filtered set. Expressed as a correlated NOT EXISTS rather
        // than GROUP BY so that the timestamp-ordered scan can stop as soon as
        // `limit` distinct commands have been emitted, instead of aggregating
        // the entire table on every keystroke. The `(timestamp, id)` row-value
        // comparison both breaks timestamp ties (one row per command) and stays
        // a sargable range scan on the (command, timestamp) index.
        let query = if filter_options.include_duplicates {
            format!("SELECT * FROM ({inner}) f ORDER BY f.timestamp {order}{tail}")
        } else {
            format!(
                "SELECT * FROM ({inner}) f WHERE NOT EXISTS ( SELECT 1 FROM ({inner}) f2 WHERE \
                 f2.command = f.command AND (f2.timestamp, f2.id) > (f.timestamp, f.id) ) ORDER \
                 BY f.timestamp {order}{tail}"
            )
        };

        let res = sqlx::query_as::<_, History>(sqlx::AssertSqlSafe(query))
            .fetch_all(self.sqlite.pool())
            .await?;

        // Rank against the same characters SQL matched: drop spaces, operators and negated terms.
        let reorder_query: String = QueryTokenizer::new(orig_query)
            .filter(|token| !token.is_inverse())
            .filter_map(|token| match token {
                QueryToken::Match(term, _)
                | QueryToken::MatchStart(term, _)
                | QueryToken::MatchEnd(term, _)
                | QueryToken::MatchFull(term, _) => Some(term),
                QueryToken::Or | QueryToken::Regex(_) => None,
            })
            .collect();
        Ok(ordering::reorder_fuzzy(search_mode, &reorder_query, res))
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn query_history(&self, query: &str) -> Result<Vec<History>> {
        let res = sqlx::query_as::<_, History>(sqlx::AssertSqlSafe(query))
            .fetch_all(self.sqlite.pool())
            .await?;

        Ok(res)
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn all_with_count(&self) -> Result<Vec<(History, i32)>> {
        debug!("listing history");

        let mut query = SqlBuilder::select_from(SqlName::new("history").alias("h").baquoted());

        query
            .fields(&[
                "id",
                "max(timestamp) as timestamp",
                "max(duration) as duration",
                "exit",
                "command",
                "deleted_at",
                "null as author",
                "null as intent",
                "null as author_kind",
                "group_concat(cwd, ':') as cwd",
                "group_concat(session) as session",
                "group_concat(hostname, ',') as hostname",
                "count(*) as count",
            ])
            .group_by("command")
            .group_by("exit")
            .and_where("deleted_at is null")
            .order_desc("timestamp");

        let query = query.sql().expect("bug in list query. please report");

        let res = sqlx::query_as::<_, HistoryWithCount>(sqlx::AssertSqlSafe(query))
            .fetch_all(self.sqlite.pool())
            .await?;

        Ok(res.into_iter().map(|r| (r.history, r.count)).collect())
    }

    #[must_use]
    pub fn all_paged(&self, page_size: usize, include_deleted: bool, unique: bool) -> Paged {
        Paged::new(self.clone(), page_size, include_deleted, unique)
    }

    // This used to scramble the command and set deleted_at, so that sync v1 could
    // propagate the deletion. Sync v2 propagates deletions through the record store
    // instead (HistoryRecord::Delete), and the only remaining caller deletes entries
    // that were never pushed to the store - so just delete the row.
    // deleted_at is still read to keep tombstones from older versions working.
    #[instrument(level = "trace", skip_all, fields(id = ?h.id), err)]
    pub async fn delete(&self, h: History) -> Result<()> {
        self.delete_rows([h.id]).await
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn delete_rows(&self, ids: impl IntoIterator<Item = HistoryId>) -> Result<()> {
        let mut ids = ids.into_iter().peekable();
        if ids.peek().is_none() {
            return Ok(());
        }

        let mut tx = self.sqlite.pool().begin().await?;

        for id in ids {
            Self::delete_row_raw(&mut tx, id.clone()).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = ?h.id), err)]
    pub async fn stats(&self, h: &History) -> Result<HistoryStats> {
        // We select the previous in the session by time. Excluding deleted
        // history matches every other read path, and lets the query use the
        // partial (session, timestamp) index.
        let mut prev = SqlBuilder::select_from("history");
        prev.field("*")
            .and_where("timestamp < ?1")
            .and_where("session = ?2")
            .and_where_is_null("deleted_at")
            .order_by("timestamp", true)
            .limit(1);

        let mut next = SqlBuilder::select_from("history");
        next.field("*")
            .and_where("timestamp > ?1")
            .and_where("session = ?2")
            .and_where_is_null("deleted_at")
            .order_by("timestamp", false)
            .limit(1);

        let mut total = SqlBuilder::select_from("history");
        total.field("count(1)").and_where("command = ?1");

        let mut average = SqlBuilder::select_from("history");
        average.field("avg(duration)").and_where("command = ?1");

        let mut exits = SqlBuilder::select_from("history");
        exits.fields(&["exit", "count(1) as count"]).and_where("command = ?1").group_by("exit");

        // rewrite the following with sqlbuilder
        let mut day_of_week = SqlBuilder::select_from("history");
        day_of_week
            .fields(&[
                "strftime('%w', ROUND(timestamp / 1000000000), 'unixepoch') AS day_of_week",
                "count(1) as count",
            ])
            .and_where("command = ?1")
            .group_by("day_of_week");

        // Intentionally format the string with 01 hardcoded. We want the average runtime for the
        // _entire month_, but will later parse it as a datetime for sorting
        // Sqlite has no datetime so we cannot do it there, and otherwise sorting will just be a
        // string sort, which won't be correct.
        let mut duration_over_time = SqlBuilder::select_from("history");
        duration_over_time
            .fields(&[
                "strftime('01-%m-%Y', ROUND(timestamp / 1000000000), 'unixepoch') AS month_year",
                "avg(duration) as duration",
            ])
            .and_where("command = ?1")
            .group_by("month_year")
            .having("duration > 0");

        let prev = prev.sql().expect("issue in stats previous query");
        let next = next.sql().expect("issue in stats next query");
        let total = total.sql().expect("issue in stats average query");
        let average = average.sql().expect("issue in stats previous query");
        let exits = exits.sql().expect("issue in stats exits query");
        let day_of_week = day_of_week.sql().expect("issue in stats day of week query");
        let duration_over_time =
            duration_over_time.sql().expect("issue in stats duration over time query");

        // The queries are all independent, so run them concurrently on the pool.
        #[allow(clippy::type_complexity)]
        let (prev, next, total, average, exits, day_of_week, duration_over_time): (
            _,
            _,
            (i64,),
            (f64,),
            Vec<(i64, i64)>,
            Vec<(String, i64)>,
            Vec<(String, f64)>,
        ) = tokio::try_join!(
            sqlx::query_as::<_, History>(sqlx::AssertSqlSafe(prev))
                .bind(h.timestamp.unix_timestamp_nanos() as i64)
                .bind(&h.session)
                .fetch_optional(self.sqlite.pool()),
            sqlx::query_as::<_, History>(sqlx::AssertSqlSafe(next))
                .bind(h.timestamp.unix_timestamp_nanos() as i64)
                .bind(&h.session)
                .fetch_optional(self.sqlite.pool()),
            sqlx::query_as(sqlx::AssertSqlSafe(total))
                .bind(&h.command)
                .fetch_one(self.sqlite.pool()),
            sqlx::query_as(sqlx::AssertSqlSafe(average))
                .bind(&h.command)
                .fetch_one(self.sqlite.pool()),
            sqlx::query_as(sqlx::AssertSqlSafe(exits))
                .bind(&h.command)
                .fetch_all(self.sqlite.pool()),
            sqlx::query_as(sqlx::AssertSqlSafe(day_of_week))
                .bind(&h.command)
                .fetch_all(self.sqlite.pool()),
            sqlx::query_as(sqlx::AssertSqlSafe(duration_over_time))
                .bind(&h.command)
                .fetch_all(self.sqlite.pool()),
        )?;

        let duration_over_time =
            duration_over_time.iter().map(|f| (f.0.clone(), f.1.round() as i64)).collect();

        Ok(HistoryStats {
            next,
            previous: prev,
            total: total.0 as u64,
            average_duration: average.0 as u64,
            exits,
            day_of_week,
            duration_over_time,
        })
    }

    #[instrument(level = "trace", skip_all, fields(before, dupkeep), err)]
    pub async fn get_dups(&self, before: i64, dupkeep: u32) -> Result<Vec<History>> {
        let res = sqlx::query_as::<_, History>(
            "SELECT * FROM (
                SELECT *, ROW_NUMBER()
                  OVER (PARTITION BY command, cwd, hostname ORDER BY timestamp DESC)
                  AS rn
                  FROM history
                ) sub
              WHERE rn > ?1 and timestamp < ?2;
            ",
        )
        .bind(dupkeep)
        .bind(before)
        .fetch_all(self.sqlite.pool())
        .await?;

        Ok(res)
    }
}

pub struct Paged {
    database: Sqlite,
    page_size: usize,
    last_id: Option<String>,
    include_deleted: bool,
    unique: bool,
}

impl Paged {
    #[must_use]
    pub fn new(database: Sqlite, page_size: usize, include_deleted: bool, unique: bool) -> Self {
        Self {
            database,
            page_size,
            last_id: None,
            include_deleted,
            unique,
        }
    }

    #[instrument(level = "trace", skip_all, err)]
    pub async fn next(&mut self) -> Result<Option<Vec<History>>> {
        let mut query = SqlBuilder::select_from(SqlName::new("history").alias("h").baquoted());

        query.field("*").order_desc("id");

        if !self.include_deleted {
            query.and_where_is_null("deleted_at");
        }

        if self.unique {
            // We want to deduplicate on command, but the user can search via cwd, hostname, and session.
            // Without those fields, filter modes won't work right. With those fields, we get duplicates.
            // This must be handled upstream.
            query.group_by("command, cwd, hostname, session").having("max(timestamp)");
        }

        query.limit(self.page_size);

        if let Some(last_id) = &self.last_id {
            query.and_where_lt("id", quote(last_id));
        }

        let query = query.sql().expect("bug in list query. please report");
        let res = self.database.query_history(&query).await?;

        if res.is_empty() {
            Ok(None)
        } else {
            self.last_id = Some(res.last().unwrap().id.0.clone());
            Ok(Some(res))
        }
    }
}

trait SqlBuilderExt {
    fn fuzzy_condition<S: ToString, T: ToString>(
        &mut self,
        field: S,
        mask: T,
        inverse: bool,
        glob: bool,
        is_or: bool,
    ) -> &mut Self;
}

impl SqlBuilderExt for SqlBuilder {
    /// adapted from the sql-builder *like functions
    fn fuzzy_condition<S: ToString, T: ToString>(
        &mut self,
        field: S,
        mask: T,
        inverse: bool,
        glob: bool,
        is_or: bool,
    ) -> &mut Self {
        let mut cond = field.to_string();
        if inverse {
            cond.push_str(" NOT");
        }
        if glob {
            cond.push_str(" GLOB '");
        } else {
            cond.push_str(" LIKE '");
        }
        cond.push_str(&esc(mask.to_string()));
        cond.push('\'');
        if is_or {
            self.or_where(cond)
        } else {
            self.and_where(cond)
        }
    }
}

pub struct QueryTokenizer<'a> {
    query: &'a str,
    last_pos: usize,
}

pub enum QueryToken<'a> {
    Match(&'a str, bool),
    MatchStart(&'a str, bool),
    MatchEnd(&'a str, bool),
    MatchFull(&'a str, bool),
    Or,
    Regex(&'a str),
}

impl QueryToken<'_> {
    pub fn has_uppercase(&self) -> bool {
        match self {
            Self::Match(term, _)
            | Self::MatchStart(term, _)
            | Self::MatchEnd(term, _)
            | Self::MatchFull(term, _) => term.contains(char::is_uppercase),
            _ => false,
        }
    }

    #[must_use]
    pub fn is_inverse(&self) -> bool {
        match self {
            Self::Match(_, inv)
            | Self::MatchStart(_, inv)
            | Self::MatchEnd(_, inv)
            | Self::MatchFull(_, inv) => *inv,
            _ => false,
        }
    }
}

impl<'a> QueryTokenizer<'a> {
    #[must_use]
    pub fn new(query: &'a str) -> Self {
        Self { query, last_pos: 0 }
    }
}

impl<'a> Iterator for QueryTokenizer<'a> {
    type Item = QueryToken<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let remaining = &self.query[self.last_pos..];
        if remaining.is_empty() {
            return None;
        }

        if let Some(remaining) = remaining.strip_prefix("r/") {
            let (regex, next_pos) = if let Some(end) = remaining.find("/ ") {
                (&remaining[..end], self.last_pos + 2 + end + 2)
            } else if let Some(remaining) = remaining.strip_suffix('/') {
                (remaining, self.query.len())
            } else {
                (remaining, self.query.len())
            };
            self.last_pos = next_pos;
            Some(QueryToken::Regex(regex))
        } else {
            let (mut part, next_pos) = if let Some(sp) = remaining.find(' ') {
                (&remaining[..sp], self.last_pos + sp + 1)
            } else {
                (remaining, self.query.len())
            };
            self.last_pos = next_pos;

            if part == "|" {
                return Some(QueryToken::Or);
            }

            let is_inverse = if let Some(s) = part.strip_prefix('!') {
                part = s;
                true
            } else {
                false
            };
            let token = if let Some(s) = part.strip_prefix('^') {
                QueryToken::MatchStart(s, is_inverse)
            } else if let Some(s) = part.strip_suffix('$') {
                QueryToken::MatchEnd(s, is_inverse)
            } else if let Some(s) = part.strip_prefix('\'') {
                QueryToken::MatchFull(s, is_inverse)
            } else {
                QueryToken::Match(part, is_inverse)
            };
            Some(token)
        }
    }
}

#[cfg(test)]
mod test {
    use std::time::{Duration, Instant};

    use rstest::{fixture, rstest};
    use time::format_description::well_known::Rfc3339;

    use super::*;
    use crate::settings::test_local_timeout;

    /// `ATUIN_SESSION` comes from the environment: a stray value whose version nibble reads as a
    /// timestamped UUID can carry a timestamp far outside the unix-nanos range, which must mean
    /// "no start time" rather than an overflow.
    #[rstest]
    #[case::a_real_v7_session(atuin_common::utils::uuid_v7().to_string(), true)]
    #[case::not_a_uuid("not-a-uuid".to_string(), false)]
    #[case::out_of_range_timestamp("ffffffff-ffff-1fff-bfff-ffffffffffff".to_string(), false)]
    fn session_start_time_tolerates_hostile_session_ids(
        #[case] session_id: String,
        #[case] has_start: bool,
    ) {
        assert_eq!(get_session_start_time(&session_id).is_some(), has_start);
    }

    fn new_context() -> Context {
        Context {
            cmd_origin: CmdOrigin::try_from("test:host").unwrap(),
            session: "beepboopiamasession".to_string(),
            cwd: "/home/ellie".to_string(),
            host_id: "test-host".to_string(),
            git_root: None,
        }
    }

    #[fixture]
    async fn empty_db() -> Sqlite {
        Sqlite::in_memory(test_local_timeout()).await.unwrap()
    }

    async fn assert_search_eq(
        db: &Sqlite,
        mode: DbSearchMode,
        filter_mode: FilterMode,
        query: &str,
        expected: usize,
    ) -> Result<Vec<History>> {
        let context = new_context();

        let results = db.search(mode, filter_mode, &context, query, Default::default()).await?;

        assert_eq!(
            results.len(),
            expected,
            "query \"{}\", commands: {:?}",
            query,
            results.iter().map(|a| &a.command).collect::<Vec<&String>>()
        );
        Ok(results)
    }

    async fn assert_search_commands(
        db: &Sqlite,
        mode: DbSearchMode,
        filter_mode: FilterMode,
        query: &str,
        expected_commands: Vec<&str>,
    ) {
        let results =
            assert_search_eq(db, mode, filter_mode, query, expected_commands.len()).await.unwrap();
        let commands: Vec<&str> = results.iter().map(|a| a.command.as_str()).collect();
        assert_eq!(commands, expected_commands);
    }

    async fn new_history_item(db: &Sqlite, cmd: &str) -> Result<()> {
        new_history_item_at(db, cmd, None).await
    }

    async fn new_history_item_at(
        db: &Sqlite,
        cmd: &str,
        timestamp: Option<OffsetDateTime>,
    ) -> Result<()> {
        let mut captured: History = History::capture()
            .timestamp(timestamp.unwrap_or_else(OffsetDateTime::now_utc))
            .command(cmd)
            .cwd("/home/ellie")
            .build()
            .into();

        captured.exit = 0;
        captured.duration = 1;
        captured.session = "beep boop".to_string();
        #[allow(deprecated)]
        {
            captured.cmd_origin = CmdOrigin::parse_lenient("booop");
        }

        db.save(&captured).await
    }

    async fn save_history_item(db: &Sqlite, cmd: &str) -> History {
        let mut captured: History = History::capture()
            .timestamp(OffsetDateTime::now_utc())
            .command(cmd)
            .cwd("/home/ellie")
            .build()
            .into();

        captured.exit = 0;
        captured.duration = 1;
        captured.session = "beep boop".to_string();
        #[allow(deprecated)]
        {
            captured.cmd_origin = CmdOrigin::parse_lenient("booop");
        }

        db.save(&captured).await.unwrap();
        captured
    }

    // `stats --filter-mode` scopes over a period by handing `list` an inclusive
    // `(from, to)` range. The bounds must be inclusive on both ends so a command
    // recorded exactly on a period boundary (e.g. at midnight) is still counted.
    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_list_range_is_inclusive(
        #[future(awt)]
        #[from(empty_db)]
        db: Sqlite,
    ) {
        let context = Context {
            #[allow(deprecated)]
            cmd_origin: CmdOrigin::parse_lenient("booop"),
            session: "beep boop".to_string(),
            cwd: "/home/ellie".to_string(),
            host_id: "test-host".to_string(),
            git_root: None,
        };

        // One item at a fixed instant, one at `now` (far outside the window below).
        let at = OffsetDateTime::from_unix_timestamp(1_708_330_400).unwrap();
        let mut past: History = History::capture()
            .timestamp(at)
            .command("ls /home/ellie")
            .cwd("/home/ellie")
            .build()
            .into();
        past.session = "beep boop".to_string();
        #[allow(deprecated)]
        {
            past.cmd_origin = CmdOrigin::parse_lenient("booop");
        }
        db.save(&past).await.unwrap();
        save_history_item(&db, "ls /home/frank").await;

        // No range -> everything.
        let all = db.list([], &context, None, false, false, None).await.unwrap();
        assert_eq!(all.len(), 2);

        // A zero-width window on the item's exact timestamp matches it, because the
        // bounds are inclusive (`timestamp >= from AND timestamp <= to`).
        let hits = db.list([], &context, None, false, false, Some((at, at))).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "ls /home/ellie");
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_load_active_returns_only_requested_rows(
        #[future(awt)]
        #[from(empty_db)]
        db: Sqlite,
    ) {
        let alpha = save_history_item(&db, "echo alpha").await;
        let bravo = save_history_item(&db, "echo bravo").await;
        let _charlie = save_history_item(&db, "echo charlie").await;

        let loaded = db.load_active([alpha.id.clone(), bravo.id.clone()]).await.unwrap();

        let mut commands: Vec<String> = loaded.into_iter().map(|h| h.command).collect();
        commands.sort();
        assert_eq!(commands, vec!["echo alpha", "echo bravo"]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_load_active_empty_never_reaches_sqlite(
        #[future(awt)]
        #[from(empty_db)]
        db: Sqlite,
    ) {
        save_history_item(&db, "echo alpha").await;

        // `select ... where id in ()` is a syntax error, so the empty case must
        // short-circuit rather than build a query.
        let loaded = db.load_active(std::iter::empty::<HistoryId>()).await.unwrap();

        assert!(loaded.is_empty());
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_load_active_skips_soft_deleted(
        #[future(awt)]
        #[from(empty_db)]
        db: Sqlite,
    ) {
        let mut alpha = save_history_item(&db, "echo alpha").await;
        let bravo = save_history_item(&db, "echo bravo").await;

        alpha.deleted_at = Some(OffsetDateTime::now_utc());
        alpha.command = String::new();
        db.update(&alpha).await.unwrap();

        let loaded = db.load_active([alpha.id.clone(), bravo.id.clone()]).await.unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].command, "echo bravo");
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_load_active_missing_ids_are_omitted(
        #[future(awt)]
        #[from(empty_db)]
        db: Sqlite,
    ) {
        let alpha = save_history_item(&db, "echo alpha").await;

        let loaded = db
            .load_active([alpha.id.clone(), HistoryId("does-not-exist".to_string())])
            .await
            .unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].command, "echo alpha");
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_load_active_chunks_past_sqlite_param_limit(
        #[future(awt)]
        #[from(empty_db)]
        db: Sqlite,
    ) {
        // Comfortably over SQLITE_MAX_VARIABLE_NUMBER's 999 floor: a single
        // `in (...)` with one placeholder per id would fail here.
        let mut ids = Vec::new();
        for i in 0..1200 {
            ids.push(save_history_item(&db, &format!("echo {i}")).await.id);
        }

        let loaded = db.load_active(ids).await.unwrap();

        assert_eq!(loaded.len(), 1200);
    }

    async fn db_with(commands: &[&str]) -> Sqlite {
        let db = Sqlite::in_memory(test_local_timeout()).await.unwrap();

        for command in commands {
            new_history_item(&db, command).await.unwrap();
        }

        db
    }

    #[rstest]
    #[case::window_spans_the_item(Some((-1, 1)), 1, true)]
    #[case::after_bound_is_exclusive(Some((0, 1)), 0, false)]
    #[case::before_bound_is_exclusive(Some((-1, 0)), 0, false)]
    #[case::window_entirely_after_the_item(Some((1, 2)), 0, false)]
    #[case::window_entirely_before_the_item(Some((-2, -1)), 0, false)]
    #[case::no_date_filter(None, 2, false)]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_before_after(
        #[case] offsets: Option<(i64, i64)>,
        #[case] expected: usize,
        #[case] expect_ellie_match: bool,
    ) {
        let t = OffsetDateTime::from_unix_timestamp(1708330400).unwrap();

        let db = Sqlite::in_memory(test_local_timeout()).await.unwrap();
        new_history_item_at(&db, "ls /home/ellie", Some(t)).await.unwrap();
        new_history_item_at(&db, "ls /home/frank", None).await.unwrap();

        let context = new_context();

        let stamp = |seconds: i64| (t + time::Duration::seconds(seconds)).format(&Rfc3339).unwrap();
        let (after, before) = match offsets {
            Some((after, before)) => (Some(stamp(after)), Some(stamp(before))),
            None => (None, None),
        };

        let results = db
            .search(DbSearchMode::FullText, FilterMode::Global, &context, "", OptFilters {
                after: after.as_deref(),
                before: before.as_deref(),
                include_duplicates: true,
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(results.len(), expected);
        if expect_ellie_match {
            assert_eq!(results[0].command, "ls /home/ellie");
        }
    }

    #[rstest]
    #[case::with_duplicates_counts_every_execution(true, 2)]
    #[case::without_duplicates_collapses_to_newest_row(false, 1)]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_include_duplicates(
        #[case] include_duplicates: bool,
        #[case] expected: usize,
    ) {
        // The same command, run twice.
        let db = db_with(&["ls", "ls"]).await;
        let context = new_context();

        let hits = db
            .search(DbSearchMode::FullText, FilterMode::Global, &context, "", OptFilters {
                include_duplicates,
                ..Default::default()
            })
            .await
            .unwrap();

        assert_eq!(hits.len(), expected);
    }

    #[rstest]
    #[case::before("before")]
    #[case::after("after")]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_rejects_unparsable_date_filter(#[case] which: &str) {
        let db = db_with(&["ls"]).await;
        let context = new_context();

        let mut filters = OptFilters::default();
        match which {
            "before" => filters.before = Some("not a date"),
            "after" => filters.after = Some("not a date"),
            _ => unreachable!(),
        }

        let result =
            db.search(DbSearchMode::FullText, FilterMode::Global, &context, "", filters).await;

        assert!(result.is_err(), "unparsable `{which}` filter must error");
    }

    #[rstest]
    #[case::matches_prefix("ls", 1)]
    #[case::not_a_prefix("/home", 0)]
    #[case::trailing_whitespace("ls  ", 0)]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_prefix(#[case] query: &str, #[case] expected: usize) {
        let db = db_with(&["ls /home/ellie"]).await;

        assert_search_eq(&db, DbSearchMode::Prefix, FilterMode::Global, query, expected)
            .await
            .unwrap();
    }

    #[rstest]
    #[case::matches_command("ls", 1)]
    #[case::matches_arg("/home", 1)]
    #[case::matches_multiple_words("ls ho", 1)]
    #[case::no_match("hm", 0)]
    // regex
    #[case::regex_anchored_start("r/^ls ", 1)]
    #[case::regex_anchored_end("r/ls / ie$", 1)]
    #[case::regex_negated_no_match("r/ls / !ie", 0)]
    #[case::regex_mixed_with_plain_term("meow r/ls/", 0)]
    #[case::regex_single_slash("r//hom/", 1)]
    #[case::regex_double_slash("r//home//", 1)]
    #[case::regex_triple_slash("r//home///", 0)]
    #[case::plain_query_looks_like_regex("/home.*e", 0)]
    #[case::regex_wildcard("r/home.*e", 1)]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_fulltext(#[case] query: &str, #[case] expected: usize) {
        let db = db_with(&["ls /home/ellie"]).await;

        assert_search_eq(&db, DbSearchMode::FullText, FilterMode::Global, query, expected)
            .await
            .unwrap();
    }

    #[rstest]
    #[case::term_with_trailing_slash("ls /", 3)]
    #[case::adjacent_terms_no_space("ls/", 2)]
    #[case::short_terms("l/h/", 2)]
    #[case::partial_match("/h/e", 3)]
    #[case::typo_no_match("/hmoe/", 0)]
    #[case::wrong_order_no_match("ellie/home", 0)]
    #[case::concatenated_terms("lsellie", 1)]
    #[case::bare_space_matches_all(" ", 4)]
    #[case::starts_with("^ls", 2)]
    #[case::exact_word("'ls", 2)]
    #[case::ends_with("ellie$", 2)]
    #[case::negated_starts_with("!^ls", 2)]
    #[case::negated_term("!ellie", 1)]
    #[case::negated_ends_with("!ellie$", 2)]
    #[case::term_and_negated_term("ls !ellie", 1)]
    #[case::starts_with_and_negated_ends_with("^ls !e$", 1)]
    #[case::term_and_negated_starts_with("home !^ls", 2)]
    #[case::or_exact_terms("'frank | 'rustup", 2)]
    #[case::or_with_and_term("'frank | 'rustup 'ls", 1)]
    #[case::case_insensitive_match("Ellie", 1)]
    #[case::regex_anchored_start("r/^ls ", 2)]
    #[case::regex_character_class("r/[Ee]llie", 3)]
    #[case::regex_combined_with_fuzzy_term("/h/e r/^ls ", 1)]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_fuzzy(#[case] query: &str, #[case] expected: usize) {
        let db = db_with(&[
            "ls /home/ellie",
            "ls /home/frank",
            "cd /home/Ellie",
            "/home/ellie/.bin/rustup",
        ])
        .await;

        assert_search_eq(&db, DbSearchMode::Fuzzy, FilterMode::Global, query, expected)
            .await
            .unwrap();
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_reordered_fuzzy(
        #[future(awt)]
        #[from(empty_db)]
        db: Sqlite,
    ) {
        // test ordering of results: we should choose the first, even though it happened longer ago.

        new_history_item(&db, "curl").await.unwrap();
        new_history_item(&db, "corburl").await.unwrap();

        // if fuzzy reordering is on, it should come back in a more sensible order
        assert_search_commands(&db, DbSearchMode::Fuzzy, FilterMode::Global, "curl", vec![
            "curl", "corburl",
        ])
        .await;

        assert_search_eq(&db, DbSearchMode::Fuzzy, FilterMode::Global, "xxxx", 0).await.unwrap();
        assert_search_eq(&db, DbSearchMode::Fuzzy, FilterMode::Global, "", 2).await.unwrap();
    }

    #[rstest]
    // The three modes the database implements map exactly.
    #[case::prefix(SearchMode::Prefix, DbSearchMode::Prefix)]
    #[case::full_text(SearchMode::FullText, DbSearchMode::FullText)]
    #[case::fuzzy(SearchMode::Fuzzy, DbSearchMode::Fuzzy)]
    // `DaemonFuzzy` never reaches the database in the interactive path: `engines::engine` routes it
    // to the daemon index. When it does arrive via `atuin search --search-mode daemon-fuzzy`, the
    // closest database behaviour is a plain fuzzy query. See issue #3670.
    #[case::daemon_fuzzy_degrades_to_fuzzy(SearchMode::DaemonFuzzy, DbSearchMode::Fuzzy)]
    fn closest_db_mode_maps_every_search_mode(
        #[case] mode: SearchMode,
        #[case] expected: DbSearchMode,
    ) {
        assert_eq!(mode.closest_db_mode(), expected);
    }

    #[rstest]
    #[case::prefix(DbSearchMode::Prefix)]
    #[case::full_text(DbSearchMode::FullText)]
    #[case::fuzzy(DbSearchMode::Fuzzy)]
    fn db_search_mode_round_trips_through_search_mode(#[case] mode: DbSearchMode) {
        assert_eq!(SearchMode::from(mode).closest_db_mode(), mode);
    }

    /// Issue #3670: `atuin search --search-mode daemon-fuzzy` reached the database as an
    /// unrecognised mode. It took the fuzzy SQL path but skipped the fuzzy relevance reordering, so
    /// results came back in raw timestamp order while plain `--search-mode fuzzy` ranked them by
    /// minimum matching span. `daemon-fuzzy` must behave exactly like `fuzzy` once it reaches the
    /// database.
    #[rstest]
    #[case::daemon_fuzzy(SearchMode::DaemonFuzzy)]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_interactive_only_modes_rank_like_fuzzy(#[case] mode: SearchMode) {
        let db = Sqlite::in_memory(test_local_timeout()).await.unwrap();

        // "corburl" is strictly newer, so an unranked query would return it first and the assertion
        // below would fail.
        let now = OffsetDateTime::now_utc();
        new_history_item_at(&db, "curl", Some(now - time::Duration::seconds(10))).await.unwrap();
        new_history_item_at(&db, "corburl", Some(now)).await.unwrap();

        assert_search_commands(&db, mode.closest_db_mode(), FilterMode::Global, "curl", vec![
            "curl", "corburl",
        ])
        .await;
    }

    // Reproduces the trailing-space ranking bug (atuinsh/atuin#3603): "screen" ranked the results
    // containing `screen` first, but "screen " prioritized an unrelated `ls` command.
    #[rstest]
    #[case::no_trailing_space("screen")]
    #[case::trailing_space("screen ")]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_fuzzy_trailing_space(#[case] query: &str) {
        let db = Sqlite::in_memory(test_local_timeout()).await.unwrap();

        let now = OffsetDateTime::now_utc();
        let irssi = "screen irssi";
        let ls_l = "ls -l secrets/rendered";
        let ls_ld = "ls -ld secrets/rendered";
        let screen_r = "screen -r";

        new_history_item_at(&db, irssi, Some(now - time::Duration::days(5))).await.unwrap();
        new_history_item_at(&db, ls_l, Some(now - time::Duration::days(4))).await.unwrap();
        new_history_item_at(
            &db,
            ls_ld,
            Some(now - time::Duration::days(4) + time::Duration::seconds(1)),
        )
        .await
        .unwrap();
        new_history_item_at(&db, screen_r, Some(now - time::Duration::hours(1))).await.unwrap();

        let results =
            assert_search_eq(&db, DbSearchMode::Fuzzy, FilterMode::Global, query, 4).await.unwrap();
        assert_eq!(
            results[0].command,
            screen_r,
            "\"{query}\" should rank the screen command first, got: {:?}",
            results.iter().map(|h| &h.command).collect::<Vec<_>>()
        );
    }

    // Make sure fuzzy search prioritizes results that contain the query as a contiguous substring,
    // but ignoring query operators, inverse terms, and whitespace. Each test case has a "close"
    // result that should rank higher by fuzzy score but is less recent, and a "far" result that is
    // more recent.
    #[rstest]
    #[case::plain_single_term("screen", "screen", "search green")]
    #[case::plain_two_terms("foo bar", "foo bar", "foo qux bar")]
    #[case::trailing_space("screen ", "screen", "search green")]
    #[case::extra_middle_space("foo   bar", "foo bar", "foo qux bar")]
    #[case::end_anchor("foo screen$", "foo screen", "foo x screen")]
    #[case::negated_term("foo screen !zzz", "foo screen", "foo x screen")]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_fuzzy_prioritizes_contiguous_match(
        #[case] query: &str,
        #[case] close: &str,
        #[case] far: &str,
    ) {
        let db = Sqlite::in_memory(test_local_timeout()).await.unwrap();

        let now = OffsetDateTime::now_utc();
        new_history_item_at(&db, close, Some(now - time::Duration::days(5))).await.unwrap();
        new_history_item_at(&db, far, Some(now - time::Duration::hours(1))).await.unwrap();

        assert_search_commands(&db, DbSearchMode::Fuzzy, FilterMode::Global, query, vec![
            close, far,
        ])
        .await;
    }

    // SQL operators are stripped when performing fuzzy reordering, but this must not affect the
    // initial SQL matching.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_fuzzy_operator() {
        let db = db_with(&["use screen", "screenshot tool"]).await;

        assert_search_commands(&db, DbSearchMode::Fuzzy, FilterMode::Global, "screen$", vec![
            "use screen",
        ])
        .await;
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_paged_basic(
        #[future(awt)]
        #[from(empty_db)]
        db: Sqlite,
    ) {
        // Add 5 history items
        for i in 0..5 {
            new_history_item(&db, &format!("command{i}")).await.unwrap();
        }

        // Create a paged iterator with page_size of 2
        let mut paged = db.all_paged(2, false, false);

        // First page should have 2 items
        let page1 = paged.next().await.unwrap();
        assert!(page1.is_some());
        assert_eq!(page1.unwrap().len(), 2);

        // Second page should have 2 items
        let page2 = paged.next().await.unwrap();
        assert!(page2.is_some());
        assert_eq!(page2.unwrap().len(), 2);

        // Third page should have 1 item
        let page3 = paged.next().await.unwrap();
        assert!(page3.is_some());
        assert_eq!(page3.unwrap().len(), 1);

        // Fourth page should be None (exhausted)
        let page4 = paged.next().await.unwrap();
        assert!(page4.is_none());
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_paged_empty(
        #[future(awt)]
        #[from(empty_db)]
        db: Sqlite,
    ) {
        // Create a paged iterator on empty database
        let mut paged = db.all_paged(10, false, false);

        // Should return None immediately
        let page = paged.next().await.unwrap();
        assert!(page.is_none());
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_paged_unique(
        #[future(awt)]
        #[from(empty_db)]
        db: Sqlite,
    ) {
        // Add duplicate commands
        new_history_item(&db, "duplicate").await.unwrap();
        new_history_item(&db, "duplicate").await.unwrap();
        new_history_item(&db, "unique1").await.unwrap();
        new_history_item(&db, "unique2").await.unwrap();

        // Without unique flag - should get all 4
        let mut paged = db.all_paged(10, false, false);
        let page = paged.next().await.unwrap().unwrap();
        assert_eq!(page.len(), 4);

        // With unique flag - should get 3 (duplicates collapsed)
        let mut paged_unique = db.all_paged(10, false, true);
        let page_unique = paged_unique.next().await.unwrap().unwrap();
        assert_eq!(page_unique.len(), 3);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_paged_include_deleted(
        #[future(awt)]
        #[from(empty_db)]
        db: Sqlite,
    ) {
        // Add items
        new_history_item(&db, "keep1").await.unwrap();
        new_history_item(&db, "keep2").await.unwrap();
        new_history_item(&db, "delete_me").await.unwrap();

        // Delete one item
        let all = db
            .list(
                [],
                &Context {
                    #[allow(deprecated)]
                    cmd_origin: CmdOrigin::parse_lenient(""),
                    session: "".to_string(),
                    cwd: "".to_string(),
                    host_id: "".to_string(),
                    git_root: None,
                },
                None,
                false,
                false,
                None,
            )
            .await
            .unwrap();

        let to_delete = all.iter().find(|h| h.command == "delete_me").unwrap().clone();
        db.delete(to_delete).await.unwrap();

        // Deletes remove the row outright, so both views should get 2
        let mut paged = db.all_paged(10, false, false);
        let page = paged.next().await.unwrap().unwrap();
        assert_eq!(page.len(), 2);

        let mut paged_deleted = db.all_paged(10, true, false);
        let page_deleted = paged_deleted.next().await.unwrap().unwrap();
        assert_eq!(page_deleted.len(), 2);

        // Tombstones written by older versions are still filtered by include_deleted
        let mut legacy = all.iter().find(|h| h.command == "keep1").unwrap().clone();
        legacy.deleted_at = Some(OffsetDateTime::now_utc());
        db.update(&legacy).await.unwrap();

        let mut paged = db.all_paged(10, false, false);
        let page = paged.next().await.unwrap().unwrap();
        assert_eq!(page.len(), 1);

        let mut paged_deleted = db.all_paged(10, true, false);
        let page_deleted = paged_deleted.next().await.unwrap().unwrap();
        assert_eq!(page_deleted.len(), 2);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_bench_dupes(
        #[future(awt)]
        #[from(empty_db)]
        db: Sqlite,
    ) {
        let context = new_context();

        for _i in 1..10000 {
            new_history_item(&db, "i am a duplicated command").await.unwrap();
        }
        let start = Instant::now();
        let _results = db
            .search(DbSearchMode::Fuzzy, FilterMode::Global, &context, "", Default::default())
            .await
            .unwrap();
        let duration = start.elapsed();

        assert!(duration < Duration::from_secs(15));
    }

    #[tokio::test(flavor = "multi_thread")]
    #[rstest]
    #[case::all([], 7)]
    #[case::bash(["bash"], 1)]
    #[case::bash_unknown(["bash", ""], 5)]
    #[case::bash_zsh(["bash", "zsh"], 3)]
    #[case::unknown([""], 4)]
    #[case::fish(["fish"], 0)]
    #[case::fish_unknown(["fish", ""], 4)]
    async fn test_search_shells<const N: usize>(
        #[case] shells: [&str; N],
        #[case] expected_count: usize,
    ) {
        let db = Sqlite::in_memory(test_local_timeout()).await.unwrap();

        for (command, shell) in [
            ("echo unknown1", None),
            ("echo zsh1", Some("zsh")),
            ("echo unknown2", None),
            ("echo bash", Some("bash")),
            ("echo unknown3", None),
            ("echo unknown4", None),
            ("echo zsh2", Some("zsh")),
        ] {
            let history = History::capture()
                .timestamp(OffsetDateTime::now_utc())
                .command(command)
                .cwd("/tmp")
                .shell_opt(shell.map(str::to_owned))
                .build()
                .into();
            db.save(&history).await.unwrap();
        }

        let context = Context {
            #[allow(deprecated)]
            cmd_origin: CmdOrigin::parse_lenient("hostname"),
            session: "session".into(),
            cwd: "/tmp".into(),
            host_id: "host".into(),
            git_root: None,
        };

        let shells = OrFilter::from_list(shells.map(str::to_owned).to_vec()).unwrap_or_default();
        let filters = OptFilters {
            shells: shells.as_slice_filter(),
            ..Default::default()
        };

        let results = db
            .search(DbSearchMode::FullText, FilterMode::Global, &context, "echo", filters)
            .await
            .unwrap();

        assert_eq!(results.len(), expected_count, "{results:?}");
    }

    /// An author_kind value this version doesn't recognise (written by a newer one) must fall
    /// through to the name heuristic in SQL, exactly like [`AuthorKind::from_repr`] returning `None`
    /// does in [`History::is_agent`] — otherwise the two classifiers disagree on the same row.
    #[tokio::test(flavor = "multi_thread")]
    #[rstest]
    async fn author_filter_treats_an_unknown_kind_as_unstated() {
        let db = Sqlite::in_memory(test_local_timeout()).await.unwrap();

        let history: History = History::import()
            .timestamp(OffsetDateTime::now_utc())
            .command("echo hello")
            .cwd("/tmp")
            .cmd_origin(CmdOrigin::try_from("mac:ellie".to_owned()).unwrap())
            .author("claude-code")
            .build()
            .into();
        db.save(&history).await.unwrap();

        // A kind from the future: one past the largest value any AuthorKind variant maps to, so
        // it stays unknown even if more variants are added.
        let unknown = AuthorKind::VARIANTS.iter().map(|kind| kind.as_u8()).max().unwrap() + 1;
        sqlx::query("update history set author_kind = ?1")
            .bind(i64::from(unknown))
            .execute(db.sqlite.pool())
            .await
            .unwrap();

        let context = Context {
            cmd_origin: CmdOrigin::try_from("mac:ellie".to_owned()).unwrap(),
            session: "session".into(),
            cwd: "/tmp".into(),
            host_id: "host".into(),
            git_root: None,
        };

        for (pattern, expected) in [(AuthorPattern::AllAgent, 1), (AuthorPattern::AllUser, 0)] {
            let authors = OrFilter::from_list(vec![pattern]).unwrap();
            let filters = OptFilters {
                authors: authors.as_slice_filter(),
                ..Default::default()
            };
            let results = db
                .search(DbSearchMode::FullText, FilterMode::Global, &context, "echo", filters)
                .await
                .unwrap();
            assert_eq!(results.len(), expected, "{authors:?}");
        }
    }

    /// A legacy row whose colonless hostname IS a known agent name (e.g. a machine hostnamed
    /// `pi`, imported before hostnames were `host:user`): the author defaulted to the whole
    /// hostname, so it tells us nothing — human on both the SQL and [`History::is_agent`] sides.
    #[tokio::test(flavor = "multi_thread")]
    #[rstest]
    async fn author_filter_treats_a_colonless_agent_hostname_as_human() {
        let db = Sqlite::in_memory(test_local_timeout()).await.unwrap();

        let history: History = History::import()
            .timestamp(OffsetDateTime::now_utc())
            .command("echo hello")
            .cwd("/tmp")
            .author("pi")
            .build()
            .into();
        db.save(&history).await.unwrap();
        // `CmdOrigin` cannot represent a colonless hostname (parse_lenient rewrites it), so plant
        // the legacy shape directly.
        sqlx::query("update history set hostname = 'pi'").execute(db.sqlite.pool()).await.unwrap();

        let loaded = db.load(history.id.0.as_str()).await.unwrap().unwrap();
        assert!(!loaded.is_agent());

        let context = Context {
            cmd_origin: CmdOrigin::try_from("pi:unknown-user".to_owned()).unwrap(),
            session: "session".into(),
            cwd: "/tmp".into(),
            host_id: "host".into(),
            git_root: None,
        };
        for (pattern, expected) in [(AuthorPattern::AllUser, 1), (AuthorPattern::AllAgent, 0)] {
            let authors = OrFilter::from_list(vec![pattern]).unwrap();
            let filters = OptFilters {
                authors: authors.as_slice_filter(),
                ..Default::default()
            };
            let results = db
                .search(DbSearchMode::FullText, FilterMode::Global, &context, "echo", filters)
                .await
                .unwrap();
            assert_eq!(results.len(), expected, "{authors:?}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[rstest]
    async fn author_filter_treats_a_placeholder_user_agent_hostname_as_human() {
        let db = Sqlite::in_memory(test_local_timeout()).await.unwrap();

        let history: History = History::import()
            .timestamp(OffsetDateTime::now_utc())
            .command("echo hello")
            .cwd("/tmp")
            .cmd_origin(CmdOrigin::try_from("pi:unknown-user".to_owned()).unwrap())
            .author("pi")
            .build()
            .into();
        db.save(&history).await.unwrap();

        let loaded = db.load(history.id.0.as_str()).await.unwrap().unwrap();
        assert!(!loaded.is_agent());

        let context = Context {
            cmd_origin: CmdOrigin::try_from("pi:unknown-user".to_owned()).unwrap(),
            session: "session".into(),
            cwd: "/tmp".into(),
            host_id: "host".into(),
            git_root: None,
        };
        for (pattern, expected) in [(AuthorPattern::AllUser, 1), (AuthorPattern::AllAgent, 0)] {
            let authors = OrFilter::from_list(vec![pattern]).unwrap();
            let filters = OptFilters {
                authors: authors.as_slice_filter(),
                ..Default::default()
            };
            let results = db
                .search(DbSearchMode::FullText, FilterMode::Global, &context, "echo", filters)
                .await
                .unwrap();
            assert_eq!(results.len(), expected, "{authors:?}");
        }
    }

    /// A user called `pi` shares a name with the `pi` agent, so on their machine the author name
    /// alone cannot say who ran a command: only an entry that states its kind is an agent's.
    #[tokio::test(flavor = "multi_thread")]
    #[rstest]
    #[case::all_user(["$all-user"], &["echo pi-human"])]
    #[case::all_agent(["$all-agent"], &["echo pi-agent", "echo claude"])]
    #[case::by_name(["pi"], &["echo pi-agent", "echo pi-human"])]
    async fn test_search_authors_when_the_user_is_named_after_an_agent<const N: usize>(
        #[case] authors: [&str; N],
        #[case] expected: &[&str],
    ) {
        let db = Sqlite::in_memory(test_local_timeout()).await.unwrap();

        for (command, author, author_kind) in [
            ("echo pi-agent", "pi", Some(AuthorKind::Agent)),
            ("echo pi-human", "pi", None),
            ("echo claude", "claude-code", None),
        ] {
            let history = History::import()
                .timestamp(OffsetDateTime::now_utc())
                .command(command)
                .cwd("/tmp")
                .cmd_origin(CmdOrigin::try_from("raspberry:pi".to_owned()).unwrap())
                .author(author)
                .author_kind(author_kind)
                .build()
                .into();
            db.save(&history).await.unwrap();
        }

        let context = Context {
            cmd_origin: CmdOrigin::try_from("raspberry:pi".to_owned()).unwrap(),
            session: "session".into(),
            cwd: "/tmp".into(),
            host_id: "host".into(),
            git_root: None,
        };

        let authors =
            OrFilter::from_list(authors.map(AuthorPattern::from).to_vec()).unwrap_or_default();
        let filters = OptFilters {
            authors: authors.as_slice_filter(),
            ..Default::default()
        };

        let results = db
            .search(DbSearchMode::FullText, FilterMode::Global, &context, "echo", filters)
            .await
            .unwrap();

        let mut commands: Vec<&str> = results.iter().map(|h| h.command.as_str()).collect();
        commands.sort_unstable();
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        assert_eq!(commands, expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[rstest]
    #[case::all([], 4)]
    #[case::all_user(["$all-user"], 2)]
    #[case::all_agent(["$all-agent"], 2)]
    #[case::claude_code(["claude-code"], 1)]
    #[case::claude_code_or_codex(["claude-code", "codex"], 2)]
    #[case::unknown_author(["nobody"], 0)]
    async fn test_search_authors<const N: usize>(
        #[case] authors: [&str; N],
        #[case] expected_count: usize,
    ) {
        let db = Sqlite::in_memory(test_local_timeout()).await.unwrap();

        for (command, author) in [
            ("echo alice1", "alice"),
            ("echo claude1", "claude-code"),
            ("echo codex1", "codex"),
            ("echo bob1", "bob"),
        ] {
            let history = History::capture()
                .timestamp(OffsetDateTime::now_utc())
                .command(command)
                .cwd("/tmp")
                .author(author)
                .build()
                .into();
            db.save(&history).await.unwrap();
        }

        let context = Context {
            #[allow(deprecated)]
            cmd_origin: CmdOrigin::parse_lenient("hostname"),
            session: "session".into(),
            cwd: "/tmp".into(),
            host_id: "host".into(),
            git_root: None,
        };

        let authors =
            OrFilter::from_list(authors.map(AuthorPattern::from).to_vec()).unwrap_or_default();
        let filters = OptFilters {
            authors: authors.as_slice_filter(),
            ..Default::default()
        };

        let results = db
            .search(DbSearchMode::FullText, FilterMode::Global, &context, "echo", filters)
            .await
            .unwrap();

        assert_eq!(results.len(), expected_count, "{results:?}");
    }
}
