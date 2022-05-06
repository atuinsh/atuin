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
use std::sync::RwLock;

use lazy_static::lazy_static;
use chrono::{prelude::*, Utc};
use directories::UserDirs;
use eyre::{eyre, Result};
use sqlx::sqlite::SqlitePool;

use super::Importer;
use crate::history::History;

// Using lazy_static! here is just of a hack. The issue with importing zsh-histdb data is that
// sqlx-rs is fully async, but the Importer trait is not, so it is not possible to call async
// functions fromt this trait. So as a workaround, i'm using lazy_static to hold a vector of
// History Structs, and pre-populate that before the Importer trait is used. Then the Importer
// trait can recall the vector held in lazy_static!.
//

lazy_static! {
    static ref ZSH_HISTDB_VEC: RwLock<Vec<History>> = {
        let m = Vec::new();
        RwLock::new(m)
    };
}

#[derive(sqlx::FromRow, Debug)]
pub struct HistDbHistoryCount {
    pub count: usize
}

#[derive(sqlx::FromRow, Debug)]
pub struct HistDbHistory {
    pub id: i64,
    pub start_time: NaiveDateTime,
    pub host: String,
    pub dir: String,
    pub argv: String,
    pub duration: i64,
}

impl From<HistDbHistory> for History {
    fn from(histdb_item: HistDbHistory) -> Self {
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
    histdb: Vec<History>,
    counter: i64,
}


async fn hist_from_db(dbpath: PathBuf) -> Result<Vec<History>> {
    let pool = SqlitePool::connect(dbpath.to_str().unwrap()).await?;
    let query = format!("select history.id,history.start_time,history.duration,places.host,places.dir,commands.argv from history left join commands on history.command_id = commands.rowid left join places on history.place_id = places.rowid order by history.start_time");
    let histdb_vec : Vec<HistDbHistory> = sqlx::query_as::<_, HistDbHistory>(&query)
            .fetch_all(&pool)
            .await?;
    let hist : Vec<History> = histdb_vec.into_iter().map(|x| x.into()).collect::<Vec<History>>();
    Ok(hist)
}

impl ZshHistDb {

    /// Creates a new ZshHistDb and populates the history based on the pre-populated data
    /// structure.
    pub fn new(_dbpath: PathBuf) -> Result<Self> {
        if let Ok(mut static_zsh_histdb_vec) = ZSH_HISTDB_VEC.write()
        {
            let mut hist_vec = Vec::with_capacity(static_zsh_histdb_vec.len());
            for i in static_zsh_histdb_vec.drain(..) { hist_vec.push(i) }
            Ok(Self {
                histdb: hist_vec,
                counter: 0,
            })
        }
        else {  Err(eyre!("Could not find copy history")) } 
    }

    /// This function is used to pre-populate a vector of readings since the Importer trait is not
    /// async. 
    pub async fn populate(dbpath: PathBuf) {
        if let Ok(mut static_zsh_histdb_vec) = ZSH_HISTDB_VEC.write()
        {
            let mut hist = hist_from_db(dbpath).await.unwrap();
            *static_zsh_histdb_vec = Vec::with_capacity(hist.len());
            for i in hist.drain(..) { static_zsh_histdb_vec.push(i) }
        }
    }

    /// get the number entries already loaded.
    pub fn count() -> usize {
        ZSH_HISTDB_VEC.read().and_then(|x| Ok(x.len())).unwrap_or_else(|_x| 0)
    }
}

impl Importer for ZshHistDb {
    // Not sure how this is used
    const NAME: &'static str = "zsh_histdb";

    fn histpath() -> Result<PathBuf> {

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

    fn parse(path: impl AsRef<Path>) -> Result<Self> {
        Self::new(path.as_ref().to_path_buf())
    }
}

impl Iterator for ZshHistDb {
    type Item = Result<History>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.histdb.pop()
        {
            Some(h) => { self.counter += 1; Some(Ok(h)) }
            None    => { None }
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use std::env;

    #[test]
    fn test_import() {
        let test_db1 = "test_files/zsh-history.db";
        let key = "HISTDB_FILE";
        env::set_var(key, test_db1);

        // test the env got set
        assert_eq!(env::var(key).unwrap(), test_db1.to_string());

        // test histdb returns the proper db from previous step
        let histdb_path = ZshHistDb::histpath();
        assert_eq!(histdb_path.unwrap().to_str().unwrap() , test_db1);

        // test histdb iterator
        let histdb_path = ZshHistDb::histpath();
        let histdb = ZshHistDb::new(histdb_path.unwrap()).unwrap();
        println!("h: {:#?}", histdb.histdb);
        println!("counter: {:?}", histdb.counter);
        for i in histdb {
            println!("{:?}", i);
        }
    }
}
