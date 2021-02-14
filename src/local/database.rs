use std::path::Path;

use eyre::{eyre, Result};

use rusqlite::NO_PARAMS;
use rusqlite::{params, Connection};

use crate::History;

pub trait Database {
    fn save(&mut self, h: History) -> Result<()>;
    fn save_bulk(&mut self, h: &[History]) -> Result<()>;
    fn load(&self, id: &str) -> Result<History>;
    fn list(&self, distinct: bool) -> Result<()>;
    fn update(&self, h: History) -> Result<()>;
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
}

impl Database for Sqlite {
    fn save(&mut self, h: History) -> Result<()> {
        debug!("saving history to sqlite");
        let v = vec![h];

        self.save_bulk(&v)
    }

    fn save_bulk(&mut self, h: &[History]) -> Result<()> {
        debug!("saving history to sqlite");

        let tx = self.conn.transaction()?;

        for i in h {
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
                    i.id,
                    i.timestamp,
                    i.duration,
                    i.exit,
                    i.command,
                    i.cwd,
                    i.session,
                    i.hostname
                ],
            )?;
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

        let mut iter = stmt.query_map(params![id], |row| {
            Ok(History {
                id: String::from(id),
                timestamp: row.get(1)?,
                duration: row.get(2)?,
                exit: row.get(3)?,
                command: row.get(4)?,
                cwd: row.get(5)?,
                session: row.get(6)?,
                hostname: row.get(7)?,
            })
        })?;

        let history = iter.next().unwrap();

        match history {
            Ok(i) => Ok(i),
            Err(e) => Err(eyre!("could not find item: {}", e)),
        }
    }

    fn update(&self, h: History) -> Result<()> {
        debug!("updating sqlite history");

        self.conn.execute(
            "update history
                set timestamp = ?2, duration = ?3, exit = ?4, command = ?5, cwd = ?6, session = ?7, hostname = ?8
                where id = ?1",
            params![h.id, h.timestamp, h.duration, h.exit, h.command, h.cwd, h.session, h.hostname],
        )?;

        Ok(())
    }

    fn list(&self, distinct: bool) -> Result<()> {
        debug!("listing history");

        let mut stmt = if distinct {
            self.conn
                .prepare("SELECT command FROM history order by timestamp asc")?
        } else {
            self.conn
                .prepare("SELECT distinct command FROM history order by timestamp asc")?
        };

        let history_iter = stmt.query_map(params![], |row| {
            let command: String = row.get(0)?;

            Ok(command)
        })?;

        for h in history_iter {
            let h = h.unwrap();

            println!("{}", h);
        }

        Ok(())
    }
}
