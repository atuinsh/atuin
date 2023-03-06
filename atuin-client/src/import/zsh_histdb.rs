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

use async_trait::async_trait;
use chrono::{prelude::*, Utc};
use directories::UserDirs;
use eyre::{bail, Result};
use sqlx::{sqlite::SqlitePool, Pool};

use super::Importer;
use crate::history::History;
use crate::import::Loader;

#[derive(sqlx::FromRow, Debug)]
pub struct HistDbEntryCount {
    pub count: usize,
}

#[derive(sqlx::FromRow, Debug)]
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

#[async_trait]
impl Importer for ZshHistDb {
    // Not sure how this is used
    const NAME: &'static str = "zsh_histdb";

    fn default_source_path() -> Result<PathBuf> {
        let Some(user_dirs) = UserDirs::new() else {
            bail!("could not find user directories");
        };
        let path = user_dirs.home_dir().join(".histdb/zsh-history.db");

        Ok(path)
    }

    /// Creates a new ZshHistDb and populates the history based on the pre-populated data
    /// structure.
    async fn new(source: &Path) -> Result<Self> {
        let histdb_entry_vec = hist_from_db(source).await?;
        Ok(Self {
            histdb: histdb_entry_vec,
        })
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(self.histdb.len())
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        for i in self.histdb {
            h.push(i.into()).await?;
        }
        Ok(())
    }
}

/// Read db at given file, return vector of entries.
async fn hist_from_db(db_path: impl AsRef<Path>) -> Result<Vec<HistDbEntry>> {
    let Some(db_path_str) = db_path.as_ref().to_str() else {
        bail!("database path is not UTF8.");
    };
    let pool = SqlitePool::connect(db_path_str).await?;
    hist_from_db_conn(pool).await
}

async fn hist_from_db_conn(pool: Pool<sqlx::Sqlite>) -> Result<Vec<HistDbEntry>> {
    let query = "select history.id,history.start_time,history.duration,places.host,places.dir,commands.argv from history left join commands on history.command_id = commands.rowid left join places on history.place_id = places.rowid order by history.start_time";
    let histdb_vec: Vec<HistDbEntry> = sqlx::query_as::<_, HistDbEntry>(query)
        .fetch_all(&pool)
        .await?;
    Ok(histdb_vec)
}

#[cfg(test)]
mod test {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_import() {
        let pool: SqlitePool = SqlitePoolOptions::new()
            .min_connections(2)
            .connect(":memory:")
            .await
            .unwrap();

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

        sqlx::query(db_sql).execute(&pool).await.unwrap();

        // test histdb iterator
        let histdb_vec = hist_from_db_conn(pool).await.unwrap();
        let histdb = ZshHistDb { histdb: histdb_vec };

        println!("h: {:#?}", histdb.histdb);
        println!("counter: {:?}", histdb.histdb.len());
        for i in histdb.histdb {
            println!("{i:?}");
        }
    }
}
