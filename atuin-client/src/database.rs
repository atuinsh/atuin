use chrono::prelude::*;
use chrono::Utc;
use std::path::Path;

use eyre::{eyre, Result};

use rusqlite::{params, Connection};
use rusqlite::{Params, Transaction};

use super::history::History;

pub trait Database {
    fn save(&mut self, h: &History) -> Result<()>;
    fn save_bulk(&mut self, h: &[History]) -> Result<()>;

    fn load(&self, id: &str) -> Result<History>;
    fn list(&self, max: Option<usize>, unique: bool) -> Result<Vec<History>>;
    fn range(&self, from: chrono::DateTime<Utc>, to: chrono::DateTime<Utc>)
        -> Result<Vec<History>>;

    fn query(&self, query: &str, params: impl Params) -> Result<Vec<History>>;
    fn update(&self, h: &History) -> Result<()>;
    fn history_count(&self) -> Result<i64>;

    fn first(&self) -> Result<History>;
    fn last(&self) -> Result<History>;
    fn before(&self, timestamp: chrono::DateTime<Utc>, count: i64) -> Result<Vec<History>>;

    fn prefix_search(&self, query: &str) -> Result<Vec<History>>;

    fn search(&self, limit: Option<i64>, query: &str) -> Result<Vec<History>>;
}

// Intended for use on a developer machine and not a sync server.
// TODO: implement IntoIterator
pub struct Sqlite {
    conn: Connection,
}

impl Sqlite {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        debug!("opening sqlite database at {:?}", path);

        let create = !path.exists();
        if create {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
        }

        let conn = Connection::open(path)?;

        Self::setup_db(&conn)?;

        Ok(Self { conn })
    }

    fn setup_db(conn: &Connection) -> Result<()> {
        debug!("running sqlite database setup");

        Ok(())
    }

    fn save_raw(tx: &Transaction, h: &History) -> Result<()> {
        tx.execute(
            "insert or ignore into history (
            id,
            timestamp,
            duration,
            exit,
            command,
            cwd,
            session,
            hostname
        ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                h.id,
                h.timestamp.timestamp_nanos(),
                h.duration,
                h.exit,
                h.command,
                h.cwd,
                h.session,
                h.hostname
            ],
        )?;

        Ok(())
    }
}

impl Database for Sqlite {
    fn save(&mut self, h: &History) -> Result<()> {
        debug!("saving history to sqlite");

        let tx = self.conn.transaction()?;
        Self::save_raw(&tx, h)?;
        tx.commit()?;

        Ok(())
    }

    fn save_bulk(&mut self, h: &[History]) -> Result<()> {
        debug!("saving history to sqlite");

        let tx = self.conn.transaction()?;
        for i in h {
            Self::save_raw(&tx, i)?
        }
        tx.commit()?;

        Ok(())
    }

    fn load(&self, id: &str) -> Result<History> {
        debug!("loading history item {}", id);

        let history = self.query(
            "select id, timestamp, duration, exit, command, cwd, session, hostname from history
                where id = ?1 limit 1",
            &[id],
        )?;

        if history.is_empty() {
            return Err(eyre!("could not find history with id {}", id));
        }

        let history = history[0].clone();

        Ok(history)
    }

    fn update(&self, h: &History) -> Result<()> {
        debug!("updating sqlite history");

        self.conn.execute(
            "update history
                set timestamp = ?2, duration = ?3, exit = ?4, command = ?5, cwd = ?6, session = ?7, hostname = ?8
                where id = ?1",
            params![h.id, h.timestamp.timestamp_nanos(), h.duration, h.exit, h.command, h.cwd, h.session, h.hostname],
        )?;

        Ok(())
    }

    // make a unique list, that only shows the *newest* version of things
    fn list(&self, max: Option<usize>, unique: bool) -> Result<Vec<History>> {
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

        let history = self.query(query.as_str(), params![])?;

        Ok(history)
    }

    fn range(
        &self,
        from: chrono::DateTime<Utc>,
        to: chrono::DateTime<Utc>,
    ) -> Result<Vec<History>> {
        debug!("listing history from {:?} to {:?}", from, to);

        let mut stmt = self.conn.prepare(
            "SELECT * FROM history where timestamp >= ?1 and timestamp <= ?2 order by timestamp asc",
        )?;

        let history_iter = stmt.query_map(
            params![from.timestamp_nanos(), to.timestamp_nanos()],
            |row| history_from_sqlite_row(None, row),
        )?;

        Ok(history_iter.filter_map(Result::ok).collect())
    }

    fn first(&self) -> Result<History> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM history order by timestamp asc limit 1")?;

        let history = stmt.query_row(params![], |row| history_from_sqlite_row(None, row))?;

        Ok(history)
    }

    fn last(&self) -> Result<History> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM history where duration >= 0 order by timestamp desc limit 1")?;

        let history = stmt.query_row(params![], |row| history_from_sqlite_row(None, row))?;

        Ok(history)
    }

    fn before(&self, timestamp: chrono::DateTime<Utc>, count: i64) -> Result<Vec<History>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM history where timestamp < ? order by timestamp desc limit ?")?;

        let history_iter = stmt.query_map(params![timestamp.timestamp_nanos(), count], |row| {
            history_from_sqlite_row(None, row)
        })?;

        Ok(history_iter.filter_map(Result::ok).collect())
    }

    fn query(&self, query: &str, params: impl Params) -> Result<Vec<History>> {
        let mut stmt = self.conn.prepare(query)?;

        let history_iter = stmt.query_map(params, |row| history_from_sqlite_row(None, row))?;

        Ok(history_iter.filter_map(Result::ok).collect())
    }

    fn prefix_search(&self, query: &str) -> Result<Vec<History>> {
        let query = query.to_string().replace("*", "%"); // allow wildcard char

        self.query(
            "select * from history h 
                where command like ?1 || '%' 
                and timestamp = (
                    select max(timestamp) from history 
                    where h.command = history.command
                ) 
                order by timestamp desc limit 200",
            &[query.as_str()],
        )
    }

    fn history_count(&self) -> Result<i64> {
        let res: i64 =
            self.conn
                .query_row_and_then("select count(1) from history;", params![], |row| row.get(0))?;

        Ok(res)
    }

    fn search(&self, limit: Option<i64>, query: &str) -> Result<Vec<History>> {
        let limit = limit.map_or("".to_owned(), |l| format!("limit {}", l));

        self.query(
            format!(
                "select * from history
            where command like ?1 || '%' 
            order by timestamp desc {}",
                limit.clone()
            )
            .as_str(),
            &[query],
        )
    }
}

fn history_from_sqlite_row(
    id: Option<String>,
    row: &rusqlite::Row,
) -> Result<History, rusqlite::Error> {
    let id = match id {
        Some(id) => id,
        None => row.get(0)?,
    };

    Ok(History {
        id,
        timestamp: Utc.timestamp_nanos(row.get(1)?),
        duration: row.get(2)?,
        exit: row.get(3)?,
        command: row.get(4)?,
        cwd: row.get(5)?,
        session: row.get(6)?,
        hostname: row.get(7)?,
    })
}
