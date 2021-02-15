use chrono::Utc;
use std::path::Path;

use eyre::Result;

use rusqlite::{params, Connection};
use rusqlite::{Transaction, NO_PARAMS};

use super::history::History;

pub enum QueryParam {
    Text(String),
}

pub trait Database {
    fn save(&mut self, h: &History) -> Result<()>;
    fn save_bulk(&mut self, h: &[History]) -> Result<()>;
    fn load(&self, id: &str) -> Result<History>;
    fn list(&self) -> Result<Vec<History>>;
    fn range(&self, from: chrono::DateTime<Utc>, to: chrono::DateTime<Utc>)
        -> Result<Vec<History>>;
    fn update(&self, h: &History) -> Result<()>;
    fn query(&self, query: &str, params: &[QueryParam]) -> Result<Vec<History>>;
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

        if create {
            Self::setup_db(&conn)?;
        }

        Ok(Self { conn })
    }

    fn setup_db(conn: &Connection) -> Result<()> {
        debug!("running sqlite database setup");

        conn.execute(
            "create table if not exists history (
                id text primary key,
                timestamp integer not null,
                duration integer not null,
                exit integer not null,
                command text not null,
                cwd text not null,
                session text not null,
                hostname text not null,

                unique(timestamp, cwd, command)
            )",
            NO_PARAMS,
        )?;

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
                h.timestamp,
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

impl rusqlite::ToSql for QueryParam {
    fn to_sql(&self) -> Result<rusqlite::types::ToSqlOutput<'_>, rusqlite::Error> {
        use rusqlite::types::{ToSqlOutput, Value};

        match self {
            QueryParam::Text(s) => Ok(ToSqlOutput::Owned(Value::Text(s.clone()))),
        }
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
        debug!("loading history item");

        let mut stmt = self.conn.prepare(
            "select id, timestamp, duration, exit, command, cwd, session, hostname from history
                where id = ?1",
        )?;

        let history = stmt.query_row(params![id], |row| {
            history_from_sqlite_row(Some(id.to_string()), row)
        })?;

        Ok(history)
    }

    fn update(&self, h: &History) -> Result<()> {
        debug!("updating sqlite history");

        self.conn.execute(
            "update history
                set timestamp = ?2, duration = ?3, exit = ?4, command = ?5, cwd = ?6, session = ?7, hostname = ?8
                where id = ?1",
            params![h.id, h.timestamp, h.duration, h.exit, h.command, h.cwd, h.session, h.hostname],
        )?;

        Ok(())
    }

    fn list(&self) -> Result<Vec<History>> {
        debug!("listing history");

        let mut stmt = self
            .conn
            .prepare("SELECT * FROM history order by timestamp asc")?;

        let history_iter = stmt.query_map(params![], |row| history_from_sqlite_row(None, row))?;

        Ok(history_iter.filter_map(Result::ok).collect())
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

    fn query(&self, query: &str, params: &[QueryParam]) -> Result<Vec<History>> {
        let mut stmt = self.conn.prepare(query)?;

        let history_iter = stmt.query_map(params, |row| history_from_sqlite_row(None, row))?;

        Ok(history_iter.filter_map(Result::ok).collect())
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
        timestamp: row.get(1)?,
        duration: row.get(2)?,
        exit: row.get(3)?,
        command: row.get(4)?,
        cwd: row.get(5)?,
        session: row.get(6)?,
        hostname: row.get(7)?,
    })
}
