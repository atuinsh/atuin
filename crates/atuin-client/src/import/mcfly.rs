use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use atuin_common::utils::uuid_v7;
use directories::UserDirs;
use eyre::{Result, eyre};
use sqlx::sqlite::SqlitePool;
use time::OffsetDateTime;

use super::{Importer, Loader};
use crate::history::History;
use crate::utils::{get_hostname, get_username};

#[derive(sqlx::FromRow, Debug)]
struct McflyCommand {
    cmd: String,
    session_id: String,
    when_run: i64,
    exit_code: i64,
    dir: String,
}

#[derive(Debug)]
pub struct Mcfly {
    entries: Vec<History>,
}

impl Mcfly {
    /// Find mcfly database path following mcfly's logic
    pub fn histpath() -> Result<PathBuf> {
        // Check for legacy path first (~/.mcfly/history.db)
        if let Some(user_dirs) = UserDirs::new() {
            let legacy_path = user_dirs.home_dir().join(".mcfly").join("history.db");
            if legacy_path.exists() {
                return Ok(legacy_path);
            }
        }

        // Use XDG data directory on Linux/Unix
        if let Ok(xdg_data_home) = std::env::var("XDG_DATA_HOME") {
            let path = PathBuf::from(xdg_data_home)
                .join("mcfly")
                .join("history.db");
            if path.exists() {
                return Ok(path);
            }
        }

        // Default XDG location (~/.local/share/mcfly/history.db)
        if let Some(user_dirs) = UserDirs::new() {
            let default_path = user_dirs
                .home_dir()
                .join(".local")
                .join("share")
                .join("mcfly")
                .join("history.db");
            if default_path.exists() {
                return Ok(default_path);
            }
        }

        // macOS
        if cfg!(target_os = "macos")
            && let Some(user_dirs) = UserDirs::new()
        {
            let macos_path = user_dirs
                .home_dir()
                .join("Library")
                .join("Application Support")
                .join("McFly")
                .join("history.db");
            if macos_path.exists() {
                return Ok(macos_path);
            }
        }

        // Windows
        if cfg!(target_os = "windows")
            && let Ok(local_data) = std::env::var("LOCALAPPDATA")
        {
            let windows_path = PathBuf::from(local_data)
                .join("McFly")
                .join("data")
                .join("history.db");
            if windows_path.exists() {
                return Ok(windows_path);
            }
        }

        Err(eyre!(
            "Could not find mcfly database. Searched common locations but no history.db found."
        ))
    }

    /// Import from mcfly database directly
    async fn from_db(db_path: PathBuf) -> Result<Self> {
        let db_url = format!("sqlite://{}", db_path.to_string_lossy());
        let pool = SqlitePool::connect(&db_url).await.map_err(|e| {
            eyre!(
                "Failed to connect to mcfly database at {}: {}",
                db_path.display(),
                e
            )
        })?;

        Self::from_pool(pool).await
    }

    /// Import from mcfly database at specific path
    pub async fn from_file<P: AsRef<std::path::Path>>(db_path: P) -> Result<Self> {
        let path = db_path.as_ref().to_path_buf();
        if !path.exists() {
            return Err(eyre!("mcfly database not found at: {}", path.display()));
        }
        Self::from_db(path).await
    }

    async fn from_pool(pool: SqlitePool) -> Result<Self> {
        let commands: Vec<McflyCommand> = sqlx::query_as(
            "SELECT cmd, session_id, when_run, exit_code, dir FROM commands
             WHERE cmd IS NOT NULL AND cmd != ''
             ORDER BY when_run",
        )
        .fetch_all(&pool)
        .await
        .map_err(|e| eyre!("Failed to query mcfly commands: {}", e))?;

        let mut session_map = HashMap::new();
        let hostname = format!("{}:{}", get_hostname(), get_username());

        let entries = commands
            .into_iter()
            .map(|cmd| {
                let timestamp = OffsetDateTime::from_unix_timestamp(cmd.when_run)
                    .unwrap_or_else(|_| OffsetDateTime::now_utc());

                // Map session_id to UUID, creating new ones as needed
                let session = session_map
                    .entry(cmd.session_id.clone())
                    .or_insert_with(uuid_v7);

                History::import()
                    .timestamp(timestamp)
                    .command(cmd.cmd)
                    .cwd(cmd.dir)
                    .exit(cmd.exit_code)
                    .session(session.as_simple().to_string())
                    .hostname(hostname.clone())
                    .build()
                    .into()
            })
            .collect();

        Ok(Self { entries })
    }
}

#[async_trait]
impl Importer for Mcfly {
    const NAME: &'static str = "mcfly";

    async fn new() -> Result<Self> {
        let db_path = Self::histpath()?;
        Self::from_db(db_path).await
    }

    async fn entries(&mut self) -> Result<usize> {
        Ok(self.entries.len())
    }

    async fn load(self, h: &mut impl Loader) -> Result<()> {
        for entry in self.entries {
            h.push(entry).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::tests::TestLoader;
    use sqlx::SqlitePool;

    async fn setup_test_db() -> Result<SqlitePool> {
        let pool = SqlitePool::connect(":memory:").await?;

        // Create mcfly commands table with real schema including session_id
        sqlx::query(
            r#"
            CREATE TABLE commands (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                cmd TEXT NOT NULL,
                session_id TEXT NOT NULL,
                when_run INTEGER NOT NULL,
                exit_code INTEGER NOT NULL,
                dir TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await?;

        // Insert test data with timestamps, exit codes, and session IDs
        sqlx::query("INSERT INTO commands (cmd, session_id, when_run, exit_code, dir) VALUES (?, ?, ?, ?, ?)")
            .bind("ls -la")
            .bind("session1")
            .bind(1672574400) // 2023-01-01 12:00:00 UTC
            .bind(0)
            .bind("/home/user")
            .execute(&pool)
            .await?;

        sqlx::query("INSERT INTO commands (cmd, session_id, when_run, exit_code, dir) VALUES (?, ?, ?, ?, ?)")
            .bind("cd /tmp")
            .bind("session1")
            .bind(1672574410) // 10 seconds later
            .bind(0)
            .bind("/home/user")
            .execute(&pool)
            .await?;

        sqlx::query("INSERT INTO commands (cmd, session_id, when_run, exit_code, dir) VALUES (?, ?, ?, ?, ?)")
            .bind("false") // command that fails
            .bind("session2")
            .bind(1672574420) // 20 seconds later
            .bind(1)
            .bind("/tmp")
            .execute(&pool)
            .await?;

        Ok(pool)
    }

    #[tokio::test]
    async fn test_mcfly_db_import() -> Result<()> {
        let pool = setup_test_db().await?;
        let mcfly = Mcfly::from_pool(pool).await?;
        let mut loader = TestLoader::default();

        mcfly.load(&mut loader).await?;

        // Should import all commands from commands table
        assert_eq!(loader.buf.len(), 3);

        // Check first command
        assert_eq!(loader.buf[0].command, "ls -la");
        assert_eq!(loader.buf[0].cwd.as_str(), "/home/user");
        assert_eq!(loader.buf[0].exit, 0);

        // Check last command (failed command)
        assert_eq!(loader.buf[2].command, "false");
        assert_eq!(loader.buf[2].cwd.as_str(), "/tmp");
        assert_eq!(loader.buf[2].exit, 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_mcfly_db_with_missing_fields() -> Result<()> {
        let pool = SqlitePool::connect(":memory:").await?;

        // Create commands table with session_id field
        sqlx::query(
            r#"
            CREATE TABLE commands (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                cmd TEXT NOT NULL,
                session_id TEXT NOT NULL,
                when_run INTEGER NOT NULL,
                exit_code INTEGER NOT NULL,
                dir TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await?;

        sqlx::query("INSERT INTO commands (cmd, session_id, when_run, exit_code, dir) VALUES (?, ?, ?, ?, ?)")
            .bind("pwd")
            .bind("session3")
            .bind(1672574400)
            .bind(0)
            .bind("/home/test")
            .execute(&pool)
            .await?;

        let mcfly = Mcfly::from_pool(pool).await?;
        let mut loader = TestLoader::default();

        mcfly.load(&mut loader).await?;

        assert_eq!(loader.buf.len(), 1);
        assert_eq!(loader.buf[0].command, "pwd");
        assert_eq!(loader.buf[0].cwd.as_str(), "/home/test");

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_mcfly_db() -> Result<()> {
        let pool = SqlitePool::connect(":memory:").await?;

        // Create empty commands table with session_id field
        sqlx::query(
            r#"
            CREATE TABLE commands (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                cmd TEXT NOT NULL,
                session_id TEXT NOT NULL,
                when_run INTEGER NOT NULL,
                exit_code INTEGER NOT NULL,
                dir TEXT NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await?;

        let mcfly = Mcfly::from_pool(pool).await?;
        let mut loader = TestLoader::default();

        mcfly.load(&mut loader).await?;

        assert_eq!(loader.buf.len(), 0);

        Ok(())
    }
}
