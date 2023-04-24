use std::{env, path::Path, str::FromStr};

use async_trait::async_trait;
use atuin_common::utils;
use chrono::{prelude::*, Utc};
use fs_err as fs;
use itertools::Itertools;
use lazy_static::lazy_static;
use rand::{distributions::Alphanumeric, Rng};
use regex::Regex;
use sql_builder::{esc, quote, SqlBuilder, SqlName};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow},
    Result, Row,
};

use super::{
    history::History,
    ordering,
    settings::{FilterMode, SearchMode, Settings},
};

pub struct Context {
    pub session: String,
    pub cwd: String,
    pub hostname: String,
    pub host_id: String,
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
}

pub fn current_context() -> Context {
    let Ok(session) = env::var("ATUIN_SESSION") else {
        eprintln!("ERROR: Failed to find $ATUIN_SESSION in the environment. Check that you have correctly set up your shell.");
        std::process::exit(1);
    };
    let hostname = format!(
        "{}:{}",
        env::var("ATUIN_HOST_NAME").unwrap_or_else(|_| whoami::hostname()),
        env::var("ATUIN_HOST_USER").unwrap_or_else(|_| whoami::username())
    );
    let cwd = utils::get_current_dir();
    let host_id = Settings::host_id().expect("failed to load host ID");

    Context {
        session,
        hostname,
        cwd,
        host_id,
    }
}

#[async_trait]
pub trait Database: Send + Sync + 'static {
    async fn save(&mut self, h: &History) -> Result<()>;
    async fn save_bulk(&mut self, h: &[History]) -> Result<()>;

    async fn load(&self, id: &str) -> Result<History>;
    async fn list(
        &self,
        filter: FilterMode,
        context: &Context,
        max: Option<usize>,
        unique: bool,
    ) -> Result<Vec<History>>;
    async fn range(
        &self,
        from: chrono::DateTime<Utc>,
        to: chrono::DateTime<Utc>,
    ) -> Result<Vec<History>>;

    async fn update(&self, h: &History) -> Result<()>;
    async fn history_count(&self) -> Result<i64>;

    async fn first(&self) -> Result<History>;
    async fn last(&self) -> Result<History>;
    async fn before(&self, timestamp: chrono::DateTime<Utc>, count: i64) -> Result<Vec<History>>;

    async fn delete(&self, mut h: History) -> Result<()>;
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
}

// Intended for use on a developer machine and not a sync server.
// TODO: implement IntoIterator
pub struct Sqlite {
    pool: SqlitePool,
}

impl Sqlite {
    pub async fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        debug!("opening sqlite database at {:?}", path);

        let create = !path.exists();
        if create {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
        }

        let opts = SqliteConnectOptions::from_str(path.as_os_str().to_str().unwrap())?
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new().connect_with(opts).await?;

        Self::setup_db(&pool).await?;

        Ok(Self { pool })
    }

    async fn setup_db(pool: &SqlitePool) -> Result<()> {
        debug!("running sqlite database setup");

        sqlx::migrate!("./migrations").run(pool).await?;

        Ok(())
    }

    async fn save_raw(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, h: &History) -> Result<()> {
        sqlx::query(
            "insert or ignore into history(id, timestamp, duration, exit, command, cwd, session, hostname, deleted_at)
                values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )
        .bind(h.id.as_str())
        .bind(h.timestamp.timestamp_nanos())
        .bind(h.duration)
        .bind(h.exit)
        .bind(h.command.as_str())
        .bind(h.cwd.as_str())
        .bind(h.session.as_str())
        .bind(h.hostname.as_str())
        .bind(h.deleted_at.map(|t|t.timestamp_nanos()))
        .execute(tx)
        .await?;

        Ok(())
    }

    fn query_history(row: SqliteRow) -> History {
        let deleted_at: Option<i64> = row.get("deleted_at");

        History::from_db()
            .id(row.get("id"))
            .timestamp(Utc.timestamp_nanos(row.get("timestamp")))
            .duration(row.get("duration"))
            .exit(row.get("exit"))
            .command(row.get("command"))
            .cwd(row.get("cwd"))
            .session(row.get("session"))
            .hostname(row.get("hostname"))
            .deleted_at(deleted_at.map(|t| Utc.timestamp_nanos(t)))
            .build()
            .into()
    }
}

#[async_trait]
impl Database for Sqlite {
    async fn save(&mut self, h: &History) -> Result<()> {
        debug!("saving history to sqlite");
        let mut tx = self.pool.begin().await?;
        Self::save_raw(&mut tx, h).await?;
        tx.commit().await?;

        Ok(())
    }

    async fn save_bulk(&mut self, h: &[History]) -> Result<()> {
        debug!("saving history to sqlite");

        let mut tx = self.pool.begin().await?;

        for i in h {
            Self::save_raw(&mut tx, i).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    async fn load(&self, id: &str) -> Result<History> {
        debug!("loading history item {}", id);

        let res = sqlx::query("select * from history where id = ?1")
            .bind(id)
            .map(Self::query_history)
            .fetch_one(&self.pool)
            .await?;

        Ok(res)
    }

    async fn update(&self, h: &History) -> Result<()> {
        debug!("updating sqlite history");

        sqlx::query(
            "update history
                set timestamp = ?2, duration = ?3, exit = ?4, command = ?5, cwd = ?6, session = ?7, hostname = ?8, deleted_at = ?9
                where id = ?1",
        )
        .bind(h.id.as_str())
        .bind(h.timestamp.timestamp_nanos())
        .bind(h.duration)
        .bind(h.exit)
        .bind(h.command.as_str())
        .bind(h.cwd.as_str())
        .bind(h.session.as_str())
        .bind(h.hostname.as_str())
        .bind(h.deleted_at.map(|t|t.timestamp_nanos()))
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // make a unique list, that only shows the *newest* version of things
    async fn list(
        &self,
        filter: FilterMode,
        context: &Context,
        max: Option<usize>,
        unique: bool,
    ) -> Result<Vec<History>> {
        debug!("listing history");

        let mut query = SqlBuilder::select_from(SqlName::new("history").alias("h").baquoted());
        query.field("*").order_desc("timestamp");

        match filter {
            FilterMode::Global => &mut query,
            FilterMode::Host => query.and_where_eq("hostname", quote(&context.hostname)),
            FilterMode::Session => query.and_where_eq("session", quote(&context.session)),
            FilterMode::Directory => query.and_where_eq("cwd", quote(&context.cwd)),
        };

        if unique {
            query.and_where_eq(
                "timestamp",
                "(select max(timestamp) from history where h.command = history.command)",
            );
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

    async fn range(
        &self,
        from: chrono::DateTime<Utc>,
        to: chrono::DateTime<Utc>,
    ) -> Result<Vec<History>> {
        debug!("listing history from {:?} to {:?}", from, to);

        let res = sqlx::query(
            "select * from history where timestamp >= ?1 and timestamp <= ?2 order by timestamp asc",
        )
        .bind(from.timestamp_nanos())
        .bind(to.timestamp_nanos())
            .map(Self::query_history)
        .fetch_all(&self.pool)
        .await?;

        Ok(res)
    }

    async fn first(&self) -> Result<History> {
        let res =
            sqlx::query("select * from history where duration >= 0 order by timestamp asc limit 1")
                .map(Self::query_history)
                .fetch_one(&self.pool)
                .await?;

        Ok(res)
    }

    async fn last(&self) -> Result<History> {
        let res = sqlx::query(
            "select * from history where duration >= 0 order by timestamp desc limit 1",
        )
        .map(Self::query_history)
        .fetch_one(&self.pool)
        .await?;

        Ok(res)
    }

    async fn before(&self, timestamp: chrono::DateTime<Utc>, count: i64) -> Result<Vec<History>> {
        let res = sqlx::query(
            "select * from history where timestamp < ?1 order by timestamp desc limit ?2",
        )
        .bind(timestamp.timestamp_nanos())
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

    async fn history_count(&self) -> Result<i64> {
        let res: (i64,) = sqlx::query_as("select count(1) from history")
            .fetch_one(&self.pool)
            .await?;

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

        sql.group_by("command").having("max(timestamp)");

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

        match filter {
            FilterMode::Global => &mut sql,
            FilterMode::Host => sql.and_where_eq("hostname", quote(&context.hostname)),
            FilterMode::Session => sql.and_where_eq("session", quote(&context.session)),
            FilterMode::Directory => sql.and_where_eq("cwd", quote(&context.cwd)),
        };

        let orig_query = query;
        let query = query.replace('*', "%"); // allow wildcard char

        match search_mode {
            SearchMode::Prefix => sql.and_where_like_left("command", query),
            SearchMode::FullText => sql.and_where_like_any("command", query),
            _ => {
                // don't recompile the regex on successive calls!
                lazy_static! {
                    static ref SPLIT_REGEX: Regex = Regex::new(r" +").unwrap();
                }

                let mut is_or = false;
                for query_part in SPLIT_REGEX.split(query.as_str()) {
                    // TODO smart case mode could be made configurable like in fzf
                    let (is_glob, glob) = if query_part.contains(char::is_uppercase) {
                        (true, "*")
                    } else {
                        (false, "%")
                    };

                    let (is_inverse, query_part) = match query_part.strip_prefix('!') {
                        Some(stripped) => (true, stripped),
                        None => (false, query_part),
                    };

                    let param = if query_part == "|" {
                        if !is_or {
                            is_or = true;
                            continue;
                        } else {
                            format!("{glob}|{glob}")
                        }
                    } else if let Some(term) = query_part.strip_prefix('^') {
                        format!("{term}{glob}")
                    } else if let Some(term) = query_part.strip_suffix('$') {
                        format!("{glob}{term}")
                    } else if let Some(term) = query_part.strip_prefix('\'') {
                        format!("{glob}{term}{glob}")
                    } else if is_inverse {
                        format!("{glob}{query_part}{glob}")
                    } else {
                        query_part.split("").join(glob)
                    };

                    sql.fuzzy_condition("command", param, is_inverse, is_glob, is_or);
                    is_or = false;
                }
                &mut sql
            }
        };

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
            interim::parse_date_string(before.as_str(), Utc::now(), interim::Dialect::Uk)
                .map(|before| sql.and_where_lt("timestamp", quote(before.timestamp_nanos())))
        });

        filter_options.after.map(|after| {
            interim::parse_date_string(after.as_str(), Utc::now(), interim::Dialect::Uk)
                .map(|after| sql.and_where_gt("timestamp", quote(after.timestamp_nanos())))
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
        let now = chrono::Utc::now();
        h.command = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(32)
            .map(char::from)
            .collect(); // overwrite with random string
        h.deleted_at = Some(now); // delete it

        self.update(&h).await?; // save it

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::time::{Duration, Instant};

    async fn assert_search_eq<'a>(
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
            .timestamp(chrono::Utc::now())
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
        let mut db = Sqlite::new("sqlite::memory:").await.unwrap();
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
        let mut db = Sqlite::new("sqlite::memory:").await.unwrap();
        new_history_item(&mut db, "ls /home/ellie").await.unwrap();

        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "ls", 1)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "/home", 1)
            .await
            .unwrap();
        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "ls  ", 0)
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_fuzzy() {
        let mut db = Sqlite::new("sqlite::memory:").await.unwrap();
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
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_reordered_fuzzy() {
        let mut db = Sqlite::new("sqlite::memory:").await.unwrap();
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
        };

        let mut db = Sqlite::new("sqlite::memory:").await.unwrap();
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
