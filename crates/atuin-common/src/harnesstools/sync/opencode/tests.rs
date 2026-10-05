use pretty_assertions::assert_eq;
use rstest::rstest;
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::opencode::rehydrate::tests::{captured, load_projection, opencode_import};
use crate::harnesstools::opencode::session::projection::ProjectedMessage;
use crate::harnesstools::rehydrate::testing;
use crate::harnesstools::session::{Content, Role};
use crate::harnesstools::sync::TableEntry;
use crate::harnesstools::sync::liveness::tests::{self as live, Fake, dir, procs};

const USER3: &str = "msg_0d154c3ed001zrhJSNW6BsHlTX";
const ASSISTANT3: &str = "msg_0d154c6b200176gyQ9ZgLlw1Sj";
const LAST: &str = "prt_0d154c9a5001gvwUYJAY42itXy";

async fn connect(path: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path)).await.unwrap()
}

async fn execute(path: &Path, sql: &str) {
    let mut conn = connect(path).await;
    // Not through `crate::db::query`, which takes only static SQL: these stand for opencode's
    // own writes.
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

/// Append through `opencode import` as opencode 1.18.32 does it.
async fn append(
    id: &str,
    base: &LocalTip,
    lines: &[RehydrateMessage],
) -> Result<AppendOutcome, SyncError> {
    let db = base.native_path.clone();
    append_in(id, base, lines, &AppendOptions::default(), move |cwd, export| async move {
        let mut conn = connect(&db).await;
        opencode_import(&mut conn, &export, &cwd.display().to_string()).await;
        conn.close().await.unwrap();
        Ok(())
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
    assert_eq!(tip.tip_source_id.as_deref(), Some(LAST));
    assert_eq!(tip.cwd.as_deref(), Some(dir.path()));
    assert!(local_tip_in(&db, "ses_none").await.unwrap().is_none());
}

/// This copy stopped before the last turn: importing the tail puts each part in the message it
/// was part of, and capture finds nothing new. One that stopped in the middle of it is forked
/// instead: no row names the last reply, so whether the rest is part of the one held can't be
/// told.
#[rstest]
#[case::before_the_last_turn(&[USER3, ASSISTANT3], &[], true)]
#[case::mid_turn(&[], &["prt_0d154c99a001dtKmLh8SYe2pad", LAST], false)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fast_forward_imports_the_tail(
    dir: TempDir,
    #[case] messages: &[&str],
    #[case] parts: &[&str],
    #[case] imported: bool,
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
    let all = synced(&full, &id).await;
    let at = all.iter().position(|r| Some(&r.source_id) == base.tip_source_id.as_ref()).unwrap();
    let tail = all[at + 1..].to_vec();

    if !imported {
        let err = append(&id, &base, &tail).await.unwrap_err();
        assert!(matches!(err, SyncError::Unsupported(_)), "{err:?}");
        return;
    }
    let outcome = append(&id, &base, &tail).await.unwrap();

    let after = local_tip_in(&db, &id).await.unwrap().unwrap();
    assert_eq!(after.tip_source_id.as_deref(), Some(LAST));
    assert_eq!(outcome.tip_source_id, after.tip_source_id);
    let again = captured(&db).await;
    testing::assert_nothing_new(
        &all,
        again.iter().map(|r| r.source_id.as_str()).filter(|s| !s.starts_with(&format!("{id}:"))),
    );
    // The last turn's parts are in one message, answering its prompt.
    let mut conn = connect(&db).await;
    let homes: Vec<(String,)> = crate::db::query_as::<sqlx::Sqlite, (String,)>(
        "SELECT DISTINCT message_id FROM part WHERE id IN (?1, ?2)",
    )
    .bind("prt_0d154c99a001dtKmLh8SYe2pad")
    .bind(LAST)
    .fetch_all(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();
    assert_eq!(homes.len(), 1, "{homes:?}");
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

const NEW: &str = "prt_0e000000000000000000000001";

const EDITED_TEXT: &str = "UPDATE part SET data = json_set(data, '$.text', 'edited') WHERE id = \
                           'prt_0d154c99a001dtKmLh8SYe2pad'";
const EDITED_MESSAGE: &str = "UPDATE message SET data = json_set(data, '$.agent', 'plan') WHERE \
                              id = 'msg_0d154c6b200176gyQ9ZgLlw1Sj'";

/// `before` runs before the local tip is read, `after` between reading it and appending.
#[rstest]
#[case::a_new_prompt_after_the_last("", "", NEW, 1_790_300_000, "Ok")]
#[case::a_prompt_before_the_last("", "", NEW, 1_790_217_000, "Unsupported")]
#[case::a_row_already_here("", "", LAST, 1_790_300_000, "IdTaken")]
#[case::a_revert_pending("UPDATE session SET revert = '{}'", "", NEW, 1_790_300_000, "Unsupported")]
#[case::a_part_removed_since(
    "",
    "DELETE FROM part WHERE id = 'prt_0d154c9a5001gvwUYJAY42itXy'",
    NEW,
    1_790_300_000,
    "Changed"
)]
#[case::a_part_edited_since("", EDITED_TEXT, NEW, 1_790_300_000, "Changed")]
#[case::a_message_edited_since("", EDITED_MESSAGE, NEW, 1_790_300_000, "Changed")]
#[case::edited_before_the_tip_was_read(EDITED_TEXT, "", NEW, 1_790_300_000, "Ok")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_a_clean_fast_forward_is_imported(
    dir: TempDir,
    #[case] before: &str,
    #[case] after: &str,
    #[case] new: &str,
    #[case] at: i64,
    #[case] expected: &str,
) {
    let (db, id) = session_in(dir.path(), "opencode.db").await;
    if !before.is_empty() {
        execute(&db, before).await;
    }
    let base = local_tip_in(&db, &id).await.unwrap().unwrap();
    if !after.is_empty() {
        execute(&db, after).await;
    }
    match append(&id, &base, &[prompt(new, at)]).await {
        Ok(outcome) => {
            assert_eq!(expected, "Ok");
            assert_eq!(outcome.appended, [new]);
            assert_eq!(outcome.tip_source_id.as_deref(), Some(new));
        }
        Err(err) => assert!(format!("{err:?}").starts_with(expected), "{err:?}"),
    }
}

/// A session whose last message is `msg_b`, created at 1000.
fn ending_in_msg_b() -> Projected {
    let message = |id: &str, created| ProjectedMessage {
        id: id.to_owned(),
        created,
        assistant: false,
        parent: None,
        completed: true,
    };
    Projected {
        directory: PathBuf::from("/nowhere"),
        created: 0,
        reverting: false,
        rows: Vec::new(),
        messages: vec![message("msg_a", 1000), message("msg_b", 1000)],
        content: 0,
        updated: None,
    }
}

/// opencode reads messages by creation time, then id: a new message created in the same
/// millisecond as the last one but with an id sorting before it would be read before it.
#[rstest]
#[case::a_later_millisecond(1001, "msg_0", true)]
#[case::the_same_millisecond_and_a_later_id(1000, "msg_c", true)]
#[case::the_same_millisecond_and_an_earlier_id(1000, "msg_ab", false)]
#[case::an_earlier_millisecond(999, "msg_z", false)]
fn a_new_message_must_sort_after_the_last(
    #[case] created: i64,
    #[case] id: &str,
    #[case] continues: bool,
) {
    let mut export = serde_json::json!({"messages": [{
        "info": {"id": id, "role": "user", "time": {"created": created}},
        "parts": [],
    }]});
    let result = continue_messages(&mut export, &ending_in_msg_b(), &HashSet::new());
    assert_eq!(result.is_ok(), continues, "{result:?}");
    if !continues {
        assert!(matches!(result, Err(SyncError::Unsupported(_))), "{result:?}");
    }
}

/// A session of a prompt, `msg_u`, and the replies to it opencode holds: each its id, all
/// created at 2000.
fn replies_to_msg_u(replies: &[&str]) -> Projected {
    let message = |id: &str, created, assistant| ProjectedMessage {
        id: id.to_owned(),
        created,
        assistant,
        parent: assistant.then(|| "msg_u".to_owned()),
        completed: true,
    };
    Projected {
        directory: PathBuf::from("/nowhere"),
        created: 0,
        reverting: false,
        rows: Vec::new(),
        messages: std::iter::once(message("msg_u", 1000, false))
            .chain(replies.iter().map(|id| message(id, 2000, true)))
            .collect(),
        content: 0,
        updated: None,
    }
}

/// An exported reply to `msg_u`, created at 2000, of one part.
fn reply(id: &str) -> serde_json::Value {
    serde_json::json!({
        "info": {"id": id, "role": "assistant", "parentID": "msg_u", "time": {"created": 2000}},
        "parts": [{"id": format!("prt_{id}"), "messageID": id}],
    })
}

/// Which message each exported reply's parts go into: its own id. One whose id was minted is a new
/// message, unless opencode holds a reply it may be, when the catch-up is refused.
#[rstest]
#[case::a_distinct_reply_of_the_same_millisecond(&["msg_a1"], &["msg_a2"], &[], Ok(vec!["msg_a2"]))]
#[case::a_reply_held_by_its_id(&["msg_a1"], &["msg_a1"], &[], Ok(vec!["msg_a1"]))]
#[case::a_minted_reply_that_may_be_the_held_one(&["msg_a1"], &["msg_m"], &["msg_m"], Err(()))]
#[case::a_minted_reply_none_may_be(&[], &["msg_m"], &["msg_m"], Ok(vec!["msg_m"]))]
#[case::a_minted_reply_beside_the_held_one(
    &["msg_a1"],
    &["msg_a1", "msg_m"],
    &["msg_m"],
    Ok(vec!["msg_a1", "msg_m"])
)]
#[case::a_minted_reply_that_may_be_either(&["msg_a1", "msg_a2"], &["msg_m"], &["msg_m"], Err(()))]
fn a_message_is_known_by_its_id(
    #[case] held: &[&str],
    #[case] exported: &[&str],
    #[case] minted: &[&str],
    #[case] expected: Result<Vec<&str>, ()>,
) {
    let mut export =
        serde_json::json!({"messages": exported.iter().map(|id| reply(id)).collect::<Vec<_>>()});
    let minted = minted.iter().map(|id| (*id).to_owned()).collect();
    let result = continue_messages(&mut export, &replies_to_msg_u(held), &minted);
    match expected {
        Ok(into) => {
            result.unwrap();
            let homes: Vec<(&str, &str)> = export["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| {
                    (
                        m["info"]["id"].as_str().unwrap(),
                        m["parts"][0]["messageID"].as_str().unwrap(),
                    )
                })
                .collect();
            let into: Vec<(&str, &str)> = into.iter().map(|id| (*id, *id)).collect();
            assert_eq!(homes, into);
        }
        Err(()) => assert!(matches!(result, Err(SyncError::Unsupported(_))), "{result:?}"),
    }
}

#[rstest]
#[case::an_opencode_running(b"/usr/bin/opencode\0serve\0".as_slice(), Liveness::Unknown)]
#[case::none(b"bash\0".as_slice(), Liveness::NotLive)]
fn any_opencode_running_may_be_writing(
    dir: TempDir,
    #[case] cmdline: &[u8],
    #[case] expected: Liveness,
) {
    let procs = procs(dir.path(), &[Fake::running(9, cmdline, "1")]);
    assert_eq!(liveness(&procs, &Seen::default()), expected);
}

const UNFINISHED: &str = "UPDATE message SET data = json_remove(data, '$.time.completed') WHERE \
                          id = 'msg_0d154c6b200176gyQ9ZgLlw1Sj'";

/// While an opencode that may be writing the session runs, a last reply opencode has not stamped
/// completed is being written; with none running, it is a copy written from a reply synced
/// half-way.
#[rstest]
#[case::running_its_last_reply_completed(true, "", Liveness::Unknown)]
#[case::running_its_last_reply_unfinished(true, UNFINISHED, Liveness::Live { pid: None })]
#[case::none_running_its_last_reply_unfinished(false, UNFINISHED, Liveness::NotLive)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reply_still_being_written_is_live(
    dir: TempDir,
    #[case] running: bool,
    #[case] change: &str,
    #[case] expected: Liveness,
) {
    let (db, id) = session_in(dir.path(), "opencode.db").await;
    if !change.is_empty() {
        execute(&db, change).await;
    }
    let opencode = [Fake::running(9, b"opencode\0", "1")];
    let procs = procs(
        dir.path(),
        if running {
            &opencode
        } else {
            &[]
        },
    );
    assert_eq!(liveness_in(Some(&db), &id, procs, None).await, expected);
}

/// A copy that stopped in the middle of a reply it holds unfinished, with no opencode running:
/// not live, and the rest of the reply is no fast-forward.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_left_mid_reply_is_not_live_but_forked(dir: TempDir) {
    let (full, id) = session_in(dir.path(), "full.db").await;
    let (db, _) = session_in(dir.path(), "local.db").await;
    execute(&db, &format!("DELETE FROM part WHERE id = '{LAST}'")).await;
    execute(&db, UNFINISHED).await;
    let procs = procs(dir.path(), &[]);
    assert_eq!(liveness_in(Some(&db), &id, procs, None).await, Liveness::NotLive);
    let base = local_tip_in(&db, &id).await.unwrap().unwrap();
    let all = synced(&full, &id).await;
    let at = all.iter().position(|r| Some(&r.source_id) == base.tip_source_id.as_ref()).unwrap();
    let err = append(&id, &base, &all[at + 1..]).await.unwrap_err();
    assert!(matches!(err, SyncError::Unsupported(_)), "{err:?}");
}

/// An opencode working elsewhere may have the session open from there (`opencode -s <id>`): it
/// counts when its command line names the session, or while opencode updated the session lately,
/// both as [`SessionSync::is_live`] and as [`SessionSync::append`] tell it.
#[rstest]
#[case::changed_lately_named_nowhere(10, false, Liveness::Unknown)]
#[case::changed_long_ago_named_nowhere(3600, false, Liveness::NotLive)]
#[case::changed_long_ago_named(3600, true, Liveness::Unknown)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_opencode_elsewhere_counts_when_named_or_writing(
    dir: TempDir,
    #[case] ago: u64,
    #[case] named: bool,
    #[case] expected: Liveness,
) {
    let (db, id) = session_in(dir.path(), "opencode.db").await;
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    execute(&db, &format!("UPDATE session SET directory = '{}'", proj.display())).await;
    let at = std::time::SystemTime::now() - std::time::Duration::from_secs(ago);
    let ms = at.duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_millis();
    for table in ["session", "message", "part"] {
        execute(&db, &format!("UPDATE {table} SET time_updated = {ms}")).await;
    }
    let base = local_tip_in(&db, &id).await.unwrap().unwrap();
    let ms = std::time::Duration::from_millis(ms.try_into().unwrap());
    assert_eq!(base.modified, Some(std::time::SystemTime::UNIX_EPOCH + ms));
    let cmd: &[&str] = if named {
        &["opencode", "-s", &id]
    } else {
        &["opencode"]
    };
    let procs = live::table(&[TableEntry {
        cwd: Some(dir.path().join("other")),
        ..live::entry(9, "opencode", cmd, 1)
    }]);
    assert_eq!(liveness(&procs, &appending(&base, &id, Some(&proj))), expected);
    assert_eq!(liveness_in(Some(&db), &id, procs, Some(&proj)).await, expected);
}
