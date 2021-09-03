use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use chrono::prelude::*;
use chrono::Utc;

use eyre::Result;
use itertools::Itertools;

use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
};
use sqlx::Row;

use super::history::History;
use super::ordering;
use super::settings::SearchMode;

#[async_trait]
pub trait Database {
    async fn save(&mut self, h: &History) -> Result<()>;
    async fn save_bulk(&mut self, h: &[History]) -> Result<()>;

    async fn load(&self, id: &str) -> Result<History>;
    async fn list(&self, max: Option<usize>, unique: bool) -> Result<Vec<History>>;
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

    async fn search(
        &self,
        limit: Option<i64>,
        search_mode: SearchMode,
        query: &str,
    ) -> Result<Vec<History>>;

    async fn query_history(&self, query: &str) -> Result<Vec<History>>;
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
                std::fs::create_dir_all(dir)?;
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
            "insert or ignore into history(id, timestamp, duration, exit, command, cwd, session, hostname)
                values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(h.id.as_str())
        .bind(h.timestamp.timestamp_nanos())
        .bind(h.duration)
        .bind(h.exit)
        .bind(h.command.as_str())
        .bind(h.cwd.as_str())
        .bind(h.session.as_str())
        .bind(h.hostname.as_str())
        .execute(tx)
        .await?;

        Ok(())
    }

    fn query_history(row: SqliteRow) -> History {
        History {
            id: row.get("id"),
            timestamp: Utc.timestamp_nanos(row.get("timestamp")),
            duration: row.get("duration"),
            exit: row.get("exit"),
            command: row.get("command"),
            cwd: row.get("cwd"),
            session: row.get("session"),
            hostname: row.get("hostname"),
        }
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
            Self::save_raw(&mut tx, i).await?
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
                set timestamp = ?2, duration = ?3, exit = ?4, command = ?5, cwd = ?6, session = ?7, hostname = ?8
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
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    // make a unique list, that only shows the *newest* version of things
    async fn list(&self, max: Option<usize>, unique: bool) -> Result<Vec<History>> {
        debug!("listing history");

        // very likely vulnerable to SQL injection
        // however, this is client side, and only used by the client, on their
        // own data. They can just open the db file...
        // otherwise building the query is awkward
        let query = format!(
            "select * from history h
                {}
                order by timestamp desc
                {}",
            // inject the unique check
            if unique {
                "where timestamp = (
                        select max(timestamp) from history
                        where h.command = history.command
                    )"
            } else {
                ""
            },
            // inject the limit
            if let Some(max) = max {
                format!("limit {}", max)
            } else {
                "".to_string()
            }
        );

        let res = sqlx::query(query.as_str())
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
        .bind(from)
        .bind(to)
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

    async fn history_count(&self) -> Result<i64> {
        let res: (i64,) = sqlx::query_as("select count(1) from history")
            .fetch_one(&self.pool)
            .await?;

        Ok(res.0)
    }

    async fn search(
        &self,
        limit: Option<i64>,
        search_mode: SearchMode,
        query: &str,
    ) -> Result<Vec<History>> {
        let orig_query = query;
        let query = query.to_string().replace("*", "%"); // allow wildcard char
        let limit = limit.map_or("".to_owned(), |l| format!("limit {}", l));

        let query = match search_mode {
            SearchMode::Prefix => query,
            SearchMode::FullText => format!("%{}", query),
            SearchMode::Fuzzy => query.split("").join("%"),
        };

        let res = sqlx::query(
            format!(
                "select * from history h
            where command like ?1 || '%'
            and timestamp = (
                    select max(timestamp) from history
                    where h.command = history.command
                )
            order by timestamp desc {}",
                limit.clone()
            )
            .as_str(),
        )
        .bind(query)
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
}

#[cfg(test)]
mod test {
    use super::*;

    async fn new_history_item(db: &mut impl Database, cmd: &str) -> Result<()> {
        let history = History::new(
            chrono::Utc::now(),
            cmd.to_string(),
            "/home/ellie".to_string(),
            0,
            1,
            Some("beep boop".to_string()),
            Some("booop".to_string()),
        );
        return db.save(&history).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_prefix() {
        let mut db = Sqlite::new("sqlite::memory:").await.unwrap();
        new_history_item(&mut db, "ls /home/ellie").await.unwrap();

        let mut results = db.search(None, SearchMode::Prefix, "ls").await.unwrap();
        assert_eq!(results.len(), 1);

        results = db.search(None, SearchMode::Prefix, "/home").await.unwrap();
        assert_eq!(results.len(), 0);

        results = db.search(None, SearchMode::Prefix, "ls  ").await.unwrap();
        assert_eq!(results.len(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_fulltext() {
        let mut db = Sqlite::new("sqlite::memory:").await.unwrap();
        new_history_item(&mut db, "ls /home/ellie").await.unwrap();

        let mut results = db.search(None, SearchMode::FullText, "ls").await.unwrap();
        assert_eq!(results.len(), 1);

        results = db
            .search(None, SearchMode::FullText, "/home")
            .await
            .unwrap();
        assert_eq!(results.len(), 1);

        results = db.search(None, SearchMode::FullText, "ls  ").await.unwrap();
        assert_eq!(results.len(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_fuzzy() {
        let mut db = Sqlite::new("sqlite::memory:").await.unwrap();
        new_history_item(&mut db, "ls /home/ellie").await.unwrap();
        new_history_item(&mut db, "ls /home/frank").await.unwrap();
        new_history_item(&mut db, "cd /home/ellie").await.unwrap();
        new_history_item(&mut db, "/home/ellie/.bin/rustup")
            .await
            .unwrap();

        let mut results = db.search(None, SearchMode::Fuzzy, "ls /").await.unwrap();
        assert_eq!(results.len(), 2);

        results = db.search(None, SearchMode::Fuzzy, "l/h/").await.unwrap();
        assert_eq!(results.len(), 2);

        results = db.search(None, SearchMode::Fuzzy, "/h/e").await.unwrap();
        assert_eq!(results.len(), 3);

        results = db.search(None, SearchMode::Fuzzy, "/hmoe/").await.unwrap();
        assert_eq!(results.len(), 0);

        results = db
            .search(None, SearchMode::Fuzzy, "ellie/home")
            .await
            .unwrap();
        assert_eq!(results.len(), 0);

        results = db.search(None, SearchMode::Fuzzy, "lsellie").await.unwrap();
        assert_eq!(results.len(), 1);

        results = db.search(None, SearchMode::Fuzzy, " ").await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_search_reordered_fuzzy() {
        let mut db = Sqlite::new("sqlite::memory:").await.unwrap();
        // test ordering of results: we should choose the first, even though it happened longer ago.

        new_history_item(&mut db, "curl").await.unwrap();
        new_history_item(&mut db, "corburl").await.unwrap();
        // if fuzzy reordering is on, it should come back in a more sensible order
        let mut results = db.search(None, SearchMode::Fuzzy, "curl").await.unwrap();
        assert_eq!(results.len(), 2);
        let commands: Vec<&String> = results.iter().map(|a| &a.command).collect();
        assert_eq!(commands, vec!["curl", "corburl"]);

        results = db.search(None, SearchMode::Fuzzy, "xxxx").await.unwrap();
        assert_eq!(results.len(), 0);

        results = db.search(None, SearchMode::Fuzzy, "").await.unwrap();
        assert_eq!(results.len(), 2);
    }
}
