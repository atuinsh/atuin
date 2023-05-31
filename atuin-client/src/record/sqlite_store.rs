// Here we are using sqlite as a pretty dumb store, and will not be running any complex queries.
// Multiple stores of multiple types are all stored in one chonky table (for now), and we just index
// by tag/host

yo tomorrow morning me
drink that coffee
then wrap up this interface

you will need to 
- make sure the records use string IDs
- add the version in
- write some tests with a memory sqlite

use std::path::Path;
use std::str::FromStr;

use async_trait::async_trait;
use fs_err as fs;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow},
    Result, Row,
};

use atuin_common::record::Record;

use super::store::Store;

pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub async fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();

        debug!("opening sqlite database at {:?}", path);

        let create = !path.exists();
        if create {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
        }

        let opts = SqliteConnectOptions::from_str(path.as_os_str().to_str().unwrap())?
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new().connect_with(opts).await?;

        Self::setup_db(&pool).await?;

        Ok(Self { pool })
    }

    async fn setup_db(pool: &SqlitePool) -> Result<()> {
        debug!("running sqlite database setup");

        sqlx::migrate!("./record-migrations").run(pool).await?;

        Ok(())
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn push(record: Record) -> Result<Record> {
        Ok(record)
    }
}
