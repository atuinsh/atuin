//! Codex's thread index: `$CODEX_HOME/state_<n>.sqlite`, table `threads` (codex-rs `state`).
//!
//! It caches what Codex knows of each thread, rebuilt from the rollouts when it is missing
//! (codex-rs `metadata.rs` `backfill_sessions`, which upserts each rollout in name order, so a
//! thread's newest segment is what it keeps). One column of it is authoritative once a row
//! exists: `rollout_path`, the file Codex resumes the thread from. A paginated thread's newest
//! segment is only ever switched to through it (`thread/revert`: `replace_rollout_path_if_current`),
//! and while the row names a file that is gone, Codex finds no rollout of the thread at all
//! (codex-rs `thread_rollout_resolver.rs`). Without a row, Codex looks the thread up by file
//! name (the newest segment) and writes the row itself.
//!
//! So a rollout written here is pointed to where a row names another file: a new segment of a
//! thread in place of the one it continues, or a rehydrated rollout in place of one deleted.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{Connection, Sqlite, SqliteConnection};

use crate::db::{query, query_scalar};

/// What [`point_at`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pointed {
    /// No index (Codex never ran here), or no row for the thread: Codex finds the rollout by name.
    Unindexed,
    /// The row already named the rollout.
    Already,
    /// The row now names the rollout.
    Repointed,
}

#[derive(Debug, thiserror::Error)]
pub enum StateDbError {
    /// The row names another rollout of the thread than the one expected, which still exists.
    #[error("Codex's thread index names another rollout of the thread: {}", .0.display())]
    Elsewhere(PathBuf),
    #[error("Codex's thread index could not be updated: {0}")]
    Db(#[from] sqlx::Error),
}

/// The current thread index under Codex home `home`: the highest-numbered `state_<n>.sqlite`.
pub fn index_path(home: &Path) -> Option<PathBuf> {
    std::fs::read_dir(home)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let version: u32 =
                name.to_str()?.strip_prefix("state_")?.strip_suffix(".sqlite")?.parse().ok()?;
            Some((version, entry.path()))
        })
        .max()
        .map(|(_, path)| path)
}

/// Make the index row of thread `thread` (if there is one) name `rollout`, where it names
/// `replacing` (the rollout `rollout` continues) or a file that is gone. Refuses
/// ([`StateDbError::Elsewhere`]) where it names another rollout that exists: Codex has moved the
/// thread on since.
pub async fn point_at(
    home: &Path,
    thread: &str,
    replacing: Option<&Path>,
    rollout: &Path,
) -> Result<Pointed, StateDbError> {
    let Some(db) = index_path(home) else {
        return Ok(Pointed::Unindexed);
    };
    let opts = SqliteConnectOptions::new()
        .filename(&db)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let mut conn = SqliteConnection::connect_with(&opts).await?;
    let has_threads = query_scalar::<Sqlite, i64>(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'threads'",
    )
    .fetch_one(&mut conn)
    .await?;
    if has_threads == 0 {
        return Ok(Pointed::Unindexed);
    }
    let stored = query_scalar::<Sqlite, String>("SELECT rollout_path FROM threads WHERE id = ?1")
        .bind(thread)
        .fetch_optional(&mut conn)
        .await?;
    let Some(stored) = stored else {
        return Ok(Pointed::Unindexed);
    };
    let stored_path = Path::new(&stored);
    if same_file(stored_path, rollout) {
        return Ok(Pointed::Already);
    }
    let replaceable = !stored_path.exists() || replacing.is_some_and(|r| same_file(stored_path, r));
    if !replaceable {
        return Err(StateDbError::Elsewhere(stored_path.to_path_buf()));
    }
    let updated =
        query::<Sqlite>("UPDATE threads SET rollout_path = ?1 WHERE id = ?2 AND rollout_path = ?3")
            .bind(rollout.to_string_lossy().into_owned())
            .bind(thread)
            .bind(&stored)
            .execute(&mut conn)
            .await?;
    conn.close().await?;
    if updated.rows_affected() == 0 {
        return Err(StateDbError::Elsewhere(stored_path.to_path_buf()));
    }
    Ok(Pointed::Repointed)
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

#[cfg(test)]
pub mod tests {
    use rstest::rstest;

    use super::*;

    const THREAD: &str = "01a0ea6e-7bbf-7d30-abed-1bf13a4ab75b";

    /// An index as Codex keeps it, holding a row for [`THREAD`] naming `path`.
    pub async fn index(home: &Path, path: Option<&Path>) -> PathBuf {
        let db = home.join("state_5.sqlite");
        let opts = SqliteConnectOptions::new().filename(&db).create_if_missing(true);
        let mut conn = SqliteConnection::connect_with(&opts).await.unwrap();
        query::<Sqlite>(
            "CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT NOT NULL, history_mode \
             TEXT NOT NULL DEFAULT 'legacy')",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        if let Some(path) = path {
            query::<Sqlite>("INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)")
                .bind(THREAD)
                .bind(path.to_string_lossy().into_owned())
                .execute(&mut conn)
                .await
                .unwrap();
        }
        db
    }

    async fn stored(db: &Path) -> String {
        let opts = SqliteConnectOptions::new().filename(db);
        let mut conn = SqliteConnection::connect_with(&opts).await.unwrap();
        query_scalar::<Sqlite, String>("SELECT rollout_path FROM threads WHERE id = ?1")
            .bind(THREAD)
            .fetch_one(&mut conn)
            .await
            .unwrap()
    }

    #[rstest]
    #[tokio::test]
    async fn no_index_or_no_row_leaves_it_to_codex() {
        let home = tempfile::tempdir().unwrap();
        let new = home.path().join("new.jsonl");
        assert_eq!(point_at(home.path(), THREAD, None, &new).await.unwrap(), Pointed::Unindexed);
        index(home.path(), None).await;
        assert_eq!(point_at(home.path(), THREAD, None, &new).await.unwrap(), Pointed::Unindexed);
    }

    /// The row moves from the rollout a new one continues, or from one that is gone; never from
    /// another rollout that exists.
    #[rstest]
    #[case::from_what_it_continues(true, true, Ok(Pointed::Repointed))]
    #[case::from_a_deleted_one(false, false, Ok(Pointed::Repointed))]
    #[case::from_another_one(true, false, Err(()))]
    #[tokio::test]
    async fn the_row_is_pointed_at_the_new_rollout(
        #[case] old_exists: bool,
        #[case] replacing: bool,
        #[case] expected: Result<Pointed, ()>,
    ) {
        let home = tempfile::tempdir().unwrap();
        let old = home.path().join("old.jsonl");
        if old_exists {
            std::fs::write(&old, "{}\n").unwrap();
        }
        let new = home.path().join("new.jsonl");
        std::fs::write(&new, "{}\n").unwrap();
        let db = index(home.path(), Some(&old)).await;
        let replacing = replacing.then_some(old.as_path());
        let pointed = point_at(home.path(), THREAD, replacing, &new).await;
        assert_eq!(pointed.as_ref().ok(), expected.as_ref().ok());
        let now = stored(&db).await;
        match expected {
            Ok(_) => assert_eq!(now, new.to_string_lossy()),
            Err(()) => assert_eq!(now, old.to_string_lossy()),
        }
        assert_eq!(point_at(home.path(), THREAD, None, &new).await.ok(), match pointed {
            Ok(_) => Some(Pointed::Already),
            Err(_) => None,
        });
    }
}
