// import old shell history from zsh-histdb!
// automatically hoover up all that we can find

// As far as i can tell there are no version numbers in the histdb sqlite DB, so we're going based
// on the schema from 2022-05-01
//
// I have run into some histories that will not import b/c of non UTF-8 characters.
//

//
// An Example sqlite query for hsitdb data:
//
//id|session|command_id|place_id|exit_status|start_time|duration|id|argv|id|host|dir
//
//
//  select
//    history.id,
//    history.start_time,
//    places.host,
//    places.dir,
//    commands.argv
//  from history
//    left join commands on history.command_id = commands.rowid
//    left join places on history.place_id = places.rowid ;
//
// CREATE TABLE history  (id integer primary key autoincrement,
//                       session int,
//                       command_id int references commands (id),
//                       place_id int references places (id),
//                       exit_status int,
//                       start_time int,
//                       duration int);
//

use std::path::{Path, PathBuf};

use chrono::{prelude::*, Utc};
use directories::UserDirs;
use eyre::{eyre, Result};
use rusqlite::{Connection, OpenFlags};

use super::Importer;
use crate::history::History;
use crate::import::Loader;

#[derive(Debug)]
pub struct HistDbEntry {
    pub id: i64,
    pub start_time: NaiveDateTime,
    pub host: Vec<u8>,
    pub dir: Vec<u8>,
    pub argv: Vec<u8>,
    pub duration: i64,
}

impl From<HistDbEntry> for History {
    fn from(histdb_item: HistDbEntry) -> Self {
        History::new(
            DateTime::from_utc(histdb_item.start_time, Utc), // must assume UTC?
            String::from_utf8(histdb_item.argv)
                .unwrap_or_else(|_e| String::from(""))
                .trim_end()
                .to_string(),
            String::from_utf8(histdb_item.dir)
                .unwrap_or_else(|_e| String::from(""))
                .trim_end()
                .to_string(),
            0, // assume 0, we have no way of knowing :(
            histdb_item.duration,
            None,
            Some(
                String::from_utf8(histdb_item.host)
                    .unwrap_or_else(|_e| String::from(""))
                    .trim_end()
                    .to_string(),
            ),
        )
    }
}

#[derive(Debug)]
pub struct ZshHistDb {
    histdb: Vec<HistDbEntry>,
}

/// Read db at given file, return vector of entries.
fn hist_from_db(dbpath: PathBuf) -> rusqlite::Result<Vec<HistDbEntry>> {
    let conn = Connection::open_with_flags(dbpath, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    hist_from_db_conn(conn)
}

fn hist_from_db_conn(conn: Connection) -> rusqlite::Result<Vec<HistDbEntry>> {
    let query = "select history.id,history.start_time,history.duration,places.host,places.dir,commands.argv from history left join commands on history.command_id = commands.rowid left join places on history.place_id = places.rowid order by history.start_time";
    let mut stmt = conn.prepare(query)?;
    let res = stmt.query_map([], |row| {
        Ok(HistDbEntry {
            id: row.get(0)?,
            start_time: row.get(1)?,
            duration: row.get(2)?,
            host: row.get(3)?,
            dir: row.get(4)?,
            argv: row.get(5)?,
        })
    })?;
    res.collect()
}

impl ZshHistDb {
    pub fn histpath_candidate() -> PathBuf {
        // By default histdb database is `${HOME}/.histdb/zsh-history.db`
        // This can be modified by ${HISTDB_FILE}
        //
        //  if [[ -z ${HISTDB_FILE} ]]; then
        //      typeset -g HISTDB_FILE="${HOME}/.histdb/zsh-history.db"
        let user_dirs = UserDirs::new().unwrap(); // should catch error here?
        let home_dir = user_dirs.home_dir();
        std::env::var("HISTDB_FILE")
            .as_ref()
            .map(|x| Path::new(x).to_path_buf())
            .unwrap_or_else(|_err| home_dir.join(".histdb/zsh-history.db"))
    }
    pub fn histpath() -> Result<PathBuf> {
        let histdb_path = ZshHistDb::histpath_candidate();
        if histdb_path.exists() {
            Ok(histdb_path)
        } else {
            Err(eyre!(
                "Could not find history file. Try setting $HISTDB_FILE"
            ))
        }
    }
}

impl Importer for ZshHistDb {
    // Not sure how this is used
    const NAME: &'static str = "zsh_histdb";

    /// Creates a new ZshHistDb and populates the history based on the pre-populated data
    /// structure.
    fn new() -> Result<Self> {
        let dbpath = ZshHistDb::histpath()?;
        let histdb_entry_vec = hist_from_db(dbpath)?;
        Ok(Self {
            histdb: histdb_entry_vec,
        })
    }
    fn entries(&mut self) -> Result<usize> {
        Ok(self.histdb.len())
    }
    fn load(self, h: &mut impl Loader) -> Result<()> {
        for i in self.histdb {
            h.push(i.into())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use std::env;

    #[test]
    fn test_env_vars() {
        let test_env_db = "nonstd-zsh-history.db";
        let key = "HISTDB_FILE";
        env::set_var(key, test_env_db);

        // test the env got set
        assert_eq!(env::var(key).unwrap(), test_env_db.to_string());

        // test histdb returns the proper db from previous step
        let histdb_path = ZshHistDb::histpath_candidate();
        assert_eq!(histdb_path.to_str().unwrap(), test_env_db);
    }

    #[test]
    fn test_import() {
        let conn = Connection::open_in_memory().unwrap();

        // sql dump directly from a test database.
        let db_sql = r#"
        PRAGMA foreign_keys=OFF;
        BEGIN TRANSACTION;
        CREATE TABLE commands (id integer primary key autoincrement, argv text, unique(argv) on conflict ignore);
        INSERT INTO commands VALUES(1,'pwd');
        INSERT INTO commands VALUES(2,'curl google.com');
        INSERT INTO commands VALUES(3,'bash');
        CREATE TABLE places   (id integer primary key autoincrement, host text, dir text, unique(host, dir) on conflict ignore);
        INSERT INTO places VALUES(1,'mbp16.local','/home/noyez');
        CREATE TABLE history  (id integer primary key autoincrement,
                               session int,
                               command_id int references commands (id),
                               place_id int references places (id),
                               exit_status int,
                               start_time int,
                               duration int);
        INSERT INTO history VALUES(1,0,1,1,0,1651497918,1);
        INSERT INTO history VALUES(2,0,2,1,0,1651497923,1);
        INSERT INTO history VALUES(3,0,3,1,NULL,1651497930,NULL);
        DELETE FROM sqlite_sequence;
        INSERT INTO sqlite_sequence VALUES('commands',3);
        INSERT INTO sqlite_sequence VALUES('places',3);
        INSERT INTO sqlite_sequence VALUES('history',3);
        CREATE INDEX hist_time on history(start_time);
        CREATE INDEX place_dir on places(dir);
        CREATE INDEX place_host on places(host);
        CREATE INDEX history_command_place on history(command_id, place_id);
        COMMIT; "#;

        conn.execute(db_sql, []).unwrap();

        // test histdb iterator
        let histdb_vec = hist_from_db_conn(conn).unwrap();
        let histdb = ZshHistDb { histdb: histdb_vec };

        println!("h: {:#?}", histdb.histdb);
        println!("counter: {:?}", histdb.histdb.len());
        for i in histdb.histdb {
            println!("{:?}", i);
        }
    }
}
