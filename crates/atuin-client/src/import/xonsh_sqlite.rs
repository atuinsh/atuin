use std::env;
use std::path::PathBuf;

use async_trait::async_trait;
use atuin_common::db;
use atuin_common::time::OffsetDateTimeExt;
use atuin_domain::record::CmdOrigin;
use easy_cast::{CastFloat, Conv};
use eyre::{Result, eyre};
use futures::TryStreamExt;
use sqlx::sqlite::SqlitePool;
use sqlx::{FromRow, Row};
use time::OffsetDateTime;

use super::{ImportedSessions, Importer, Loader, get_histfile_path};
use crate::history::History;
use crate::history::builder::HistoryImported;

#[derive(Debug, FromRow)]
struct HistDbEntry {
    inp: String,
    rtn: Option<i64>,
    tsb: f64,
    tse: f64,
    cwd: String,
    // Nullable in xonsh's schema; rows without one share a session, as they share a partition.
    sessionid: Option<String>,
    session_start: f64,
}

impl HistDbEntry {
    fn into_hist_with_cmd_origin(
        self,
        cmd_origin: CmdOrigin,
        sessions: &mut ImportedSessions,
    ) -> History {
        let timestamp =
            OffsetDateTime::from_unix_seconds_f64(self.tsb).unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let session_start = OffsetDateTime::from_unix_seconds_f64(self.session_start)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let session_id = sessions.id(self.sessionid.as_deref().unwrap_or_default(), session_start);
        let duration = ((self.tse - self.tsb) * 1_000_000_000_f64)
            .try_cast_trunc()
            .unwrap_or(HistoryImported::DEFAULT_DURATION);

        History::import()
            .shell("xonsh")
            .timestamp(timestamp)
            .duration(duration)
            .exit(self.rtn.unwrap_or(HistoryImported::DEFAULT_EXIT))
            .command(self.inp)
            .cwd(self.cwd)
            .session(session_id)
            .cmd_origin(cmd_origin)
            .build()
            .into()
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
    let data_dir = dirs::data_dir().ok_or_else(|| eyre!("Could not determine data directory"))?;

    let hist_file = data_dir.join("xonsh/xonsh-history.sqlite");
    if hist_file.exists() || cfg!(test) {
        Ok(hist_file)
    } else {
        Err(eyre!("Could not find xonsh history db at: {}", hist_file.to_string_lossy()))
    }
}

#[derive(Debug)]
pub struct XonshSqlite {
    pool: SqlitePool,
    cmd_origin: CmdOrigin,
}

#[async_trait]
impl Importer for XonshSqlite {
    const NAME: &'static str = "xonsh_sqlite";

    async fn new() -> Result<Self> {
        // wrap xonsh-specific path resolver in general one so that it respects $HISTPATH
        let xonsh_data_dir = env::var("XONSH_DATA_DIR").ok();
        let db_path = get_histfile_path(|| xonsh_db_path(xonsh_data_dir))?;
        let connection_str = db_path.to_str().ok_or_else(|| {
            eyre!("Invalid path for SQLite database: {}", db_path.to_string_lossy())
        })?;

        let pool = SqlitePool::connect(connection_str).await?;
        let cmd_origin = CmdOrigin::probe_current();
        Ok(Self { pool, cmd_origin })
    }

    async fn entries(&mut self) -> Result<usize> {
        let query = "SELECT COUNT(*) FROM xonsh_history";
        let row = db::query(query).fetch_one(&self.pool).await?;
        let count: u32 = row.get(0);
        Ok(usize::conv(count))
    }

    async fn load(self, loader: &mut impl Loader) -> Result<()> {
        let query = r"
            SELECT inp, rtn, tsb, tse, cwd, sessionid,
            MIN(tsb) OVER (PARTITION BY sessionid) AS session_start
            FROM xonsh_history
            ORDER BY rowid
        ";

        let mut entries = db::query_as::<_, HistDbEntry>(query).fetch(&self.pool);

        let mut sessions = ImportedSessions::new(Self::NAME);
        let mut count = 0;
        while let Some(entry) = entries.try_next().await? {
            let hist = entry.into_hist_with_cmd_origin(self.cmd_origin.clone(), &mut sessions);
            loader.push(hist).await?;
            count += 1;
        }

        println!("Loaded: {count}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::macros::datetime;

    use super::*;
    use crate::history::History;
    use crate::import::tests::TestLoader;

    #[rstest]
    fn test_db_path_xonsh() {
        let db_path = xonsh_db_path(Some("/home/user/xonsh_data".to_string())).unwrap();
        assert_eq!(db_path, PathBuf::from("/home/user/xonsh_data/xonsh-history.sqlite"));
    }

    #[rstest]
    fn out_of_range_timestamp_falls_back_to_epoch() {
        let entry = HistDbEntry {
            inp: "echo hello".to_string(),
            rtn: Some(0),
            tsb: 1e30,
            tse: 1e30,
            cwd: "/tmp".to_string(),
            sessionid: Some("s".to_string()),
            session_start: 0.0,
        };

        let hist = entry.into_hist_with_cmd_origin(
            CmdOrigin::try_from("box:user").unwrap(),
            &mut ImportedSessions::new(XonshSqlite::NAME),
        );
        assert_eq!(hist.timestamp, OffsetDateTime::UNIX_EPOCH);
        assert_eq!(hist.command, "echo hello");
    }

    async fn import(pool: SqlitePool) -> Vec<History> {
        let xonsh_sqlite = XonshSqlite {
            pool,
            cmd_origin: CmdOrigin::try_from("box:user").unwrap(),
        };

        let mut loader = TestLoader::default();
        xonsh_sqlite.load(&mut loader).await.unwrap();
        loader.buf
    }

    async fn import_fixture() -> Vec<History> {
        import(SqlitePool::connect("tests/data/xonsh-history.sqlite").await.unwrap()).await
    }

    #[rstest]
    #[tokio::test]
    async fn test_import() {
        for (actual, expected) in import_fixture().await.iter().zip(expected_hist_entries().iter())
        {
            assert_eq!(actual.timestamp, expected.timestamp);
            assert_eq!(actual.command, expected.command);
            assert_eq!(actual.cwd, expected.cwd);
            assert_eq!(actual.exit, expected.exit);
            assert_eq!(actual.duration, expected.duration);
            assert_eq!(actual.cmd_origin, expected.cmd_origin);
        }
    }

    /// The fixture holds two xonsh sessions of two commands each, rows 1-2 and 3-4.
    #[rstest]
    #[tokio::test]
    async fn each_session_gets_one_id_that_importing_again_reproduces() {
        let sessions = || async {
            let ids: Vec<String> = import_fixture().await.into_iter().map(|h| h.session).collect();
            <[String; 4]>::try_from(ids).unwrap()
        };
        let [a1, a2, b1, b2] = sessions().await;

        assert_eq!(a1, a2);
        assert_eq!(b1, b2);
        assert_ne!(a1, b1);
        assert_eq!(sessions().await, [a1, a2, b1, b2]);
    }

    #[rstest]
    #[tokio::test]
    async fn a_row_without_a_session_id_still_imports() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        db::query(
            "CREATE TABLE xonsh_history (inp TEXT, rtn INTEGER, tsb REAL, tse REAL, sessionid \
             TEXT, cwd TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        db::query(
            "INSERT INTO xonsh_history VALUES ('echo hi', 0, 1707242181.0, 1707242182.0, NULL, \
             '/tmp')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let imported = import(pool).await;

        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].command, "echo hi");
    }

    fn expected_hist_entries() -> [History; 4] {
        [
            History::import()
                .timestamp(datetime!(2024-02-6 17:56:21.130956173 +00:00:00))
                .command("echo hello world!".to_string())
                .cwd("/home/user/Documents/code/atuin".to_string())
                .exit(0)
                .duration(2_628_564)
                .cmd_origin(CmdOrigin::try_from("box:user").unwrap())
                .build()
                .into(),
            History::import()
                .timestamp(datetime!(2024-02-06 17:56:28.190406084 +00:00:00))
                .command("ls -l".to_string())
                .cwd("/home/user/Documents/code/atuin".to_string())
                .exit(0)
                .duration(9_371_519)
                .cmd_origin(CmdOrigin::try_from("box:user").unwrap())
                .build()
                .into(),
            History::import()
                .timestamp(datetime!(2024-02-06 17:56:46.989020824 +00:00:00))
                .command("false".to_string())
                .cwd("/home/user/Documents/code/atuin".to_string())
                .exit(1)
                .duration(17_337_560)
                .cmd_origin(CmdOrigin::try_from("box:user").unwrap())
                .build()
                .into(),
            History::import()
                .timestamp(datetime!(2024-02-06 17:56:48.218384027 +00:00:00))
                .command("exit".to_string())
                .cwd("/home/user/Documents/code/atuin".to_string())
                .exit(0)
                .duration(4_599_094)
                .cmd_origin(CmdOrigin::try_from("box:user").unwrap())
                .build()
                .into(),
        ]
    }
}
