// import old shell history!
// automatically hoover up all that we can find

use std::path::PathBuf;

use async_trait::async_trait;
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
        // A nushell history row is a set of sqlite blobs: the timestamp is an
        // arbitrary i64 (milliseconds) and command_line/cwd/hostname are raw
        // bytes that may not be valid UTF-8 (e.g. a path or command containing
        // non-UTF-8 bytes, which is legal on POSIX). Neither
        // from_unix_timestamp_nanos nor from_utf8 is guaranteed to succeed, so
        // unwrapping them here made a single bad row abort the entire
        // 'atuin import nu-histdb' run. Widen to i128 nanoseconds (which cannot
        // overflow for any i64 millisecond value) so the whole timestamp is
        // range-checked at once, clamp an out-of-range value to the epoch, and
        // decode the byte fields lossily, so importing keeps the row instead of
        // panicking.
        let ts_nanos = i128::from(histdb_item.start_timestamp) * 1_000_000;
        let timestamp = OffsetDateTime::from_unix_timestamp_nanos(ts_nanos)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let imported = History::import()
            .timestamp(timestamp)
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

#[cfg(test)]
mod test {
    use time::PrimitiveDateTime;

    use super::*;

    #[test]
    fn from_entry_survives_non_utf8_and_out_of_range_timestamp() {
        // 0xc3 0x28 is an invalid UTF-8 sequence (0xc3 starts a two-byte
        // sequence but 0x28 is not a continuation byte). i64::MAX
        // milliseconds is far past OffsetDateTime's supported range, so
        // from_unix_timestamp_nanos returns Err. Both used to unwrap and
        // panic the whole import.
        let entry = HistDbEntry {
            id: 1,
            command_line: vec![b'l', b's', b' ', 0xc3, 0x28],
            start_timestamp: i64::MAX,
            session_id: 0x1a2b,
            hostname: vec![b'h', 0xff],
            cwd: vec![b'/', 0xfe],
            duration_ms: 42,
            exit_status: 0,
            more_info: Vec::new(),
        };

        let history: History = entry.into();

        assert_eq!(history.command, "ls \u{fffd}(");
        assert_eq!(history.cwd, "/\u{fffd}");
        assert_eq!(history.hostname, "h\u{fffd}");
        assert_eq!(history.session, "1a2b");
        assert_eq!(history.duration, 42);
        assert_eq!(history.timestamp, OffsetDateTime::UNIX_EPOCH);
    }

    #[test]
    fn from_entry_clamps_timestamp_just_below_supported_range() {
        // One millisecond below OffsetDateTime's minimum. The previous fix
        // split the millisecond value into whole seconds plus a nanosecond
        // remainder and *added* them: here the seconds truncate to exactly
        // the in-range minimum while the -1ms remainder pushes the sum below
        // it, panicking in `OffsetDateTime + Duration`. It must clamp to the
        // epoch instead of panicking.
        let min_secs = PrimitiveDateTime::MIN.assume_utc().unix_timestamp();
        let entry = HistDbEntry {
            id: 3,
            command_line: b"echo hi".to_vec(),
            start_timestamp: min_secs * 1000 - 1,
            session_id: 1,
            hostname: b"host".to_vec(),
            cwd: b"/".to_vec(),
            duration_ms: 1,
            exit_status: 0,
            more_info: Vec::new(),
        };

        let history: History = entry.into();

        assert_eq!(history.command, "echo hi");
        assert_eq!(history.timestamp, OffsetDateTime::UNIX_EPOCH);
    }

    #[test]
    fn from_entry_decodes_valid_row() {
        let entry = HistDbEntry {
            id: 2,
            command_line: b"cargo test".to_vec(),
            start_timestamp: 1_613_322_469_000,
            session_id: 255,
            hostname: b"host".to_vec(),
            cwd: b"/home/user".to_vec(),
            duration_ms: 1000,
            exit_status: 0,
            more_info: Vec::new(),
        };

        let history: History = entry.into();

        assert_eq!(history.command, "cargo test");
        assert_eq!(history.cwd, "/home/user");
        assert_eq!(history.hostname, "host");
        assert_eq!(history.session, "ff");
        assert_eq!(
            history.timestamp,
            OffsetDateTime::from_unix_timestamp(1_613_322_469).unwrap()
        );
    }
}
