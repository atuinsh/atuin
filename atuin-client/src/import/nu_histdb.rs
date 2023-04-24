// import old shell history!
// automatically hoover up all that we can find

use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{prelude::*, Utc};
use directories::BaseDirs;
use eyre::{eyre, Result};
use sqlx::{sqlite::SqlitePool, Pool};

use super::Importer;
use crate::history::History;
use crate::import::Loader;

#[derive(sqlx::FromRow, Debug)]
pub struct HistDbEntry {
    pub id: i64,
    pub command_line: Vec<u8>,
    pub start_timestamp: i64,
    pub session_id: i64,
    pub hostname: Vec<u8>,
    pub cwd: Vec<u8>,
    pub duration_ms: i64,
    pub exit_status: i64,
    pub more_info: Vec<u8>,
}

impl From<HistDbEntry> for History {
    fn from(histdb_item: HistDbEntry) -> Self {
        let ts_secs = histdb_item.start_timestamp / 1000;
        let ts_ns = (histdb_item.start_timestamp % 1000) * 1_000_000;
        let imported = History::import()
            .timestamp(DateTime::from_utc(
                NaiveDateTime::from_timestamp(ts_secs, ts_ns as u32),
                Utc,
            ))
            .command(String::from_utf8(histdb_item.command_line).unwrap())
            .cwd(String::from_utf8(histdb_item.cwd).unwrap())
            .exit(histdb_item.exit_status)
            .duration(histdb_item.duration_ms)
            .session(format!("{:x}", histdb_item.session_id))
            .hostname(String::from_utf8(histdb_item.hostname).unwrap());

        imported.build().into()
    }
}

#[derive(Debug)]
pub struct NuHistDb {
    histdb: Vec<HistDbEntry>,
}

/// Read db at given file, return vector of entries.
async fn hist_from_db(dbpath: PathBuf) -> Result<Vec<HistDbEntry>> {
    let pool = SqlitePool::connect(dbpath.to_str().unwrap()).await?;
    hist_from_db_conn(pool).await
}

async fn hist_from_db_conn(pool: Pool<sqlx::Sqlite>) -> Result<Vec<HistDbEntry>> {
    let query = r#"
        SELECT
            id, command_line, start_timestamp, session_id, hostname, cwd, duration_ms, exit_status,
            more_info
        FROM history
        ORDER BY start_timestamp
    "#;
    let histdb_vec: Vec<HistDbEntry> = sqlx::query_as::<_, HistDbEntry>(query)
        .fetch_all(&pool)
        .await?;
    Ok(histdb_vec)
}

impl NuHistDb {
    pub fn histpath() -> Result<PathBuf> {
        let base = BaseDirs::new().ok_or_else(|| eyre!("could not determine data directory"))?;
        let config_dir = base.config_dir().join("nushell");

        let histdb_path = config_dir.join("history.sqlite3");
        if histdb_path.exists() {
            Ok(histdb_path)
        } else {
            Err(eyre!("Could not find history file."))
        }
    }
}

#[async_trait]
impl Importer for NuHistDb {
    // Not sure how this is used
    const NAME: &'static str = "nu_histdb";

    /// Creates a new NuHistDb and populates the history based on the pre-populated data
    /// structure.
    async fn new() -> Result<Self> {
        let dbpath = NuHistDb::histpath()?;
        let histdb_entry_vec = hist_from_db(dbpath).await?;
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
