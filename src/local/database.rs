use std::path::Path;

use eyre::{eyre, Result};

use rusqlite::NO_PARAMS;
use rusqlite::{params, Connection};

use crate::History;

pub trait Database {
    fn save(&self, h: History) -> Result<()>;
    fn load(&self, id: &str) -> Result<History>;
    fn list(&self) -> Result<()>;
    fn update(&self, h: History) -> Result<()>;
}

// Intended for use on a developer machine and not a sync server.
// TODO: implement IntoIterator
pub struct SqliteDatabase {
    conn: Connection,
}

impl SqliteDatabase {
    pub fn new(path: impl AsRef<Path>) -> Result<SqliteDatabase> {
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

        Ok(SqliteDatabase { conn })
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
                cwd text not null
            )",
            NO_PARAMS,
        )?;

        Ok(())
    }
}

impl Database for SqliteDatabase {
    fn save(&self, h: History) -> Result<()> {
        debug!("saving history to sqlite");

        self.conn.execute(
            "insert into history (
                id,
                timestamp,
                duration,
                exit,
                command,
                cwd
            ) values (?1, ?2, ?3, ?4, ?5, ?6)",
            params![h.id, h.timestamp, h.duration, h.exit, h.command, h.cwd],
        )?;

        Ok(())
    }

    fn load(&self, id: &str) -> Result<History> {
        debug!("loading history item");

        let mut stmt = self.conn.prepare(
            "select id, timestamp, duration, exit, command, cwd from history
                where id = ?1",
        )?;

        let iter = stmt.query_map(params![id], |row| {
            Ok(History {
                id: String::from(id),
                timestamp: row.get(1)?,
                duration: row.get(2)?,
                exit: row.get(3)?,
                command: row.get(4)?,
                cwd: row.get(5)?,
            })
        })?;

        for i in iter {
            return Ok(i.unwrap());
        }

        return Err(eyre!("Failed to fetch history: {}", id));
    }

    fn update(&self, h: History) -> Result<()> {
        debug!("updating sqlite history");

        self.conn.execute(
            "update history
                set timestamp = ?2, duration = ?3, exit = ?4, command = ?5, cwd = ?6
                where id = ?1",
            params![h.id, h.timestamp, h.duration, h.exit, h.command, h.cwd],
        )?;

        Ok(())
    }

    fn list(&self) -> Result<()> {
        debug!("listing history");

        let mut stmt = self
            .conn
            .prepare("SELECT id, timestamp, duration, exit, command, cwd FROM history")?;

        let history_iter = stmt.query_map(params![], |row| {
            Ok(History {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                duration: row.get(2)?,
                exit: row.get(3)?,
                command: row.get(4)?,
                cwd: row.get(5)?,
            })
        })?;

        for h in history_iter {
            let h = h.unwrap();

            println!(
                "{} | {} | {} | {} | {}",
                h.timestamp, h.cwd, h.duration, h.exit, h.command
            );
        }

        Ok(())
    }
}
