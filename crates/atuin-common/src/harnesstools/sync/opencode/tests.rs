use pretty_assertions::assert_eq;
use rstest::{fixture, rstest};
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::opencode::rehydrate::tests::{captured, load_projection, opencode_import};
use crate::harnesstools::rehydrate::testing;
use crate::harnesstools::session::{Content, Role};

const USER3: &str = "msg_0d154c3ed001zrhJSNW6BsHlTX";
const ASSISTANT3: &str = "msg_0d154c6b200176gyQ9ZgLlw1Sj";

#[fixture]
fn dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

async fn connect(path: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path)).await.unwrap()
}

/// Change opencode's database as opencode (or a newer schema of it) would.
async fn execute(path: &Path, sql: &str) {
    let mut conn = connect(path).await;
    // Not through `crate::db::query`, which refuses schema changes: this one stands for a newer
    // opencode's schema.
    #[allow(clippy::disallowed_methods)]
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned())).execute(&mut conn).await.unwrap();
    conn.close().await.unwrap();
}

/// The session opencode 1.18.32 wrote, in a database of its own, working in `dir`.
async fn session_in(dir: &Path, name: &str) -> (PathBuf, String) {
    let db = dir.join(name);
    let id = load_projection(&db).await;
    execute(&db, &format!("UPDATE session SET directory = '{}'", dir.display())).await;
    (db, id)
}

/// The rows capture synced of the session in `db`, bar the session's own.
async fn synced(db: &Path, id: &str) -> Vec<RehydrateMessage> {
    let rows = captured(db).await;
    testing::synced(
        rows.into_iter().filter(|r| !r.source_id.starts_with(&format!("{id}:"))).collect(),
    )
}

/// `opencode import` as opencode 1.18.32 does it, into `db`.
async fn import_into(db: &Path, cwd: &Path, export: &Value) -> Result<(), SyncError> {
    let mut conn = connect(db).await;
    opencode_import(&mut conn, export, &cwd.display().to_string()).await;
    conn.close().await.unwrap();
    Ok(())
}

async fn append(
    db: &Path,
    id: &str,
    base: &LocalTip,
    lines: &[RehydrateMessage],
) -> Result<AppendOutcome, SyncError> {
    let into = db.to_path_buf();
    append_in(db, id, base, lines, &AppendOptions::default(), move |cwd, export| async move {
        import_into(&into, &cwd, &export).await
    })
    .await
}

#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tip_is_the_last_row_opencode_reads(dir: TempDir) {
    let (db, id) = session_in(dir.path(), "opencode.db").await;
    let tip = local_tip_in(&db, &id).await.unwrap().unwrap();
    let rows = synced(&db, &id).await;
    assert_eq!(
        tip.known_source_ids,
        rows.iter().map(|r| r.source_id.clone()).collect::<HashSet<_>>()
    );
    assert_eq!(tip.tip_source_id.as_deref(), Some("prt_0d154c9a5001gvwUYJAY42itXy"));
    assert!(local_tip_in(&db, "ses_none").await.unwrap().is_none());
}

/// A reverted message is gone from the projection (the event log still holds it): it is no
/// row of the copy here.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_a_revert_removed_is_not_held(dir: TempDir) {
    let (db, id) = session_in(dir.path(), "opencode.db").await;
    execute(&db, &format!("DELETE FROM part WHERE message_id = '{ASSISTANT3}'")).await;
    execute(&db, &format!("DELETE FROM message WHERE id = '{ASSISTANT3}'")).await;
    let tip = local_tip_in(&db, &id).await.unwrap().unwrap();
    assert_eq!(tip.tip_source_id.as_deref(), Some("prt_0d154c3f0001VPSmfCQt4r6LWN"));
}

/// This copy stopped before the last turn, or in the middle of it: importing only the rows it
/// lacks puts each part in the message it was part of, and capture finds nothing new.
#[rstest]
#[case::before_the_last_turn(&[USER3, ASSISTANT3], &[])]
#[case::mid_turn(&[], &["prt_0d154c99a001dtKmLh8SYe2pad", "prt_0d154c9a5001gvwUYJAY42itXy"])]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catching_up_imports_only_what_is_missing(
    dir: TempDir,
    #[case] messages: &[&str],
    #[case] parts: &[&str],
) {
    let (full, id) = session_in(dir.path(), "full.db").await;
    let (db, _) = session_in(dir.path(), "local.db").await;
    for message in messages {
        execute(&db, &format!("DELETE FROM part WHERE message_id = '{message}'")).await;
        execute(&db, &format!("DELETE FROM message WHERE id = '{message}'")).await;
    }
    for part in parts {
        execute(&db, &format!("DELETE FROM part WHERE id = '{part}'")).await;
    }
    let base = local_tip_in(&db, &id).await.unwrap().unwrap();
    let synced = synced(&full, &id).await;
    let missing: Vec<RehydrateMessage> =
        synced.iter().filter(|r| !base.known_source_ids.contains(&r.source_id)).cloned().collect();
    assert!(!missing.is_empty());

    let outcome = append(&db, &id, &base, &missing).await.unwrap();

    let again = captured(&db).await;
    testing::assert_nothing_new(
        &synced,
        again.iter().map(|r| r.source_id.as_str()).filter(|s| !s.starts_with(&format!("{id}:"))),
    );
    let after = local_tip_in(&db, &id).await.unwrap().unwrap();
    assert_eq!(after.tip_source_id.as_deref(), Some("prt_0d154c9a5001gvwUYJAY42itXy"));
    assert_eq!(outcome.tip_source_id, after.tip_source_id);
    // Every row the export can carry is here now (capture syncs a `step-start` part as an empty
    // row, which no part is made of).
    let carried: Vec<&str> = missing
        .iter()
        .filter(|r| !r.content.is_empty() || r.usage.is_some())
        .map(|r| r.source_id.as_str())
        .collect();
    assert!(carried.iter().all(|id| after.known_source_ids.contains(*id)), "{carried:?}");
    assert_eq!(outcome.appended, carried);
    // The last turn's parts are in one message answering its own prompt: the one this copy
    // holds when it holds it (a new one's id is minted when no row names it).
    let mut conn = connect(&db).await;
    let homes: Vec<(String, String)> = crate::db::query_as::<sqlx::Sqlite, (String, String)>(
        "SELECT part.message_id, message.data FROM part JOIN message ON message.id = \
         part.message_id WHERE part.id IN ('prt_0d154c99a001dtKmLh8SYe2pad', \
         'prt_0d154c9a5001gvwUYJAY42itXy')",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    assert_eq!(homes.len(), 2);
    assert_eq!(homes[0].0, homes[1].0);
    let info: Value = serde_json::from_str(&homes[0].1).unwrap();
    assert_eq!(info["parentID"], USER3);
    if messages.is_empty() {
        assert_eq!(homes[0].0, ASSISTANT3);
    }
    let held: Vec<(String,)> = crate::db::query_as::<sqlx::Sqlite, (String,)>(
        "SELECT id FROM message ORDER BY time_created, id",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    assert_eq!(held.len(), 6, "{held:?}");
    conn.close().await.unwrap();
}

fn prompt(id: &str, at: i64) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: None,
        timestamp: OffsetDateTime::from_unix_timestamp(at).unwrap(),
        role: Role::User,
        content: vec![Content::Text("from the other machine".to_owned())],
        model: None,
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
    }
}

#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_prompt_after_the_last_is_appended(dir: TempDir) {
    let (db, id) = session_in(dir.path(), "opencode.db").await;
    let base = local_tip_in(&db, &id).await.unwrap().unwrap();
    let outcome =
        append(&db, &id, &base, &[prompt("prt_0e000000000000000000000001", 1_790_300_000)])
            .await
            .unwrap();
    assert_eq!(outcome.appended, vec!["prt_0e000000000000000000000001"]);
    assert_eq!(outcome.tip_source_id.as_deref(), Some("prt_0e000000000000000000000001"));
}

#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_branch_off_an_earlier_message_is_refused(dir: TempDir) {
    let (db, id) = session_in(dir.path(), "opencode.db").await;
    let base = local_tip_in(&db, &id).await.unwrap().unwrap();
    let err = append(&db, &id, &base, &[prompt("prt_0e000000000000000000000001", 1_790_217_000)])
        .await
        .unwrap_err();
    assert!(matches!(err, SyncError::Unsupported(_)), "{err:?}");
}

#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pending_revert_is_not_caught_up_under(dir: TempDir) {
    let (db, id) = session_in(dir.path(), "opencode.db").await;
    execute(&db, "ALTER TABLE session ADD COLUMN revert TEXT").await;
    execute(&db, &format!("UPDATE session SET revert = '{{\"messageID\":\"{USER3}\"}}'")).await;
    let base = local_tip_in(&db, &id).await.unwrap().unwrap();
    let err = append(&db, &id, &base, &[prompt("prt_0e000000000000000000000001", 1_790_300_000)])
        .await
        .unwrap_err();
    assert!(matches!(err, SyncError::Unsupported(_)), "{err:?}");
}

#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_changed_since_it_was_read_is_left_alone(dir: TempDir) {
    let (db, id) = session_in(dir.path(), "opencode.db").await;
    let base = local_tip_in(&db, &id).await.unwrap().unwrap();
    execute(&db, "DELETE FROM part WHERE id = 'prt_0d154c9a5001gvwUYJAY42itXy'").await;
    let err = append(&db, &id, &base, &[prompt("prt_0e000000000000000000000001", 1_790_300_000)])
        .await
        .unwrap_err();
    assert!(matches!(err, SyncError::Changed), "{err:?}");
}

#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_already_here_is_refused(dir: TempDir) {
    let (db, id) = session_in(dir.path(), "opencode.db").await;
    let base = local_tip_in(&db, &id).await.unwrap().unwrap();
    let err = append(&db, &id, &base, &[prompt("prt_0d154c9a5001gvwUYJAY42itXy", 1_790_300_000)])
        .await
        .unwrap_err();
    assert!(matches!(err, SyncError::IdTaken(_)), "{err:?}");
}

#[rstest]
#[case::the_tip(Some("prt_0d154c9a5001gvwUYJAY42itXy"), true)]
#[case::nothing_named(None, true)]
#[case::an_earlier_row(Some("prt_0d148d4bf001JaxA9TwaMgU5vE"), false)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_last_row_can_be_the_tip(
    dir: TempDir,
    #[case] head: Option<&str>,
    #[case] ok: bool,
) {
    let (db, id) = session_in(dir.path(), "opencode.db").await;
    let base = local_tip_in(&db, &id).await.unwrap().unwrap();
    let options = AppendOptions {
        make_tip: true,
        head,
        ..AppendOptions::default()
    };
    let found = append_in(&db, &id, &base, &[], &options, |_, _| async {
        unreachable!("nothing is imported")
    })
    .await;
    assert_eq!(found.is_ok(), ok, "{found:?}");
}
