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
use crate::path::AbsPathBuf;

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
    newest(home, "state_")
}

/// The highest-numbered `<prefix><n>.sqlite` under Codex home `home`.
fn newest(home: &Path, prefix: &str) -> Option<PathBuf> {
    std::fs::read_dir(home)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let version: u32 =
                name.to_str()?.strip_prefix(prefix)?.strip_suffix(".sqlite")?.parse().ok()?;
            Some((version, entry.path()))
        })
        .max()
        .map(|(_, path)| path)
}

async fn connect(db: &Path) -> Result<SqliteConnection, sqlx::Error> {
    let opts = SqliteConnectOptions::new()
        .filename(db)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    SqliteConnection::connect_with(&opts).await
}

async fn has_table(conn: &mut SqliteConnection, table: &str) -> Result<bool, sqlx::Error> {
    let found = query_scalar::<Sqlite, i64>(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
    )
    .bind(table)
    .fetch_one(conn)
    .await?;
    Ok(found > 0)
}

/// Forget what Codex projected of thread `thread`'s history from its rollout
/// (`thread_history_<n>.sqlite`: the thread's turns and items, which it shows the thread from,
/// and the byte offset into the rollout it goes on projecting from), which Codex projects again
/// from the whole rollout when it next loads the thread. A rollout written anew (switched to
/// another branch, or restored) no longer holds the lines it was projected from: Codex would go
/// on showing those, reading on from an offset into lines that are gone.
pub async fn forget_history(home: &Path, thread: &str) -> Result<(), StateDbError> {
    let Some(db) = newest(home, "thread_history_") else {
        return Ok(());
    };
    let mut conn = connect(&db).await?;
    if !has_table(&mut conn, "thread_history_projection_state").await? {
        return Ok(());
    }
    let mut tx = conn.begin().await?;
    for delete in [
        "DELETE FROM thread_history_projection_state WHERE thread_id = ?1",
        "DELETE FROM thread_items WHERE thread_id = ?1",
        "DELETE FROM thread_turns WHERE thread_id = ?1",
    ] {
        query::<Sqlite>(delete).bind(thread).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    conn.close().await?;
    Ok(())
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
    let mut conn = connect(&db).await?;
    if !has_table(&mut conn, "threads").await? {
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
        || matches!((AbsPathBuf::canonicalize(a), AbsPathBuf::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
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

    /// Codex's history of threads as it projects it (codex 0.160.1's `thread_history_1.sqlite`,
    /// its columns cut down), holding some of each of `threads`.
    pub async fn history(home: &Path, threads: &[&str]) -> PathBuf {
        let db = home.join("thread_history_1.sqlite");
        let opts = SqliteConnectOptions::new().filename(&db).create_if_missing(true);
        let mut conn = SqliteConnection::connect_with(&opts).await.unwrap();
        for sql in [
            "CREATE TABLE thread_turns (thread_id TEXT NOT NULL, turn_id TEXT NOT NULL, \
             rollout_ordinal INTEGER NOT NULL, PRIMARY KEY (thread_id, turn_id))",
            "CREATE TABLE thread_items (thread_id TEXT NOT NULL, turn_id TEXT NOT NULL, item_id \
             TEXT NOT NULL, rollout_ordinal INTEGER NOT NULL, PRIMARY KEY (thread_id, turn_id, \
             item_id))",
            "CREATE TABLE thread_history_projection_state (thread_id TEXT PRIMARY KEY, \
             next_rollout_byte_offset INTEGER NOT NULL, next_rollout_ordinal INTEGER NOT NULL)",
        ] {
            query::<Sqlite>(sql).execute(&mut conn).await.unwrap();
        }
        for thread in threads {
            for sql in [
                "INSERT INTO thread_turns VALUES (?1, 't1', 1)",
                "INSERT INTO thread_items VALUES (?1, 't1', 'i1', 2)",
                "INSERT INTO thread_history_projection_state VALUES (?1, 83674, 36)",
            ] {
                query::<Sqlite>(sql).bind(*thread).execute(&mut conn).await.unwrap();
            }
        }
        db
    }

    /// How many rows of its history Codex holds of `thread`.
    pub async fn projected(db: &Path, thread: &str) -> i64 {
        let mut conn = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(db))
            .await
            .unwrap();
        query_scalar::<Sqlite, i64>(
            "SELECT (SELECT count(*) FROM thread_turns WHERE thread_id = ?1) + (SELECT count(*) \
             FROM thread_items WHERE thread_id = ?1) + (SELECT count(*) FROM \
             thread_history_projection_state WHERE thread_id = ?1)",
        )
        .bind(thread)
        .fetch_one(&mut conn)
        .await
        .unwrap()
    }

    /// The thread's history goes, to be projected again from its rollout; another thread's
    /// stays. With no history at all there is nothing to forget.
    #[rstest]
    #[tokio::test]
    async fn a_threads_history_is_forgotten() {
        let home = tempfile::tempdir().unwrap();
        forget_history(home.path(), THREAD).await.unwrap();
        let other = "01a0ea6e-0000-7000-8000-000000000000";
        let db = history(home.path(), &[THREAD, other]).await;
        forget_history(home.path(), THREAD).await.unwrap();
        assert_eq!(projected(&db, THREAD).await, 0);
        assert_eq!(projected(&db, other).await, 3);
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
