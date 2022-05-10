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

use std::{
    path::{Path, PathBuf},
};

use chrono::{prelude::*, Utc};
use async_trait::async_trait;
use directories::UserDirs;
use eyre::{eyre, Result};
use sqlx::sqlite::SqlitePool;

use super::Importer;
use crate::history::History;
use crate::import::Loader;

#[derive(sqlx::FromRow, Debug)]
pub struct HistDbEntryCount {
    pub count: usize
}

#[derive(sqlx::FromRow, Debug)]
pub struct HistDbEntry {
    pub id: i64,
    pub start_time: NaiveDateTime,
    pub host: String,
    pub dir: String,
    pub argv: String,
    pub duration: i64,
}

impl From<HistDbEntry> for History {
    fn from(histdb_item: HistDbEntry) -> Self {
        History::new (
            DateTime::from_utc(histdb_item.start_time, Utc), // must assume UTC? 
            histdb_item.argv.trim_end().to_string(),
            histdb_item.dir,
            0, // assume 0, we have no way of knowing :(
            histdb_item.duration,
            None,
            Some(histdb_item.host),
        )
    }
}

#[derive(Debug)]
pub struct ZshHistDb {
    histdb: Vec<HistDbEntry>,
}


/// Read db at given file, return vector of entries.
async fn hist_from_db(dbpath: PathBuf) -> Result<Vec<HistDbEntry>> {
    let pool = SqlitePool::connect(dbpath.to_str().unwrap()).await?;
    let query = format!("select history.id,history.start_time,history.duration,places.host,places.dir,commands.argv from history left join commands on history.command_id = commands.rowid left join places on history.place_id = places.rowid order by history.start_time");
    let histdb_vec : Vec<HistDbEntry> = sqlx::query_as::<_, HistDbEntry>(&query)
            .fetch_all(&pool)
            .await?;
    Ok(histdb_vec)
}

impl ZshHistDb {
    pub fn histpath() -> Result<PathBuf> {

        // By default histdb database is `${HOME}/.histdb/zsh-history.db`
        // This can be modified by ${HISTDB_FILE}
        //
        //  if [[ -z ${HISTDB_FILE} ]]; then
        //      typeset -g HISTDB_FILE="${HOME}/.histdb/zsh-history.db"
        let user_dirs = UserDirs::new().unwrap(); // should catch error here?
        let home_dir = user_dirs.home_dir();
        let histdb_path = std::env::var("HISTDB_FILE")
                            .as_ref()
                            .map(|x| Path::new(x).to_path_buf())
                            .unwrap_or_else(|_err| home_dir.join(".histdb/zsh-history.db"));
        if histdb_path.exists() { Ok(histdb_path) }
        else {  Err(eyre!("Could not find history file. Try setting $HISTDB_FILE")) }
    }
}

#[async_trait]
impl Importer for ZshHistDb {
    // Not sure how this is used
    const NAME: &'static str = "zsh_histdb";

    /// Creates a new ZshHistDb and populates the history based on the pre-populated data
    /// structure.
    async fn new() -> Result<Self> {

        let dbpath = ZshHistDb::histpath()?; 
        let histdb_entry_vec = hist_from_db(dbpath).await?;
        Ok(Self {
            histdb: histdb_entry_vec,
        })
    }
    async fn entries(&mut self) -> Result<usize> {
        Ok(self.histdb.len())
    }
    async fn load(self, h: &mut impl Loader) -> Result<()> {
        for i in self.histdb { h.push(i.into()).await?; }
        Ok(())
    }
}


#[cfg(test)]
mod test {

    use super::*;
    use std::env;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_import() {
        let test_db1 = "test_files/zsh-history.db";
        let key = "HISTDB_FILE";
        env::set_var(key, test_db1);

        // test the env got set
        assert_eq!(env::var(key).unwrap(), test_db1.to_string());

        // test histdb returns the proper db from previous step
        let histdb_path = ZshHistDb::histpath();
        assert_eq!(histdb_path.unwrap().to_str().unwrap() , test_db1);

        // test histdb iterator
        let histdb = ZshHistDb::new().await.unwrap();
        println!("h: {:#?}", histdb.histdb);
        println!("counter: {:?}", histdb.histdb.len());
        for i in histdb.histdb {
            println!("{:?}", i);
        }
    }
}
