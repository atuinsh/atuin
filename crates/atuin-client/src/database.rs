use std::{
    env,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use crate::history::{AUTHOR_FILTER_ALL_AGENT, AUTHOR_FILTER_ALL_USER, KNOWN_AGENTS};
use async_trait::async_trait;
use atuin_common::utils;
use fs_err as fs;
use itertools::Itertools;
use rand::{Rng, distributions::Alphanumeric};
use sql_builder::{SqlBuilder, SqlName, bind::Bind, esc, quote};
use sqlx::{
    Result, Row,
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
        SqliteSynchronous,
    },
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    history::{HistoryId, HistoryStats},
    utils::get_host_user,
};

use super::{
    history::History,
    ordering,
    settings::{FilterMode, SearchMode, Settings},
};

#[derive(Clone)]
pub struct Context {
    pub session: String,
    pub cwd: String,
    pub hostname: String,
    pub host_id: String,
    pub git_root: Option<PathBuf>,
}

#[derive(Default, Clone)]
pub struct OptFilters {
    pub exit: Option<i64>,
    pub exclude_exit: Option<i64>,
    /// Only commands that recorded a non-zero exit. Unlike `exclude_exit: 0`,
    /// this also skips the `exit = -1` sentinel rows for commands still
    /// running (or whose end hook never fired).
    pub only_failed: bool,
    pub cwd: Option<String>,
    pub exclude_cwd: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub reverse: bool,
    pub include_duplicates: bool,
    /// Author filter. Supports special values `$all-user` and `$all-agent`.
    pub authors: Vec<String>,
    pub shells: Vec<String>,
}

/// Build a query [`Context`] without requiring a live shell session.
///
/// Outside of an atuin-hooked shell (e.g. when running as an MCP server),
/// `ATUIN_SESSION` is unset; the session is left empty so session-scoped
/// filters simply match nothing.
pub async fn query_context() -> eyre::Result<Context> {
    let session = env::var("ATUIN_SESSION").unwrap_or_default();
    let hostname = get_host_user();
    let cwd = utils::get_current_dir();
    let host_id = Settings::host_id().await?;
    let git_root = utils::in_git_repo(cwd.as_str());

    Ok(Context {
        session,
        hostname,
        cwd,
        git_root,
        host_id: host_id.0.as_simple().to_string(),
    })
}

pub async fn current_context() -> eyre::Result<Context> {
    if env::var("ATUIN_SESSION").is_err() {
        return Err(eyre::eyre!(
            "Failed to find $ATUIN_SESSION in the environment. Check that you have correctly set up your shell."
        ));
    }

    query_context().await
}

impl Context {
    pub fn from_history(entry: &History) -> Self {
        Context {
            session: entry.session.to_string(),
            cwd: entry.cwd.to_string(),
            hostname: entry.hostname.to_string(),
            host_id: String::new(),
            git_root: utils::in_git_repo(entry.cwd.as_str()),
        }
    }
}

/// Each entry is OR'd: `$all-user` → NOT IN agents, `$all-agent` → IN agents, literal → exact match.
fn apply_author_filter(sql: &mut SqlBuilder, authors: &[String]) {
    let mut conditions: Vec<String> = Vec::new();
    let agent_list: String = KNOWN_AGENTS.iter().map(quote).join(", ");
    let author_expr = "CASE \
        WHEN author IS NULL OR trim(author) = '' THEN \
            CASE \
                WHEN instr(hostname, ':') > 0 THEN substr(hostname, instr(hostname, ':') + 1) \
                ELSE hostname \
            END \
        ELSE author \
    END";

    for author in authors {
        match author.as_str() {
            AUTHOR_FILTER_ALL_USER => {
                conditions.push(format!("{author_expr} NOT IN ({agent_list})"));
            }
            AUTHOR_FILTER_ALL_AGENT => {
                conditions.push(format!("{author_expr} IN ({agent_list})"));
            }
            literal => {
                conditions.push(format!("{author_expr} = {}", quote(literal)));
            }
        }
    }

    if !conditions.is_empty() {
        sql.and_where(format!("({})", conditions.join(" OR ")));
    }
}

fn apply_shell_filter<S>(sql: &mut SqlBuilder, shells: S)
where
    S: IntoIterator,
    S::Item: AsRef<str>,
{
    let mut include_null = false;
    let nonempty_shells = shells.into_iter().filter(|s| {
        let is_empty = s.as_ref().is_empty();
        if is_empty {
            include_null = true;
        }
        !is_empty
    });

    let shell_list = nonempty_shells.map(|s| quote(s.as_ref())).join(", ");
    let mut cond = (!shell_list.is_empty()).then(|| format!("shell in ({shell_list})"));

    if include_null {
        cond = Some(cond.map_or_else(String::new, |s| s + " OR ") + "shell IS NULL");
    }
    if let Some(cond) = cond {
        sql.and_where(cond);
    }
}

fn get_session_start_time(session_id: &str) -> Option<i64> {
    if let Ok(uuid) = Uuid::parse_str(session_id)
        && let Some(timestamp) = uuid.get_timestamp()
    {
        let (seconds, nanos) = timestamp.to_unix();
        return Some(seconds as i64 * 1_000_000_000 + nanos as i64);
    }
    None
}

#[async_trait]
pub trait Database: Send + Sync + 'static {
    async fn save(&self, h: &History) -> Result<()>;
    async fn save_bulk(&self, h: &[History]) -> Result<()>;

    async fn load(&self, id: &str) -> Result<Option<History>>;

    /// Load the *active* (not soft-deleted) entries for the given IDs.
    ///
    /// Unlike [`load`](Self::load), this filters out soft-deleted rows -- an ID whose
    /// row exists but is deleted is omitted, exactly like an ID that isn't present at
    /// all. Ordering is unspecified. Prefer this over calling `load` in a loop: it
    /// chunks into a handful of queries rather than one round trip per ID.
    async fn load_active(&self, ids: &[HistoryId]) -> Result<Vec<History>>;
    async fn list(
        &self,
        filters: &[FilterMode],
        context: &Context,
        max: Option<usize>,
        unique: bool,
        include_deleted: bool,
        range: Option<(OffsetDateTime, OffsetDateTime)>,
    ) -> Result<Vec<History>>;
    async fn range(&self, from: OffsetDateTime, to: OffsetDateTime) -> Result<Vec<History>>;

    async fn update(&self, h: &History) -> Result<()>;
    async fn history_count(&self, include_deleted: bool) -> Result<i64>;

    async fn last(&self) -> Result<Option<History>>;
    async fn before(&self, timestamp: OffsetDateTime, count: i64) -> Result<Vec<History>>;

    async fn delete(&self, h: History) -> Result<()>;
    async fn delete_rows(&self, ids: &[HistoryId]) -> Result<()>;

    // Yes I know, it's a lot.
    // Could maybe break it down to a searchparams struct or smth but that feels a little... pointless.
    // Been debating maybe a DSL for search? eg "before:time limit:1 the query"
    #[allow(clippy::too_many_arguments)]
    async fn search(
        &self,
        search_mode: SearchMode,
        filter: FilterMode,
        context: &Context,
        query: &str,
        filter_options: OptFilters,
    ) -> Result<Vec<History>>;

    async fn query_history(&self, query: &str) -> Result<Vec<History>>;

    async fn all_with_count(&self) -> Result<Vec<(History, i32)>>;

    fn all_paged(&self, page_size: usize, include_deleted: bool, unique: bool) -> Paged;

    async fn stats(&self, h: &History) -> Result<HistoryStats>;

    async fn get_dups(&self, before: i64, dupkeep: u32) -> Result<Vec<History>>;

    fn clone_boxed(&self) -> Box<dyn Database + 'static>;
}

// Intended for use on a developer machine and not a sync server.
// TODO: implement IntoIterator
#[derive(Debug, Clone)]
pub struct Sqlite {
    pub pool: SqlitePool,
}

impl Sqlite {
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();
        debug!("opening sqlite database at {path:?}");

        if utils::broken_symlink(path) {
            eprintln!(
                "Atuin: Sqlite db path ({path:?}) is a broken symlink. Unable to read or create replacement."
            );
            std::process::exit(1);
        }

        if !path.exists()
            && let Some(dir) = path.parent()
        {
            fs::create_dir_all(dir)?;
        }

        let opts = SqliteConnectOptions::from_str(path.as_os_str().to_str().unwrap())?
            .journal_mode(SqliteJournalMode::Wal)
            .optimize_on_close(true, None)
            .synchronous(SqliteSynchronous::Normal)
            .with_regexp()
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .acquire_timeout(Duration::from_secs_f64(timeout))
            .connect_with(opts)
            .await?;

        Self::setup_db(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn sqlite_version(&self) -> Result<String> {
        sqlx::query_scalar("SELECT sqlite_version()")
            .fetch_one(&self.pool)
            .await
    }

    async fn setup_db(pool: &SqlitePool) -> Result<()> {
        debug!("running sqlite database setup");

        sqlx::migrate!("./migrations").run(pool).await?;

        Ok(())
    }

    async fn save_raw(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, h: &History) -> Result<()> {
        sqlx::query(
            "insert or ignore into history(
                id, timestamp, duration, exit, command, cwd, session, hostname, author, intent,
                deleted_at, shell
            ) values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )
        .bind(h.id.0.as_str())
        .bind(h.timestamp.unix_timestamp_nanos() as i64)
        .bind(h.duration)
        .bind(h.exit)
        .bind(h.command.as_str())
        .bind(h.cwd.as_str())
        .bind(h.session.as_str())
        .bind(h.hostname.as_str())
        .bind(h.author.as_str())
        .bind(h.intent.as_deref())
        .bind(h.deleted_at.map(|t| t.unix_timestamp_nanos() as i64))
        .bind(h.shell.as_deref())
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

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

    fn query_history(row: SqliteRow) -> History {
        let deleted_at: Option<i64> = row.get("deleted_at");
        let hostname: String = row.get("hostname");
        let author: Option<String> = row.try_get("author").ok().flatten();
        let author = author
            .filter(|author| !author.trim().is_empty())
            .unwrap_or_else(|| History::author_from_hostname(hostname.as_str()));
        let intent: Option<String> = row.try_get("intent").ok().flatten();
        let intent = intent.filter(|intent| !intent.trim().is_empty());
        let shell: Option<String> = row.try_get("shell").ok().flatten();

        History::from_db()
            .id(row.get("id"))
            .timestamp(
                OffsetDateTime::from_unix_timestamp_nanos(row.get::<i64, _>("timestamp") as i128)
                    .unwrap(),
            )
            .duration(row.get("duration"))
            .exit(row.get("exit"))
            .command(row.get("command"))
            .cwd(row.get("cwd"))
            .session(row.get("session"))
            .hostname(hostname)
            .author(author)
            .intent(intent)
            .deleted_at(
                deleted_at.and_then(|t| OffsetDateTime::from_unix_timestamp_nanos(t as i128).ok()),
            )
            .shell(shell)
            .build()
            .into()
    }
}

#[async_trait]
impl Database for Sqlite {
    async fn save(&self, h: &History) -> Result<()> {
        debug!("saving history to sqlite");
        let mut tx = self.pool.begin().await?;
        Self::save_raw(&mut tx, h).await?;
        tx.commit().await?;

        Ok(())
    }

    async fn save_bulk(&self, h: &[History]) -> Result<()> {
        debug!("saving history to sqlite");

        let mut tx = self.pool.begin().await?;

        for i in h {
            Self::save_raw(&mut tx, i).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<History>> {
        debug!("loading history item {}", id);

        let res = sqlx::query("select * from history where id = ?1")
            .bind(id)
            .map(Self::query_history)
            .fetch_optional(&self.pool)
            .await?;

        Ok(res)
    }

    async fn load_active(&self, ids: &[HistoryId]) -> Result<Vec<History>> {
        // sqlite caps bound parameters per statement (SQLITE_MAX_VARIABLE_NUMBER, as low as 999).
        // Chunk well under that.
        const CHUNK: usize = 500;

        debug!("loading {} history items", ids.len());

        let mut out = Vec::with_capacity(ids.len());

        for chunk in ids.chunks(CHUNK) {
            let placeholders = ["?"].repeat(chunk.len()).join(",");
            let sql = format!(
                "select * from history where id in ({placeholders}) and deleted_at is null"
            );

            let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
            for id in chunk {
                query = query.bind(id.0.as_str());
            }

            let rows = query.map(Self::query_history).fetch_all(&self.pool).await?;
            out.extend(rows);
        }

        Ok(out)
    }

    async fn update(&self, h: &History) -> Result<()> {
        debug!("updating sqlite history");

        sqlx::query(
            "update history
                set timestamp = ?2, duration = ?3, exit = ?4, command = ?5, cwd = ?6, session = ?7, hostname = ?8, author = ?9, intent = ?10, deleted_at = ?11
                where id = ?1",
        )
        .bind(h.id.0.as_str())
        .bind(h.timestamp.unix_timestamp_nanos() as i64)
        .bind(h.duration)
        .bind(h.exit)
        .bind(h.command.as_str())
        .bind(h.cwd.as_str())
        .bind(h.session.as_str())
        .bind(h.hostname.as_str())
        .bind(h.author.as_str())
        .bind(h.intent.as_deref())
        .bind(h.deleted_at.map(|t|t.unix_timestamp_nanos() as i64))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // make a unique list, that only shows the *newest* version of things
    async fn list(
        &self,
        filters: &[FilterMode],
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
                FilterMode::Host => query.and_where_eq("hostname", quote(&context.hostname)),
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

        let res = sqlx::query(sqlx::AssertSqlSafe(query))
            .map(Self::query_history)
            .fetch_all(&self.pool)
            .await?;

        Ok(res)
    }

    async fn range(&self, from: OffsetDateTime, to: OffsetDateTime) -> Result<Vec<History>> {
        debug!("listing history from {:?} to {:?}", from, to);

        let res = sqlx::query(
            "select * from history where timestamp >= ?1 and timestamp <= ?2 order by timestamp asc",
        )
        .bind(from.unix_timestamp_nanos() as i64)
        .bind(to.unix_timestamp_nanos() as i64)
            .map(Self::query_history)
        .fetch_all(&self.pool)
        .await?;

        Ok(res)
    }

    async fn last(&self) -> Result<Option<History>> {
        let res = sqlx::query(
            "select * from history where duration >= 0 order by timestamp desc limit 1",
        )
        .map(Self::query_history)
        .fetch_optional(&self.pool)
        .await?;

        Ok(res)
    }

    async fn before(&self, timestamp: OffsetDateTime, count: i64) -> Result<Vec<History>> {
        let res = sqlx::query(
            "select * from history where timestamp < ?1 order by timestamp desc limit ?2",
        )
        .bind(timestamp.unix_timestamp_nanos() as i64)
        .bind(count)
        .map(Self::query_history)
        .fetch_all(&self.pool)
        .await?;

        Ok(res)
    }

    async fn history_count(&self, include_deleted: bool) -> Result<i64> {
        let query = if include_deleted {
            "select count(1) from history"
        } else {
            "select count(1) from history where deleted_at is null"
        };

        let res: (i64,) = sqlx::query_as(query).fetch_one(&self.pool).await?;
        Ok(res.0)
    }

    async fn search(
        &self,
        search_mode: SearchMode,
        filter: FilterMode,
        context: &Context,
        query: &str,
        filter_options: OptFilters,
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
            FilterMode::Host => {
                sql.and_where_eq("lower(hostname)", quote(context.hostname.to_lowercase()))
            }
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
            SearchMode::Prefix => sql.and_where_like_left("command", query.replace('*', "%")),
            _ => {
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
                            if search_mode == SearchMode::FullText {
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

        filter_options
            .exit
            .map(|exit| sql.and_where_eq("exit", exit));

        filter_options
            .exclude_exit
            .map(|exclude_exit| sql.and_where_ne("exit", exclude_exit));

        if filter_options.only_failed {
            sql.and_where("exit != 0 AND exit != -1");
        }

        filter_options
            .cwd
            .map(|cwd| sql.and_where_eq("cwd", quote(cwd)));

        filter_options
            .exclude_cwd
            .map(|exclude_cwd| sql.and_where_ne("cwd", quote(exclude_cwd)));

        if let Some(before) = filter_options.before {
            let parsed = interim::parse_date_string(
                before.as_str(),
                OffsetDateTime::now_utc(),
                interim::Dialect::Uk,
            )
            .map_err(|e| {
                sqlx::Error::Decode(format!("invalid `before` filter {before:?}: {e}").into())
            })?;
            sql.and_where_lt("timestamp", quote(parsed.unix_timestamp_nanos() as i64));
        }

        if let Some(after) = filter_options.after {
            let parsed = interim::parse_date_string(
                after.as_str(),
                OffsetDateTime::now_utc(),
                interim::Dialect::Uk,
            )
            .map_err(|e| {
                sqlx::Error::Decode(format!("invalid `after` filter {after:?}: {e}").into())
            })?;
            sql.and_where_gt("timestamp", quote(parsed.unix_timestamp_nanos() as i64));
        }

        if !filter_options.authors.is_empty() {
            apply_author_filter(&mut sql, &filter_options.authors);
        }
        apply_shell_filter(&mut sql, &filter_options.shells);

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
                "SELECT * FROM ({inner}) f \
                 WHERE NOT EXISTS ( \
                     SELECT 1 FROM ({inner}) f2 \
                     WHERE f2.command = f.command \
                       AND (f2.timestamp, f2.id) > (f.timestamp, f.id) \
                 ) \
                 ORDER BY f.timestamp {order}{tail}"
            )
        };

        let res = sqlx::query(sqlx::AssertSqlSafe(query))
            .map(Self::query_history)
            .fetch_all(&self.pool)
            .await?;

        Ok(ordering::reorder_fuzzy(search_mode, orig_query, res))
    }

    async fn query_history(&self, query: &str) -> Result<Vec<History>> {
        let res = sqlx::query(sqlx::AssertSqlSafe(query))
            .map(Self::query_history)
            .fetch_all(&self.pool)
            .await?;

        Ok(res)
    }

    async fn all_with_count(&self) -> Result<Vec<(History, i32)>> {
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

        let res = sqlx::query(sqlx::AssertSqlSafe(query))
            .map(|row: SqliteRow| {
                let count: i32 = row.get("count");
                (Self::query_history(row), count)
            })
            .fetch_all(&self.pool)
            .await?;

        Ok(res)
    }

    fn all_paged(&self, page_size: usize, include_deleted: bool, unique: bool) -> Paged {
        Paged::new(Box::new(self.clone()), page_size, include_deleted, unique)
    }

    // deleted_at doesn't mean the actual time that the user deleted it,
    // but the time that the system marks it as deleted
    async fn delete(&self, mut h: History) -> Result<()> {
        let now = OffsetDateTime::now_utc();
        h.command = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(32)
            .map(char::from)
            .collect(); // overwrite with random string
        h.deleted_at = Some(now); // delete it

        self.update(&h).await?; // save it

        Ok(())
    }

    async fn delete_rows(&self, ids: &[HistoryId]) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for id in ids {
            Self::delete_row_raw(&mut tx, id.clone()).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    async fn stats(&self, h: &History) -> Result<HistoryStats> {
        // We select the previous in the session by time
        let mut prev = SqlBuilder::select_from("history");
        prev.field("*")
            .and_where("timestamp < ?1")
            .and_where("session = ?2")
            .order_by("timestamp", true)
            .limit(1);

        let mut next = SqlBuilder::select_from("history");
        next.field("*")
            .and_where("timestamp > ?1")
            .and_where("session = ?2")
            .order_by("timestamp", false)
            .limit(1);

        let mut total = SqlBuilder::select_from("history");
        total.field("count(1)").and_where("command = ?1");

        let mut average = SqlBuilder::select_from("history");
        average.field("avg(duration)").and_where("command = ?1");

        let mut exits = SqlBuilder::select_from("history");
        exits
            .fields(&["exit", "count(1) as count"])
            .and_where("command = ?1")
            .group_by("exit");

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
        let duration_over_time = duration_over_time
            .sql()
            .expect("issue in stats duration over time query");

        let prev = sqlx::query(sqlx::AssertSqlSafe(prev))
            .bind(h.timestamp.unix_timestamp_nanos() as i64)
            .bind(&h.session)
            .map(Self::query_history)
            .fetch_optional(&self.pool)
            .await?;

        let next = sqlx::query(sqlx::AssertSqlSafe(next))
            .bind(h.timestamp.unix_timestamp_nanos() as i64)
            .bind(&h.session)
            .map(Self::query_history)
            .fetch_optional(&self.pool)
            .await?;

        let total: (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(total))
            .bind(&h.command)
            .fetch_one(&self.pool)
            .await?;

        let average: (f64,) = sqlx::query_as(sqlx::AssertSqlSafe(average))
            .bind(&h.command)
            .fetch_one(&self.pool)
            .await?;

        let exits: Vec<(i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(exits))
            .bind(&h.command)
            .fetch_all(&self.pool)
            .await?;

        let day_of_week: Vec<(String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(day_of_week))
            .bind(&h.command)
            .fetch_all(&self.pool)
            .await?;

        let duration_over_time: Vec<(String, f64)> =
            sqlx::query_as(sqlx::AssertSqlSafe(duration_over_time))
                .bind(&h.command)
                .fetch_all(&self.pool)
                .await?;

        let duration_over_time = duration_over_time
            .iter()
            .map(|f| (f.0.clone(), f.1.round() as i64))
            .collect();

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

    async fn get_dups(&self, before: i64, dupkeep: u32) -> Result<Vec<History>> {
        let res = sqlx::query(
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
        .map(Self::query_history)
        .fetch_all(&self.pool)
        .await?;

        Ok(res)
    }

    fn clone_boxed(&self) -> Box<dyn Database + 'static> {
        Box::new(self.clone())
    }
}

pub struct Paged {
    database: Box<dyn Database + 'static>,
    page_size: usize,
    last_id: Option<String>,
    include_deleted: bool,
    unique: bool,
}

impl Paged {
    pub fn new(
        database: Box<dyn Database + 'static>,
        page_size: usize,
        include_deleted: bool,
        unique: bool,
    ) -> Self {
        Self {
            database,
            page_size,
            last_id: None,
            include_deleted,
            unique,
        }
    }

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
            query
                .group_by("command, cwd, hostname, session")
                .having("max(timestamp)");
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

impl<'a> QueryToken<'a> {
    pub fn has_uppercase(&self) -> bool {
        match self {
            Self::Match(term, _)
            | Self::MatchStart(term, _)
            | Self::MatchEnd(term, _)
            | Self::MatchFull(term, _) => term.contains(char::is_uppercase),
            _ => false,
        }
    }

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

            let mut is_inverse = false;
            if let Some(s) = part.strip_prefix('!') {
                part = s;
                is_inverse = true;
            }
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
    use crate::settings::test_local_timeout;

    use super::*;
    use rstest::rstest;
    use std::time::{Duration, Instant};
    use time::format_description::well_known::Rfc3339;

    fn new_context() -> Context {
        Context {
            hostname: "test:host".to_string(),
            session: "beepboopiamasession".to_string(),
            cwd: "/home/ellie".to_string(),
            host_id: "test-host".to_string(),
            git_root: None,
        }
    }

    async fn assert_search_eq(
        db: &impl Database,
        mode: SearchMode,
        filter_mode: FilterMode,
        query: &str,
        expected: usize,
    ) -> Result<Vec<History>> {
        let context = new_context();

        let results = db
            .search(
                mode,
                filter_mode,
                &context,
                query,
                OptFilters {
                    ..Default::default()
                },
            )
            .await?;

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
        db: &impl Database,
        mode: SearchMode,
        filter_mode: FilterMode,
        query: &str,
        expected_commands: Vec<&str>,
    ) {
        let results = assert_search_eq(db, mode, filter_mode, query, expected_commands.len())
            .await
            .unwrap();
        let commands: Vec<&str> = results.iter().map(|a| a.command.as_str()).collect();
        assert_eq!(commands, expected_commands);
    }

    async fn new_history_item(db: &mut impl Database, cmd: &str) -> Result<()> {
        new_history_item_at(db, cmd, None).await
    }

    async fn new_history_item_at(
        db: &mut impl Database,
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
        captured.hostname = "booop".to_string();

        db.save(&captured).await
    }

    async fn save_history_item(db: &impl Database, cmd: &str) -> History {
        let mut captured: History = History::capture()
            .timestamp(OffsetDateTime::now_utc())
            .command(cmd)
            .cwd("/home/ellie")
            .build()
            .into();

        captured.exit = 0;
        captured.duration = 1;
        captured.session = "beep boop".to_string();
        captured.hostname = "booop".to_string();

        db.save(&captured).await.unwrap();
        captured
    }

    // `stats --filter-mode` scopes over a period by handing `list` an inclusive
    // `(from, to)` range. The bounds must be inclusive on both ends so a command
    // recorded exactly on a period boundary (e.g. at midnight) is still counted.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_list_range_is_inclusive() {
        let db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();
        let context = Context {
            hostname: "booop".to_string(),
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
        past.hostname = "booop".to_string();
        db.save(&past).await.unwrap();
        save_history_item(&db, "ls /home/frank").await;

        // No range -> everything.
        let all = db
            .list(&[], &context, None, false, false, None)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);

        // A zero-width window on the item's exact timestamp matches it, because the
        // bounds are inclusive (`timestamp >= from AND timestamp <= to`).
        let hits = db
            .list(&[], &context, None, false, false, Some((at, at)))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].command, "ls /home/ellie");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_load_active_returns_only_requested_rows() {
        let db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();
        let alpha = save_history_item(&db, "echo alpha").await;
        let bravo = save_history_item(&db, "echo bravo").await;
        let _charlie = save_history_item(&db, "echo charlie").await;

        let loaded = db
            .load_active(&[alpha.id.clone(), bravo.id.clone()])
            .await
            .unwrap();

        let mut commands: Vec<String> = loaded.into_iter().map(|h| h.command).collect();
        commands.sort();
        assert_eq!(commands, vec!["echo alpha", "echo bravo"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_load_active_empty_never_reaches_sqlite() {
        let db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();
        save_history_item(&db, "echo alpha").await;

        // `select ... where id in ()` is a syntax error, so the empty case must
        // short-circuit rather than build a query.
        let loaded = db.load_active(&[]).await.unwrap();

        assert!(loaded.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_load_active_skips_soft_deleted() {
        let db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();
        let mut alpha = save_history_item(&db, "echo alpha").await;
        let bravo = save_history_item(&db, "echo bravo").await;

        alpha.deleted_at = Some(OffsetDateTime::now_utc());
        alpha.command = String::new();
        db.update(&alpha).await.unwrap();

        let loaded = db
            .load_active(&[alpha.id.clone(), bravo.id.clone()])
            .await
            .unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].command, "echo bravo");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_load_active_missing_ids_are_omitted() {
        let db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();
        let alpha = save_history_item(&db, "echo alpha").await;

        let loaded = db
            .load_active(&[alpha.id.clone(), HistoryId("does-not-exist".to_string())])
            .await
            .unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].command, "echo alpha");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_load_active_chunks_past_sqlite_param_limit() {
        let db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();

        // Comfortably over SQLITE_MAX_VARIABLE_NUMBER's 999 floor: a single
        // `in (...)` with one placeholder per id would fail here.
        let mut ids = Vec::new();
        for i in 0..1200 {
            ids.push(save_history_item(&db, &format!("echo {i}")).await.id);
        }

        let loaded = db.load_active(&ids).await.unwrap();

        assert_eq!(loaded.len(), 1200);
    }

    async fn db_with(commands: &[&str]) -> Sqlite {
        let mut db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();

        for command in commands {
            new_history_item(&mut db, command).await.unwrap();
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

        let mut db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();
        new_history_item_at(&mut db, "ls /home/ellie", Some(t))
            .await
            .unwrap();
        new_history_item_at(&mut db, "ls /home/frank", None)
            .await
            .unwrap();

        let context = new_context();

        let stamp = |seconds: i64| {
            (t + time::Duration::seconds(seconds))
                .format(&Rfc3339)
                .unwrap()
        };
        let (after, before) = match offsets {
            Some((after, before)) => (Some(stamp(after)), Some(stamp(before))),
            None => (None, None),
        };

        let results = db
            .search(
                SearchMode::FullText,
                FilterMode::Global,
                &context,
                "",
                OptFilters {
                    after,
                    before,
                    include_duplicates: true,
                    ..Default::default()
                },
            )
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
            .search(
                SearchMode::FullText,
                FilterMode::Global,
                &context,
                "",
                OptFilters {
                    include_duplicates,
                    ..Default::default()
                },
            )
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
            "before" => filters.before = Some("not a date".to_string()),
            "after" => filters.after = Some("not a date".to_string()),
            _ => unreachable!(),
        }

        let result = db
            .search(
                SearchMode::FullText,
                FilterMode::Global,
                &context,
                "",
                filters,
            )
            .await;

        assert!(result.is_err(), "unparsable `{which}` filter must error");
    }

    #[rstest]
    #[case::matches_prefix("ls", 1)]
    #[case::not_a_prefix("/home", 0)]
    #[case::trailing_whitespace("ls  ", 0)]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_prefix(#[case] query: &str, #[case] expected: usize) {
        let db = db_with(&["ls /home/ellie"]).await;

        assert_search_eq(&db, SearchMode::Prefix, FilterMode::Global, query, expected)
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

        assert_search_eq(
            &db,
            SearchMode::FullText,
            FilterMode::Global,
            query,
            expected,
        )
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

        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, query, expected)
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_reordered_fuzzy() {
        let mut db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();
        // test ordering of results: we should choose the first, even though it happened longer ago.

        new_history_item(&mut db, "curl").await.unwrap();
        new_history_item(&mut db, "corburl").await.unwrap();

        // if fuzzy reordering is on, it should come back in a more sensible order
        assert_search_commands(
            &db,
            SearchMode::Fuzzy,
            FilterMode::Global,
            "curl",
            vec!["curl", "corburl"],
        )
        .await;

        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "xxxx", 0)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "", 2)
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_paged_basic() {
        let mut db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();

        // Add 5 history items
        for i in 0..5 {
            new_history_item(&mut db, &format!("command{}", i))
                .await
                .unwrap();
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_paged_empty() {
        let db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();

        // Create a paged iterator on empty database
        let mut paged = db.all_paged(10, false, false);

        // Should return None immediately
        let page = paged.next().await.unwrap();
        assert!(page.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_paged_unique() {
        let mut db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();

        // Add duplicate commands
        new_history_item(&mut db, "duplicate").await.unwrap();
        new_history_item(&mut db, "duplicate").await.unwrap();
        new_history_item(&mut db, "unique1").await.unwrap();
        new_history_item(&mut db, "unique2").await.unwrap();

        // Without unique flag - should get all 4
        let mut paged = db.all_paged(10, false, false);
        let page = paged.next().await.unwrap().unwrap();
        assert_eq!(page.len(), 4);

        // With unique flag - should get 3 (duplicates collapsed)
        let mut paged_unique = db.all_paged(10, false, true);
        let page_unique = paged_unique.next().await.unwrap().unwrap();
        assert_eq!(page_unique.len(), 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_paged_include_deleted() {
        let mut db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();

        // Add items
        new_history_item(&mut db, "keep1").await.unwrap();
        new_history_item(&mut db, "keep2").await.unwrap();
        new_history_item(&mut db, "delete_me").await.unwrap();

        // Delete one item
        let all = db
            .list(
                &[],
                &Context {
                    hostname: "".to_string(),
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

        let to_delete = all
            .iter()
            .find(|h| h.command == "delete_me")
            .unwrap()
            .clone();
        db.delete(to_delete).await.unwrap();

        // Without include_deleted - should get 2
        let mut paged = db.all_paged(10, false, false);
        let page = paged.next().await.unwrap().unwrap();
        assert_eq!(page.len(), 2);

        // With include_deleted - should get 3
        let mut paged_deleted = db.all_paged(10, true, false);
        let page_deleted = paged_deleted.next().await.unwrap().unwrap();
        assert_eq!(page_deleted.len(), 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_bench_dupes() {
        let context = Context {
            hostname: "test:host".to_string(),
            session: "beepboopiamasession".to_string(),
            cwd: "/home/ellie".to_string(),
            host_id: "test-host".to_string(),
            git_root: None,
        };

        let mut db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();
        for _i in 1..10000 {
            new_history_item(&mut db, "i am a duplicated command")
                .await
                .unwrap();
        }
        let start = Instant::now();
        let _results = db
            .search(
                SearchMode::Fuzzy,
                FilterMode::Global,
                &context,
                "",
                OptFilters {
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let duration = start.elapsed();

        assert!(duration < Duration::from_secs(15));
    }
}
