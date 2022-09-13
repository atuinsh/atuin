use crate::{
    history::History,
    ordering,
    settings::{FilterMode, SearchMode},
};
use chrono::{prelude::*, Utc};
use fallible_iterator::FallibleIterator;
use fs_err as fs;
use itertools::Itertools;
use lazy_static::lazy_static;
use regex::Regex;
use rusqlite::{Connection, Params, Result, Row, Transaction};
use sha2::{Digest, Sha384};
use sql_builder::{esc, quote, SqlBuilder, SqlName};
use std::{collections::HashMap, path::Path, time::Instant};

use super::{Context, Database};

// Intended for use on a developer machine and not a sync server.
// TODO: implement IntoIterator
pub struct Sqlite {
    conn: Connection,
}

impl Sqlite {
    pub fn new(path: impl AsRef<Path>) -> eyre::Result<Self> {
        let path = path.as_ref();
        debug!("opening sqlite database at {:?}", path);

        let create = !path.exists();
        if create {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
        }

        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "wal")?;

        Self::setup_db(&mut conn)?;

        Ok(Self { conn })
    }

    fn setup_db(conn: &mut Connection) -> eyre::Result<()> {
        debug!("running sqlite database setup");

        conn.execute(
            "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL,
            checksum BLOB NOT NULL,
            execution_time BIGINT NOT NULL
        );",
            [],
        )?;

        let dirty_version: Option<i64> = conn
            .prepare(
                "SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version LIMIT 1",
            )?
            .query([])?
            .map(|r| r.get(0))
            .next()?;
        if dirty_version.is_some() {
            eyre::bail!("dirty");
        }

        let applied_versions: Result<HashMap<i64, Vec<u8>>> = conn
            .prepare("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")?
            .query_map((), |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect();
        let applied_versions = applied_versions?;

        struct Mig {
            version: i64,
            desc: &'static str,
            query: &'static str,
        }

        // Add new migrations here
        let migrations = [
            Mig {
                version: 20210422143411,
                desc: "create history",
                query: include_str!("migrations/20210422143411_create_history.sql"),
            },
            Mig {
                version: 20220806155627,
                desc: "interactive search index",
                query: include_str!("migrations/20220806155627_interactive_search_index.sql"),
            },
        ];
        for m in migrations {
            let checksum = Sha384::digest(m.query.as_bytes());
            match applied_versions.get(&m.version) {
                Some(chck) => {
                    if chck != checksum.as_slice() {
                        eyre::bail!("invalid version {}", m.version);
                    }
                }
                None => {
                    let tx = conn.transaction()?;
                    let start = Instant::now();

                    let _ = tx.execute(m.query, [])?;

                    tx.commit()?;

                    let elapsed = start.elapsed();

                    let _ = conn.execute(
                        r#"
                            INSERT INTO _sqlx_migrations ( version, description, success, checksum, execution_time )
                            VALUES ( ?1, ?2, TRUE, ?3, ?4 )
                        "#,
                        (
                            m.version,
                            m.desc,
                            checksum.as_slice(),
                            elapsed.as_nanos() as i64,
                        ),
                    )?;
                }
            }
        }

        Ok(())
    }

    fn save_raw(tx: &mut Transaction, h: &History) -> Result<()> {
        tx.execute(
            "insert or ignore into history(id, timestamp, duration, exit, command, cwd, session, hostname)
                values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                h.id.as_str(),
                h.timestamp.timestamp_nanos(),
                h.duration,
                h.exit,
                h.command.as_str(),
                h.cwd.as_str(),
                h.session.as_str(),
                h.hostname.as_str(),
            ),
        )?;
        Ok(())
    }

    fn query_history(row: &Row<'_>) -> Result<History> {
        Ok(History {
            id: row.get("id")?,
            timestamp: Utc.timestamp_nanos(row.get("timestamp")?),
            duration: row.get("duration")?,
            exit: row.get("exit")?,
            command: row.get("command")?,
            cwd: row.get("cwd")?,
            session: row.get("session")?,
            hostname: row.get("hostname")?,
        })
    }

    fn query_one(&self, query: &str, p: impl Params) -> Result<History> {
        self.conn.query_row(query, p, Self::query_history)
    }

    fn query_many(&self, query: &str, p: impl Params) -> Result<Vec<History>> {
        self.conn
            .prepare(query)?
            .query_map(p, Self::query_history)?
            .collect()
    }
}

impl Database for Sqlite {
    type Error = rusqlite::Error;
    fn save(&mut self, h: &History) -> Result<()> {
        debug!("saving history to sqlite");

        let mut tx = self.conn.transaction()?;
        Self::save_raw(&mut tx, h)?;
        tx.commit()?;

        Ok(())
    }

    fn save_bulk(&mut self, h: &[History]) -> Result<()> {
        debug!("saving history to sqlite");

        let mut tx = self.conn.transaction()?;

        for i in h {
            Self::save_raw(&mut tx, i)?
        }

        tx.commit()?;

        Ok(())
    }

    fn load(&self, id: &str) -> Result<History> {
        debug!("loading history item {}", id);

        self.conn.query_row(
            "select * from history where id = ?1",
            (id,),
            Self::query_history,
        )
    }

    fn update(&self, h: &History) -> Result<()> {
        debug!("updating sqlite history");
        self.conn
            .execute(
                "update history
                set timestamp = ?2, duration = ?3, exit = ?4, command = ?5, cwd = ?6, session = ?7, hostname = ?8
                where id = ?1",
                (
                    h.id.as_str(),
                    h.timestamp.timestamp_nanos(),
                    h.duration,
                    h.exit,
                    h.command.as_str(),
                    h.cwd.as_str(),
                    h.session.as_str(),
                    h.hostname.as_str(),
                ),
            )?;
        Ok(())
    }

    // make a unique list, that only shows the *newest* version of things
    fn list(
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

        self.query_many(&query, ())
    }

    fn range(
        &self,
        from: chrono::DateTime<Utc>,
        to: chrono::DateTime<Utc>,
    ) -> Result<Vec<History>> {
        debug!("listing history from {:?} to {:?}", from, to);
        let q = "select * from history where timestamp >= ?1 and timestamp <= ?2 order by timestamp asc";
        self.query_many(q, (from.timestamp_nanos(), to.timestamp_nanos()))
    }

    fn first(&self) -> Result<History> {
        self.query_one(
            "select * from history where duration >= 0 order by timestamp asc limit 1",
            (),
        )
    }

    fn last(&self) -> Result<History> {
        let q = "select * from history where duration >= 0 order by timestamp desc limit 1";
        self.query_one(q, ())
    }

    fn before(&self, timestamp: chrono::DateTime<Utc>, count: i64) -> Result<Vec<History>> {
        let q = "select * from history where timestamp < ?1 order by timestamp desc limit ?2";
        self.query_many(q, (timestamp.timestamp_nanos(), count))
    }

    fn history_count(&self) -> Result<i64> {
        self.conn
            .query_row("select count(1) from history", (), |r| r.get(0))
    }

    fn search(
        &self,
        limit: Option<i64>,
        search_mode: SearchMode,
        filter: FilterMode,
        context: &Context,
        query: &str,
    ) -> Result<Vec<History>> {
        let mut sql = SqlBuilder::select_from("history");

        sql.group_by("command")
            .having("max(timestamp)")
            .order_desc("timestamp");

        if let Some(limit) = limit {
            sql.limit(limit);
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
            SearchMode::Fuzzy => {
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
                        format!("{glob}{term}{glob}", term = query_part)
                    } else {
                        query_part.split("").join(glob)
                    };

                    sql.fuzzy_condition("command", param, is_inverse, is_glob, is_or);
                    is_or = false;
                }
                &mut sql
            }
        };

        let query = sql.sql().expect("bug in search query. please report");
        let res = self.query_many(&query, ())?;
        Ok(ordering::reorder_fuzzy(search_mode, orig_query, res))
    }

    fn query_history(&self, query: &str) -> Result<Vec<History>, Self::Error> {
        self.query_many(query, ())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::time::{Duration, Instant};

    fn assert_search_eq(
        db: &Sqlite,
        mode: SearchMode,
        filter_mode: FilterMode,
        query: &str,
        expected: usize,
    ) -> Result<Vec<History>> {
        let context = Context {
            hostname: "test:host".to_string(),
            session: "beepboopiamasession".to_string(),
            cwd: "/home/ellie".to_string(),
        };
        let results = db.search(None, mode, filter_mode, &context, query)?;
        assert_eq!(
            results.len(),
            expected,
            "query \"{}\", commands: {:?}",
            query,
            results.iter().map(|a| &a.command).collect::<Vec<&String>>()
        );
        Ok(results)
    }

    fn assert_search_commands(
        db: &Sqlite,
        mode: SearchMode,
        filter_mode: FilterMode,
        query: &str,
        expected_commands: Vec<&str>,
    ) {
        let results =
            assert_search_eq(db, mode, filter_mode, query, expected_commands.len()).unwrap();
        let commands: Vec<&str> = results.iter().map(|a| a.command.as_str()).collect();
        assert_eq!(commands, expected_commands);
    }

    fn new_history_item(db: &mut Sqlite, cmd: &str) -> Result<()> {
        let history = History::new(
            chrono::Utc::now(),
            cmd.to_string(),
            "/home/ellie".to_string(),
            0,
            1,
            Some("beep boop".to_string()),
            Some("booop".to_string()),
        );
        db.save(&history)
    }

    #[test]
    fn test_search_prefix() {
        let mut db = Sqlite::new("sqlite::memory:").unwrap();
        new_history_item(&mut db, "ls /home/ellie").unwrap();

        assert_search_eq(&db, SearchMode::Prefix, FilterMode::Global, "ls", 1).unwrap();
        assert_search_eq(&db, SearchMode::Prefix, FilterMode::Global, "/home", 0).unwrap();
        assert_search_eq(&db, SearchMode::Prefix, FilterMode::Global, "ls  ", 0).unwrap();
    }

    #[test]
    fn test_search_fulltext() {
        let mut db = Sqlite::new("sqlite::memory:").unwrap();
        new_history_item(&mut db, "ls /home/ellie").unwrap();

        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "ls", 1).unwrap();
        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "/home", 1).unwrap();
        assert_search_eq(&db, SearchMode::FullText, FilterMode::Global, "ls  ", 0).unwrap();
    }

    #[test]
    fn test_search_fuzzy() {
        let mut db = Sqlite::new("sqlite::memory:").unwrap();
        new_history_item(&mut db, "ls /home/ellie").unwrap();
        new_history_item(&mut db, "ls /home/frank").unwrap();
        new_history_item(&mut db, "cd /home/Ellie").unwrap();
        new_history_item(&mut db, "/home/ellie/.bin/rustup").unwrap();

        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "ls /", 3).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "ls/", 2).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "l/h/", 2).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "/h/e", 3).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "/hmoe/", 0).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "ellie/home", 0).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "lsellie", 1).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, " ", 4).unwrap();

        // single term operators
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "^ls", 2).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "'ls", 2).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "ellie$", 2).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "!^ls", 2).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "!ellie", 1).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "!ellie$", 2).unwrap();

        // multiple terms
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "ls !ellie", 1).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "^ls !e$", 1).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "home !^ls", 2).unwrap();
        assert_search_eq(
            &db,
            SearchMode::Fuzzy,
            FilterMode::Global,
            "'frank | 'rustup",
            2,
        )
        .unwrap();
        assert_search_eq(
            &db,
            SearchMode::Fuzzy,
            FilterMode::Global,
            "'frank | 'rustup 'ls",
            1,
        )
        .unwrap();

        // case matching
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "Ellie", 1).unwrap();
    }

    #[test]
    fn test_search_reordered_fuzzy() {
        let mut db = Sqlite::new("sqlite::memory:").unwrap();
        new_history_item(&mut db, "curl").unwrap();
        new_history_item(&mut db, "corburl").unwrap();

        assert_search_commands(
            &db,
            SearchMode::Fuzzy,
            FilterMode::Global,
            "curl",
            vec!["curl", "corburl"],
        );
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "xxxx", 0).unwrap();
        assert_search_eq(&db, SearchMode::Fuzzy, FilterMode::Global, "", 2).unwrap();
    }

    #[test]
    fn test_search_bench_dupes() {
        let context = Context {
            hostname: "test:host".to_string(),
            session: "beepboopiamasession".to_string(),
            cwd: "/home/ellie".to_string(),
        };
        let mut db = Sqlite::new("sqlite::memory:").unwrap();
        for _i in 1..10000 {
            new_history_item(&mut db, "i am a duplicated command").unwrap();
        }

        let start = Instant::now();
        let _results = db
            .search(None, SearchMode::Fuzzy, FilterMode::Global, &context, "")
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
        cond.push_str(&esc(&mask.to_string()));
        cond.push('\'');
        if is_or {
            self.or_where(cond)
        } else {
            self.and_where(cond)
        }
    }
}
