use std::{
    env,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

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
    pub cwd: Option<String>,
    pub exclude_cwd: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub reverse: bool,
    pub include_duplicates: bool,
}

pub async fn current_context() -> eyre::Result<Context> {
    let session = env::var("ATUIN_SESSION").map_err(|_| {
        eyre::eyre!("Failed to find $ATUIN_SESSION in the environment. Check that you have correctly set up your shell.")
    })?;
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
    async fn list(
        &self,
        filters: &[FilterMode],
        context: &Context,
        max: Option<usize>,
        unique: bool,
        include_deleted: bool,
    ) -> Result<Vec<History>>;
    async fn range(&self, from: OffsetDateTime, to: OffsetDateTime) -> Result<Vec<History>>;

    async fn update(&self, h: &History) -> Result<()>;
    async fn history_count(&self, include_deleted: bool) -> Result<i64>;

    async fn last(&self) -> Result<Option<History>>;
    async fn before(&self, timestamp: OffsetDateTime, count: i64) -> Result<Vec<History>>;

    async fn delete(&self, h: History) -> Result<()>;
    async fn delete_rows(&self, ids: &[HistoryId]) -> Result<()>;
    async fn deleted(&self) -> Result<Vec<History>>;

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

    async fn stats(&self, h: &History) -> Result<HistoryStats>;

    async fn get_dups(&self, before: i64, dupkeep: u32) -> Result<Vec<History>>;
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
            "insert or ignore into history(id, timestamp, duration, exit, command, cwd, session, hostname, author, intent, deleted_at)
                values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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

        let query = query.sql().expect("bug in list query. please report");

        let res = sqlx::query(&query)
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

    async fn deleted(&self) -> Result<Vec<History>> {
        let res = sqlx::query("select * from history where deleted_at is not null")
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
        let mut sql = SqlBuilder::select_from("history");

        if !filter_options.include_duplicates {
            sql.group_by("command").having("max(timestamp)");
        }

        if let Some(limit) = filter_options.limit {
            sql.limit(limit);
        }

        if let Some(offset) = filter_options.offset {
            sql.offset(offset);
        }

        if filter_options.reverse {
            sql.order_asc("timestamp");
        } else {
            sql.order_desc("timestamp");
        }

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

        filter_options
            .cwd
            .map(|cwd| sql.and_where_eq("cwd", quote(cwd)));

        filter_options
            .exclude_cwd
            .map(|exclude_cwd| sql.and_where_ne("cwd", quote(exclude_cwd)));

        filter_options.before.map(|before| {
            interim::parse_date_string(
                before.as_str(),
                OffsetDateTime::now_utc(),
                interim::Dialect::Uk,
            )
            .map(|before| {
                sql.and_where_lt("timestamp", quote(before.unix_timestamp_nanos() as i64))
            })
        });

        filter_options.after.map(|after| {
            interim::parse_date_string(
                after.as_str(),
                OffsetDateTime::now_utc(),
                interim::Dialect::Uk,
            )
            .map(|after| sql.and_where_gt("timestamp", quote(after.unix_timestamp_nanos() as i64)))
        });

        sql.and_where_is_null("deleted_at");

        let query = sql.sql().expect("bug in search query. please report");

        let res = sqlx::query(&query)
            .map(Self::query_history)
            .fetch_all(&self.pool)
            .await?;

        Ok(ordering::reorder_fuzzy(search_mode, orig_query, res))
    }

    async fn query_history(&self, query: &str) -> Result<Vec<History>> {
        let res = sqlx::query(query)
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

        let res = sqlx::query(&query)
            .map(|row: SqliteRow| {
                let count: i32 = row.get("count");
                (Self::query_history(row), count)
            })
            .fetch_all(&self.pool)
            .await?;

        Ok(res)
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

        let prev = sqlx::query(&prev)
            .bind(h.timestamp.unix_timestamp_nanos() as i64)
            .bind(&h.session)
            .map(Self::query_history)
            .fetch_optional(&self.pool)
            .await?;

        let next = sqlx::query(&next)
            .bind(h.timestamp.unix_timestamp_nanos() as i64)
            .bind(&h.session)
            .map(Self::query_history)
            .fetch_optional(&self.pool)
            .await?;

        let total: (i64,) = sqlx::query_as(&total)
            .bind(&h.command)
            .fetch_one(&self.pool)
            .await?;

        let average: (f64,) = sqlx::query_as(&average)
            .bind(&h.command)
            .fetch_one(&self.pool)
            .await?;

        let exits: Vec<(i64, i64)> = sqlx::query_as(&exits)
            .bind(&h.command)
            .fetch_all(&self.pool)
            .await?;

        let day_of_week: Vec<(String, i64)> = sqlx::query_as(&day_of_week)
            .bind(&h.command)
            .fetch_all(&self.pool)
            .await?;

        let duration_over_time: Vec<(String, f64)> = sqlx::query_as(&duration_over_time)
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

#[cfg(test)]
mod test {
    use crate::settings::test_local_timeout;

    use super::*;
    use std::time::{Duration, Instant};

    async fn assert_search_eq(
        db: &impl Database,
        mode: SearchMode,
        filter_mode: FilterMode,
        query: &str,
        expected: usize,
    ) -> Result<Vec<History>> {
        let context = Context {
            hostname: "test:host".to_string(),
            session: "beepboopiamasession".to_string(),
            cwd: "/home/ellie".to_string(),
            host_id: "test-host".to_string(),
            git_root: None,
        };

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

        db.save(&captured).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_prefix() {
        let mut db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();
        new_history_item(&mut db, "ls /home/ellie").await.unwrap();

        assert_search_eq(&db, SearchMode::Prefix, FilterMode::Global, "ls", 1)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Prefix, FilterMode::Global, "/home", 0)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Prefix, FilterMode::Global, "ls  ", 0)
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_fulltext() {
        let mut db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();
        new_history_item(&mut db, "ls /home/ellie").await.unwrap();

        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "ls", 1)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "/home", 1)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "ls ho", 1)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "hm", 0)
            .await
            .unwrap();

        // regex
        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "r/^ls ", 1)
            .await
            .unwrap();
        assert_search_eq(
            &db,
            SearchMode::FullText,
            FilterMode::Global,
            "r/ls / ie$",
            1,
        )
        .await
        .unwrap();
        assert_search_eq(
            &db,
            SearchMode::FullText,
            FilterMode::Global,
            "r/ls / !ie",
            0,
        )
        .await
        .unwrap();
        assert_search_eq(
            &db,
            SearchMode::FullText,
            FilterMode::Global,
            "meow r/ls/",
            0,
        )
        .await
        .unwrap();
        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "r//hom/", 1)
            .await
            .unwrap();
        assert_search_eq(
            &db,
            SearchMode::FullText,
            FilterMode::Global,
            "r//home//",
            1,
        )
        .await
        .unwrap();
        assert_search_eq(
            &db,
            SearchMode::FullText,
            FilterMode::Global,
            "r//home///",
            0,
        )
        .await
        .unwrap();
        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "/home.*e", 0)
            .await
            .unwrap();
        assert_search_eq(
            &db,
            SearchMode::FullText,
            FilterMode::Global,
            "r/home.*e",
            1,
        )
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_fuzzy() {
        let mut db = Sqlite::new("sqlite::memory:", test_local_timeout())
            .await
            .unwrap();
        new_history_item(&mut db, "ls /home/ellie").await.unwrap();
        new_history_item(&mut db, "ls /home/frank").await.unwrap();
        new_history_item(&mut db, "cd /home/Ellie").await.unwrap();
        new_history_item(&mut db, "/home/ellie/.bin/rustup")
            .await
            .unwrap();

        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "ls /", 3)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "ls/", 2)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "l/h/", 2)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "/h/e", 3)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "/hmoe/", 0)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "ellie/home", 0)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "lsellie", 1)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, " ", 4)
            .await
            .unwrap();

        // single term operators
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "^ls", 2)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "'ls", 2)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "ellie$", 2)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "!^ls", 2)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "!ellie", 1)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "!ellie$", 2)
            .await
            .unwrap();

        // multiple terms
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "ls !ellie", 1)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "^ls !e$", 1)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "home !^ls", 2)
            .await
            .unwrap();
        assert_search_eq(
            &db,
            SearchMode::Fuzzy,
            FilterMode::Global,
            "'frank | 'rustup",
            2,
        )
        .await
        .unwrap();
        assert_search_eq(
            &db,
            SearchMode::Fuzzy,
            FilterMode::Global,
            "'frank | 'rustup 'ls",
            1,
        )
        .await
        .unwrap();

        // case matching
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "Ellie", 1)
            .await
            .unwrap();

        // regex
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "r/^ls ", 2)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "r/[Ee]llie", 3)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "/h/e r/^ls ", 1)
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
