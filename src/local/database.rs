use std::path::Path;

use eyre::Result;
use shellexpand;

use rusqlite::{params, Connection};
use rusqlite::NO_PARAMS;

use super::history::History;

pub trait Database {
    fn save(&self, h: History) -> Result<()>;
    fn list(&self) -> Result<()>;
}

// Intended for use on a developer machine and not a sync server.
// TODO: implement IntoIterator
pub struct SqliteDatabase {
    conn: Connection,
}

impl SqliteDatabase{
    pub fn new(path: &str) -> Result<SqliteDatabase> {
        let path = shellexpand::full(path)?;
        let path = path.as_ref();

        debug!("opening sqlite database at {:?}", path);

        let create = !Path::new(path).exists();
        let conn = Connection::open(path)?;

        if create {
            Self::setup_db(&conn)?;
        }

        Ok(SqliteDatabase{
            conn: conn,
        })
    }

    fn setup_db(conn: &Connection) -> Result<()> {
        debug!("running sqlite database setup");

        conn.execute(
            "create table if not exists history (
                 id integer primary key,
                 timestamp integer not null,
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
                timestamp, 
                command,
                cwd
             ) values (?1, ?2, ?3)", 
            params![h.timestamp, h.command, h.cwd])?;

        Ok(())
    }

    fn list(&self) -> Result<()> {
        debug!("listing history");
    
        let mut stmt = self.conn.prepare("SELECT timestamp, command, cwd FROM history")?;
        let history_iter = stmt.query_map(params![], |row| {
            Ok(History {
                timestamp: row.get(0)?,
                command: row.get(1)?,
                cwd: row.get(2)?,
            })
        })?;

        for h in history_iter {
            let h = h.unwrap();

            println!("{}:{}:{}", h.timestamp, h.cwd, h.command);
        }

        Ok(())
    }
}
