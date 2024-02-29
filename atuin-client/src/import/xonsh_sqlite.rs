use std::env;
use std::path::PathBuf;

use async_trait::async_trait;
use directories::BaseDirs;
use eyre::{eyre, Result};
use futures::TryStreamExt;
use sqlx::{sqlite::SqlitePool, FromRow, Row};
use time::OffsetDateTime;
use uuid::timestamp::{context::NoContext, Timestamp};
use uuid::Uuid;

use super::{get_histpath, Importer, Loader};
use crate::history::History;
use crate::utils::get_host_user;

#[derive(Debug, FromRow)]
struct HistDbEntry {
    inp: String,
    rtn: Option<i64>,
    tsb: f64,
    tse: f64,
    cwd: String,
    session_start: f64,
}

impl HistDbEntry {
    fn into_hist_with_hostname(self, hostname: String) -> History {
        let ts_nanos = (self.tsb * 1_000_000_000_f64) as i128;
        let timestamp = OffsetDateTime::from_unix_timestamp_nanos(ts_nanos).unwrap();

        let session_ts_seconds = self.session_start.trunc() as u64;
        let session_ts_nanos = (self.session_start.fract() * 1_000_000_000_f64) as u32;
        let session_ts = Timestamp::from_unix(NoContext, session_ts_seconds, session_ts_nanos);
        let session_id = Uuid::new_v7(session_ts).to_string();
        let duration = (self.tse - self.tsb) * 1_000_000_000_f64;

        if let Some(exit) = self.rtn {
            let imported = History::import()
                .timestamp(timestamp)
                .duration(duration.trunc() as i64)
                .exit(exit)
                .command(self.inp)
                .cwd(self.cwd)
                .session(session_id)
                .hostname(hostname);
            imported.build().into()
        } else {
            let imported = History::import()
                .timestamp(timestamp)
                .duration(duration.trunc() as i64)
                .command(self.inp)
                .cwd(self.cwd)
                .session(session_id)
                .hostname(hostname);
            imported.build().into()
        }
    }
}

fn xonsh_db_path(xonsh_data_dir: Option<String>) -> Result<PathBuf> {
    // if running within xonsh, this will be available
    if let Some(d) = xonsh_data_dir {
        let mut path = PathBuf::from(d);
        path.push("xonsh-history.sqlite");
        return Ok(path);
    }

    // otherwise, fall back to default
    let base = BaseDirs::new().ok_or_else(|| eyre!("Could not determine home directory"))?;

    let hist_file = base.data_dir().join("xonsh/xonsh-history.sqlite");
    if hist_file.exists() || cfg!(test) {
        Ok(hist_file)
    } else {
        Err(eyre!(
            "Could not find xonsh history db at: {}",
            hist_file.to_string_lossy()
        ))
    }
}

#[derive(Debug)]
pub struct XonshSqlite {
    pool: SqlitePool,
    hostname: String,
}

#[async_trait]
impl Importer for XonshSqlite {
    const NAME: &'static str = "xonsh_sqlite";

    async fn new() -> Result<Self> {
        // wrap xonsh-specific path resolver in general one so that it respects $HISTPATH
        let xonsh_data_dir = env::var("XONSH_DATA_DIR").ok();
        let db_path = get_histpath(|| xonsh_db_path(xonsh_data_dir))?;
        let connection_str = db_path.to_str().ok_or_else(|| {
            eyre!(
                "Invalid path for SQLite database: {}",
                db_path.to_string_lossy()
            )
        })?;

        let pool = SqlitePool::connect(connection_str).await?;
        let hostname = get_host_user();
        Ok(XonshSqlite { pool, hostname })
    }

    async fn entries(&mut self) -> Result<usize> {
        let query = "SELECT COUNT(*) FROM xonsh_history";
        let row = sqlx::query(query).fetch_one(&self.pool).await?;
        let count: u32 = row.get(0);
        Ok(count as usize)
    }

    async fn load(self, loader: &mut impl Loader) -> Result<()> {
        let query = r#"
            SELECT inp, rtn, tsb, tse, cwd,
            MIN(tsb) OVER (PARTITION BY sessionid) AS session_start
            FROM xonsh_history
            ORDER BY rowid
        "#;

        let mut entries = sqlx::query_as::<_, HistDbEntry>(query).fetch(&self.pool);

        let mut count = 0;
        while let Some(entry) = entries.try_next().await? {
            let hist = entry.into_hist_with_hostname(self.hostname.clone());
            loader.push(hist).await?;
            count += 1;
        }

        println!("Loaded: {count}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    use crate::history::History;
    use crate::import::tests::TestLoader;

    #[test]
    fn test_db_path_xonsh() {
        let db_path = xonsh_db_path(Some("/home/user/xonsh_data".to_string())).unwrap();
        assert_eq!(
            db_path,
            PathBuf::from("/home/user/xonsh_data/xonsh-history.sqlite")
        );
    }

    #[tokio::test]
    async fn test_import() {
        let connection_str = "tests/data/xonsh-history.sqlite";
        let xonsh_sqlite = XonshSqlite {
            pool: SqlitePool::connect(connection_str).await.unwrap(),
            hostname: "box:user".to_string(),
        };

        let mut loader = TestLoader::default();
        xonsh_sqlite.load(&mut loader).await.unwrap();

        for (actual, expected) in loader.buf.iter().zip(expected_hist_entries().iter()) {
            assert_eq!(actual.timestamp, expected.timestamp);
            assert_eq!(actual.command, expected.command);
            assert_eq!(actual.cwd, expected.cwd);
            assert_eq!(actual.exit, expected.exit);
            assert_eq!(actual.duration, expected.duration);
            assert_eq!(actual.hostname, expected.hostname);
        }
    }

    fn expected_hist_entries() -> [History; 4] {
        [
            History::import()
                .timestamp(datetime!(2024-02-6 17:56:21.130956288 +00:00:00))
                .command("echo hello world!".to_string())
                .cwd("/home/user/Documents/code/atuin".to_string())
                .exit(0)
                .duration(2628564)
                .hostname("box:user".to_string())
                .build()
                .into(),
            History::import()
                .timestamp(datetime!(2024-02-06 17:56:28.190406144 +00:00:00))
                .command("ls -l".to_string())
                .cwd("/home/user/Documents/code/atuin".to_string())
                .exit(0)
                .duration(9371519)
                .hostname("box:user".to_string())
                .build()
                .into(),
            History::import()
                .timestamp(datetime!(2024-02-06 17:56:46.989020928 +00:00:00))
                .command("false".to_string())
                .cwd("/home/user/Documents/code/atuin".to_string())
                .exit(1)
                .duration(17337560)
                .hostname("box:user".to_string())
                .build()
                .into(),
            History::import()
                .timestamp(datetime!(2024-02-06 17:56:48.218384128 +00:00:00))
                .command("exit".to_string())
                .cwd("/home/user/Documents/code/atuin".to_string())
                .exit(0)
                .duration(4599094)
                .hostname("box:user".to_string())
                .build()
                .into(),
        ]
    }
}
