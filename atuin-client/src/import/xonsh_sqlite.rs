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

use super::{Importer, Loader};
use crate::history::History;

#[derive(Debug, FromRow)]
struct HistDbEntry {
    inp: String,
    rtn: Option<i64>,
    tsb: f64,
    tse: f64,
    cwd: String,
    session_start: f64,
}

impl From<HistDbEntry> for History {
    fn from(entry: HistDbEntry) -> Self {
        let ts_nanos = (entry.tsb * 1_000_000_000_f64) as i128;
        let timestamp = OffsetDateTime::from_unix_timestamp_nanos(ts_nanos).unwrap();

        let session_ts_seconds = entry.session_start.trunc() as u64;
        let session_ts_nanos = (entry.session_start.fract() * 1_000_000_000_f64) as u32;
        let session_ts = Timestamp::from_unix(NoContext, session_ts_seconds, session_ts_nanos);
        let session_id = Uuid::new_v7(session_ts).to_string();
        let duration = (entry.tse - entry.tsb) * 1_000_000_000_f64;
        let hostname = get_hostname();

        if let Some(exit) = entry.rtn {
            let imported = History::import()
                .timestamp(timestamp)
                .duration(duration.trunc() as i64)
                .exit(exit)
                .command(entry.inp)
                .cwd(entry.cwd)
                .session(session_id)
                .hostname(hostname);
            imported.build().into()
        } else {
            let imported = History::import()
                .timestamp(timestamp)
                .duration(duration.trunc() as i64)
                .command(entry.inp)
                .cwd(entry.cwd)
                .session(session_id)
                .hostname(hostname);
            imported.build().into()
        }
    }
}

fn get_db_path() -> Result<PathBuf> {
    // if running within xonsh, this will be available
    if let Ok(d) = env::var("XONSH_DATA_DIR") {
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

fn get_hostname() -> String {
    format!(
        "{}:{}",
        env::var("ATUIN_HOST_NAME").unwrap_or_else(|_| whoami::hostname()),
        env::var("ATUIN_HOST_USER").unwrap_or_else(|_| whoami::username()),
    )
}

#[derive(Debug)]
pub struct XonshSqlite {
    pool: SqlitePool,
}

#[async_trait]
impl Importer for XonshSqlite {
    const NAME: &'static str = "xonsh_sqlite";

    async fn new() -> Result<Self> {
        let db_path = get_db_path()?;
        let connection_str = db_path.to_str().ok_or_else(|| {
            eyre!(
                "Invalid path for SQLite database: {}",
                db_path.to_string_lossy()
            )
        })?;

        let pool = SqlitePool::connect(connection_str).await?;
        Ok(XonshSqlite { pool })
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
            loader.push(entry.into()).await?;
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
    fn test_db_path() {
        // similarly to in utils.rs, these need to be run sequentially
        test_db_path_xonsh();
        test_db_path_xdg();
        test_db_path_default();
    }

    fn test_db_path_xonsh() {
        env::set_var("XONSH_DATA_DIR", "/home/user/xonsh_data");
        assert_eq!(
            get_db_path().unwrap(),
            PathBuf::from("/home/user/xonsh_data/xonsh-history.sqlite"),
        );
        env::remove_var("XONSH_DATA_DIR");
    }

    fn test_db_path_xdg() {
        env::set_var("XDG_DATA_HOME", "/home/user/custom_data");
        assert_eq!(
            get_db_path().unwrap(),
            PathBuf::from("/home/user/custom_data/xonsh/xonsh-history.sqlite"),
        );
        env::remove_var("XDG_DATA_HOME");
    }

    fn test_db_path_default() {
        // some other tests need this, so save it for later
        let orig_home = env::var("HOME");

        env::set_var("HOME", "/home/user");
        assert_eq!(
            get_db_path().unwrap(),
            PathBuf::from("/home/user/.local/share/xonsh/xonsh-history.sqlite"),
        );

        if let Ok(v) = orig_home {
            env::set_var("HOME", v);
        } else {
            env::remove_var("HOME");
        }
    }

    #[test]
    fn test_hostname_override() {
        env::set_var("ATUIN_HOST_NAME", "box");
        env::set_var("ATUIN_HOST_USER", "user");
        assert_eq!(get_hostname(), "box:user");
        env::remove_var("ATUIN_HOST_NAME");
        env::remove_var("ATUIN_HOST_USER");
    }

    #[tokio::test]
    async fn test_import() {
        env::set_var("ATUIN_HOST_NAME", "box");
        env::set_var("ATUIN_HOST_USER", "user");

        let connection_str = "tests/data/xonsh-history.sqlite";
        let xonsh_sqlite = XonshSqlite {
            pool: SqlitePool::connect(connection_str).await.unwrap(),
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

        env::remove_var("ATUIN_HOST_NAME");
        env::remove_var("ATUIN_HOST_USER");
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
