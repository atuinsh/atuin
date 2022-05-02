// import old shell history from zsh-histdb!
// automatically hoover up all that we can find

// As far as i can tell there are no version numbers in the histdb sqlite DB, so we're going based
// on the schema from 2022-05-01

//
//select * from history left join commands on history.command_id = commands.rowid left join places on history.place_id = places.rowid limit 10;
//
//id|session|command_id|place_id|exit_status|start_time|duration|id|argv|id|host|dir
//
//
// select history.id,history.start_time,places.host,places.dir,commands.argv from history left join commands on history.command_id = commands.rowid left join places on history.place_id = places.rowid ;
//
// CREATE TABLE history  (id integer primary key autoincrement,
//                       session int,
//                       command_id int references commands (id),
//                       place_id int references places (id),
//                       exit_status int,
//                       start_time int,
//                       duration int);

use std::{
    path::{Path, PathBuf},
};

use chrono::{prelude::*, Utc};
use directories::UserDirs;
use eyre::{eyre, Result};
use sqlx::sqlite::SqlitePool;

use super::Importer;
use crate::history::History;

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

impl ZshHistDb {
    //fn new<P: AsRef<Path>>(dbpath: P) -> Result<Self> {
    fn new(dbpath: PathBuf) -> Result<Self> {
        // Create the runtime
        //let rt  = tokio::runtime::Runtime::new().unwrap();
        let handle = tokio::runtime::Handle::current();

        // Execute the future, blocking the current thread until completion
        let histdb_vec :Vec<HistDbHistory> = handle.block_on(async {
            let pool = SqlitePool::connect(dbpath.to_str().unwrap()).await?;
            let query = format!("select history.id,history.start_time,history.duration,places.host,places.dir,commands.argv from history left join commands on history.command_id = commands.rowid left join places on history.place_id = places.rowid");
            sqlx::query_as::<_, HistDbHistory>(&query)
                    .fetch_all(&pool)
                    .await
        }).unwrap();
        
        let hist : Vec<History> = histdb_vec.into_iter().map(|x| x.into()).collect::<Vec<History>>();
        Ok(Self {
            histdb: hist,
            counter: 0,
        })
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

// This was a debug function 
pub async fn _print_db() -> Result<()> {
    let db_path = ZshHistDb::histpath().unwrap();
    let pool = SqlitePool::connect(db_path.to_str().unwrap()).await?;
    let query = format!("select history.id,history.start_time,places.host,places.dir,commands.argv from history left join commands on history.command_id = commands.rowid left join places on history.place_id = places.rowid");
    //db.query_history(&query).await?;
    let a = sqlx::query_as::<_, HistDbHistory>(&query)
            .fetch_one(&pool)
            .await?;
    println!("{:?}", a);
    Ok(())
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
