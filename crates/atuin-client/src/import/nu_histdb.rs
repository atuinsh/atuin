// import old shell history!
// automatically hoover up all that we can find

use std::path::PathBuf;

use async_trait::async_trait;
use atuin_common::time::OffsetDateTimeExt;
use directories::BaseDirs;
use eyre::{Result, eyre};
use sqlx::{Pool, sqlite::SqlitePool};
use time::OffsetDateTime;

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
        // a corrupt row must not take down the whole import. the epoch sorts to the
        // bottom of history and is obviously not a real time
        let timestamp = OffsetDateTime::from_timespec(i128::from(ts_secs), i128::from(ts_ns))
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);

        let imported = History::import()
            .shell("nu")
            .timestamp(timestamp)
            // nushell stores raw bytes: keep the entry even if it is not valid utf8
            .command(String::from_utf8_lossy(&histdb_item.command_line).into_owned())
            .cwd(String::from_utf8_lossy(&histdb_item.cwd).into_owned())
            .exit(histdb_item.exit_status)
            .duration(histdb_item.duration_ms)
            .session(format!("{:x}", histdb_item.session_id))
            .hostname(String::from_utf8_lossy(&histdb_item.hostname).into_owned());

        imported.build().into()
    }
}

#[derive(Debug)]
pub struct NuHistDb {
    histdb: Vec<HistDbEntry>,
}

/// Read db at given file, return vector of entries.
async fn hist_from_db(dbpath: PathBuf) -> Result<Vec<HistDbEntry>> {
    let connection_str = dbpath.to_str().ok_or_else(|| {
        eyre!(
            "Invalid path for SQLite database: {}",
            dbpath.to_string_lossy()
        )
    })?;
    let pool = SqlitePool::connect(connection_str).await?;
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

#[cfg(test)]
mod test {
    use super::*;

    fn entry(start_timestamp: i64, command_line: Vec<u8>) -> HistDbEntry {
        HistDbEntry {
            id: 1,
            command_line,
            start_timestamp,
            session_id: 1,
            hostname: b"host".to_vec(),
            cwd: b"/tmp".to_vec(),
            duration_ms: 100,
            exit_status: 0,
            more_info: Vec::new(),
        }
    }

    #[test]
    fn valid_timestamp_is_converted() {
        let h: History = entry(1_639_162_832_500, b"echo hello".to_vec()).into();
        assert_eq!(h.timestamp.unix_timestamp(), 1_639_162_832);
        assert_eq!(h.timestamp.nanosecond(), 500_000_000);
        assert_eq!(h.command, "echo hello");
    }

    #[test]
    fn out_of_range_timestamp_falls_back_to_epoch() {
        let h: History = entry(i64::MAX, b"echo hello".to_vec()).into();
        assert_eq!(h.timestamp, OffsetDateTime::UNIX_EPOCH);
        // the command is what matters - it must survive
        assert_eq!(h.command, "echo hello");
    }

    #[test]
    fn invalid_utf8_command_is_kept_lossily() {
        let h: History = entry(0, vec![0x65, 0x63, 0x68, 0x6f, 0xff]).into();
        assert_eq!(h.command, "echo\u{fffd}");
    }
}
