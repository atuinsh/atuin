//! Catching a Codex thread up, and branching it, over a real rollout.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};

use pretty_assertions::assert_eq;
use rstest::{fixture, rstest};
use serde_json::Value;
use tempfile::TempDir;

use super::*;
use crate::harnesstools::codex::rehydrate::tests::{
    assert_paginated, captured, fixture as codex_fixture, own_id, read, row, session,
};
use crate::harnesstools::codex::rehydrate::write as rehydrate;
use crate::harnesstools::codex::session::locate;
use crate::harnesstools::codex::state_db::tests::index;
use crate::harnesstools::rehydrate::testing::assert_nothing_new;
use crate::harnesstools::session::{Content, Role};

/// A real paginated rollout (Codex 0.156): three turns, two of them compacted.
const ORIGINAL: &str = "paginated-compacted.jsonl";
/// Where its second turn starts (its `task_started`).
const SECOND_TURN: u64 = 17;

/// The session the fixture holds, and its rows as capture synced them.
struct Synced {
    id: String,
    rows: Vec<RehydrateMessage>,
}

impl Synced {
    fn before(&self, seq: u64) -> Vec<RehydrateMessage> {
        self.rows.iter().filter(|r| r.seq.is_some_and(|s| s < seq)).cloned().collect()
    }

    fn from(&self, seq: u64) -> Vec<RehydrateMessage> {
        self.rows.iter().filter(|r| r.seq.is_some_and(|s| s >= seq)).cloned().collect()
    }
}

#[fixture]
async fn synced() -> Synced {
    let id = own_id(&codex_fixture(ORIGINAL));
    let rows = captured(&id, &read(&id, codex_fixture(ORIGINAL)).await);
    assert!(rows.iter().all(|r| r.seq.is_some()));
    Synced { id, rows }
}

#[fixture]
fn home() -> TempDir {
    tempfile::tempdir().unwrap()
}

fn sessions(home: &TempDir) -> PathBuf {
    home.path().join("sessions")
}

/// `rows` rehydrated under `home`, as this machine's copy of the session.
fn local_copy(home: &TempDir, id: &str, rows: Vec<RehydrateMessage>) -> PathBuf {
    rehydrate(&sessions(home), &session(id, rows)).unwrap()
}

/// The original rollout itself, as the machine that recorded it holds it.
fn original(home: &TempDir, id: &str) -> PathBuf {
    let dir = sessions(home).join("2026/09/24");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("rollout-2026-09-24T02-47-48-{id}.jsonl"));
    std::fs::copy(codex_fixture(ORIGINAL), &path).unwrap();
    path
}

/// What capture reads from every rollout of the thread, each read from its start.
async fn recapture(home: &TempDir, id: &str) -> Vec<RehydrateMessage> {
    let mut rows = Vec::new();
    for path in rollouts_of(&sessions(home), id) {
        rows.extend(captured(id, &read(id, path).await));
    }
    rows
}

fn tip(home: &TempDir, id: &str) -> LocalTip {
    local_tip_in(home.path(), id).unwrap().unwrap()
}

fn ids(rows: &[RehydrateMessage]) -> Vec<String> {
    rows.iter().map(|r| r.source_id.clone()).collect()
}

fn lines(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap();
    text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// Another host's turn, continuing the thread after its first turn: numbered as Codex numbers
/// it there, from the same numbers this host's second turn took.
fn other_turn(first: u64) -> Vec<RehydrateMessage> {
    let text = |t: &str| vec![Content::Text(t.to_owned())];
    // Its `task_started` took `first`, uncaptured.
    let mut prompt = row("msg_elsewhere_u", 1_790_000_000, Role::User, text("meanwhile"));
    prompt.seq = Some(first + 1);
    let mut reply = row("msg_elsewhere_a", 1_790_000_001, Role::Assistant, text("done there"));
    reply.seq = Some(first + 4);
    vec![prompt, reply]
}

#[rstest]
fn a_thread_not_here_has_no_tip(home: TempDir) {
    assert_eq!(local_tip_in(home.path(), THREAD).unwrap(), None);
}

/// The tip is the last line capture keeps a row of by its own id; every such line is known.
#[rstest]
#[tokio::test]
async fn the_tip_is_the_last_line_with_an_id(#[future] synced: Synced, home: TempDir) {
    let synced = synced.await;
    let path = original(&home, &synced.id);
    let tip = tip(&home, &synced.id);
    assert_eq!(tip.native_path, path);
    let native: Vec<&RehydrateMessage> =
        synced.rows.iter().filter(|r| !r.source_id.starts_with("syn-")).collect();
    assert_eq!(tip.tip_source_id.as_deref(), native.last().map(|r| r.source_id.as_str()));
    assert_eq!(
        tip.known_source_ids,
        native.iter().map(|r| r.source_id.clone()).collect::<HashSet<_>>()
    );
}

/// A copy that stopped short is caught up in place: the missing rows go on the end of it at
/// their numbers, as a rollout rehydrated whole has them, and Codex continues from the last.
/// Rows the copy holds already may be sent again: they are skipped.
#[rstest]
#[case::only_the_missing(0)]
#[case::overlapping(4)]
#[tokio::test]
async fn a_copy_behind_is_fast_forwarded(
    #[future] synced: Synced,
    home: TempDir,
    #[case] overlap: usize,
) {
    let synced = synced.await;
    let behind = synced.before(SECOND_TURN);
    let path = local_copy(&home, &synced.id, behind.clone());
    let base = tip(&home, &synced.id);

    let missing = synced.rows[behind.len() - overlap..].to_vec();
    let outcome = append_in(home.path(), &synced.id, &base, &missing, &AppendOptions::default())
        .await
        .unwrap();
    assert_eq!(outcome.native_path, path);
    assert!(!outcome.marked_tip);
    let caught_up = ids(&synced.from(SECOND_TURN));
    let written: Vec<String> =
        caught_up.into_iter().filter(|id| outcome.appended.contains(id)).collect();
    assert!(!written.is_empty());
    assert_eq!(outcome.appended, written, "only the missing rows, in order");
    assert_eq!(outcome.tip_source_id, outcome.appended.last().cloned());

    // The same rollout as one rehydrated whole, row for row and number for number (the turn
    // events around them aside: a turn the copy ended is not the one a whole rollout would
    // have gone on with).
    let whole = tempfile::tempdir().unwrap();
    let expected = local_copy(&whole, &synced.id, synced.rows.clone());
    let rows_of = |path: &Path| -> Vec<Value> {
        lines(path).into_iter().filter(|l| l["type"] != "event_msg").collect()
    };
    assert_eq!(rows_of(&path), rows_of(&expected));
    assert_paginated(&path);

    let again = recapture(&home, &synced.id).await;
    assert_nothing_new(&synced.rows, again.iter().map(|r| r.source_id.as_str()));
    for row in &again {
        let at = synced.rows.iter().find(|r| r.source_id == row.source_id).unwrap();
        assert_eq!(row.seq, at.seq, "{} moved", row.source_id);
    }
    assert_eq!(tip(&home, &synced.id).tip_source_id, outcome.tip_source_id);
}

/// Where another host went on from an earlier line (its lines numbered as the ones here past
/// that point), the branch becomes a new segment of the same thread, the way Codex writes a
/// revert: its header takes the number before the branch's first row, and its history continues
/// this rollout just past the line below that. Nothing is copied, the thread resumes from the
/// branch, and re-capturing every rollout finds nothing new.
#[rstest]
#[case::placed_by_number(false)]
#[case::hanging_from_a_line(true)]
#[tokio::test]
async fn a_branch_from_elsewhere_is_a_new_segment(
    #[future] synced: Synced,
    home: TempDir,
    #[case] hint: bool,
) {
    let synced = synced.await;
    let path = original(&home, &synced.id);
    let base = tip(&home, &synced.id);
    let shared = synced.before(SECOND_TURN);
    let mut branch = other_turn(SECOND_TURN);
    let hangs_from = shared.iter().rfind(|r| !r.source_id.starts_with("syn-")).unwrap();
    if hint {
        branch[0].parent_source_id = Some(hangs_from.source_id.clone());
    }
    // The caller may send the rows both hosts share too: the thread holds them.
    let mut sent = shared.clone();
    sent.extend(branch.clone());

    let outcome =
        append_in(home.path(), &synced.id, &base, &sent, &AppendOptions::default()).await.unwrap();
    let segment = outcome.native_path.clone();
    assert_ne!(segment, path);
    assert_eq!(outcome.appended, ids(&branch));
    assert_eq!(outcome.tip_source_id.as_deref(), Some("msg_elsewhere_a"));
    assert_eq!(locate(&sessions(&home), &synced.id), Some(segment.clone()));
    let name = RolloutName::of(&segment).unwrap();
    assert_eq!(name.thread, synced.id);
    assert_ne!(name.rollout, synced.id);

    let written = lines(&segment);
    let original = std::fs::read(&path).unwrap();
    let header = &written[0]["payload"];
    assert_eq!(header["id"], synced.id.as_str());
    assert_eq!(header["history_mode"], "paginated");
    let at = &header["history_base"];
    assert_eq!(at["thread_id"], synced.id.as_str());
    let exclusive = at["end_ordinal_exclusive"].as_u64().unwrap();
    let offset = usize::try_from(at["end_byte_offset"].as_u64().unwrap()).unwrap();
    assert_eq!(written[0]["ordinal"].as_u64(), Some(exclusive));
    assert_eq!(original[offset - 1], b'\n', "the history ends at a line's end");
    let kept: Vec<Value> = original[..offset]
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap())
        .collect();
    // Just past the first turn's end, where a revert of the second would continue: what this
    // host wrote to start its next turn is not the branch's.
    let last_kept = kept.last().unwrap();
    assert_eq!(last_kept["payload"]["type"], "task_complete");
    assert_eq!(exclusive, last_kept["ordinal"].as_u64().unwrap() + 1);
    assert!(exclusive < SECOND_TURN);
    // The branch's rows at their numbers.
    let numbered: Vec<(Option<&str>, u64)> = written
        .iter()
        .map(|l| (l["payload"]["id"].as_str(), l["ordinal"].as_u64().unwrap()))
        .collect();
    for row in &branch {
        assert!(numbered.contains(&(Some(row.source_id.as_str()), row.seq.unwrap())));
    }
    // The thread goes on from the branch, and nothing of this host's second turn is in it.
    let after = tip(&home, &synced.id);
    assert_eq!(after.native_path, segment);
    assert_eq!(after.tip_source_id.as_deref(), Some("msg_elsewhere_a"));
    assert!(after.known_source_ids.contains("msg_elsewhere_u"));
    assert!(after.known_source_ids.contains(&hangs_from.source_id));
    for row in synced.from(SECOND_TURN) {
        assert!(!after.known_source_ids.contains(&row.source_id), "{}", row.source_id);
    }

    let mut everything = synced.rows.clone();
    everything.extend(branch);
    let again = recapture(&home, &synced.id).await;
    // The segment's header is the thread's own `session_meta` again.
    let mut once_more = everything.clone();
    once_more.push(everything[0].clone());
    assert_nothing_new(&once_more, again.iter().map(|r| r.source_id.as_str()));
    let segment_rows = captured(&synced.id, &read(&synced.id, segment).await);
    assert_eq!(ids(&segment_rows), [
        synced.id.clone(),
        "msg_elsewhere_u".to_owned(),
        "msg_elsewhere_a".to_owned()
    ]);
}

/// A branch hanging from a line within a turn continues the history just past that line.
#[rstest]
#[tokio::test]
async fn a_branch_from_within_a_turn_continues_past_its_line(
    #[future] synced: Synced,
    home: TempDir,
) {
    let synced = synced.await;
    original(&home, &synced.id);
    let base = tip(&home, &synced.id);
    let prompt = synced.from(SECOND_TURN).into_iter().find(|r| r.role == Role::User).unwrap();
    let at = prompt.seq.unwrap();
    let mut branch = other_turn(at + 1);
    branch[0].role = Role::Assistant;
    branch[0].parent_source_id = Some(prompt.source_id.clone());
    let outcome = append_in(home.path(), &synced.id, &base, &branch, &AppendOptions::default())
        .await
        .unwrap();
    let header = &lines(&outcome.native_path)[0];
    assert_eq!(header["ordinal"].as_u64(), Some(at + 1));
    assert_eq!(header["payload"]["history_base"]["end_ordinal_exclusive"].as_u64(), Some(at + 1));
    let after = tip(&home, &synced.id);
    assert!(after.known_source_ids.contains(&prompt.source_id));
    assert_eq!(after.tip_source_id.as_deref(), Some("msg_elsewhere_a"));
}

/// With nothing to append, a line of a branch the thread already has here becomes the tip: a
/// segment of nothing but its header, continuing just past it.
#[rstest]
#[tokio::test]
async fn a_branch_already_here_is_switched_to(#[future] synced: Synced, home: TempDir) {
    let synced = synced.await;
    let path = original(&home, &synced.id);
    let base = tip(&home, &synced.id);
    let ours = base.tip_source_id.clone().unwrap();
    let options = AppendOptions::default();
    append_in(home.path(), &synced.id, &base, &other_turn(SECOND_TURN), &options).await.unwrap();

    let base = tip(&home, &synced.id);
    let options = AppendOptions {
        head: Some(&ours),
        ..AppendOptions::default()
    };
    let outcome = append_in(home.path(), &synced.id, &base, &[], &options).await.unwrap();
    assert!(outcome.marked_tip);
    assert!(outcome.appended.is_empty());
    assert_eq!(outcome.tip_source_id.as_deref(), Some(ours.as_str()));
    let written = lines(&outcome.native_path);
    assert_eq!(written.len(), 1, "a header and nothing else");
    let at = &written[0]["payload"]["history_base"];
    let read = read_lines(&std::fs::read(&path).unwrap(), &SessionId::from(synced.id.clone()));
    let line = read.iter().find(|l| l.message.id().is_some_and(|id| id.as_ref() == ours)).unwrap();
    assert_eq!(at["end_byte_offset"].as_u64(), Some(line.end));
    assert_eq!(at["end_ordinal_exclusive"].as_u64(), line.message.seq().map(|seq| seq + 1));

    let after = tip(&home, &synced.id);
    assert_eq!(after.native_path, outcome.native_path);
    assert_eq!(after.tip_source_id.as_deref(), Some(ours.as_str()));
    assert!(!after.known_source_ids.contains("msg_elsewhere_u"));
    // Already the tip: nothing to do.
    let again = append_in(home.path(), &synced.id, &after, &[], &options).await.unwrap();
    assert_eq!(again.native_path, after.native_path);
    assert!(!again.marked_tip);
    assert_eq!(rollouts_of(&sessions(&home), &synced.id).len(), 3);

    let recaptured = recapture(&home, &synced.id).await;
    let mut everything = synced.rows.clone();
    everything.extend(other_turn(SECOND_TURN));
    everything.extend([everything[0].clone(), everything[0].clone()]);
    assert_nothing_new(&everything, recaptured.iter().map(|r| r.source_id.as_str()));
}

/// A Codex holding the thread's writer lock is never written under.
#[rstest]
#[tokio::test]
async fn a_thread_codex_holds_is_refused(#[future] synced: Synced, home: TempDir) {
    let synced = synced.await;
    let path = local_copy(&home, &synced.id, synced.before(SECOND_TURN));
    let before = std::fs::read(&path).unwrap();
    let base = tip(&home, &synced.id);
    let locks = home.path().join(LOCKS);
    std::fs::create_dir_all(&locks).unwrap();
    let held = File::create(locks.join(format!("{}.lock", synced.id))).unwrap();
    held.lock().unwrap();
    let missing = synced.from(SECOND_TURN);
    let options = AppendOptions::default();
    let refused = append_in(home.path(), &synced.id, &base, &missing, &options).await;
    assert!(matches!(refused, Err(SyncError::Live(_))), "{refused:?}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    drop(held);
    append_in(home.path(), &synced.id, &base, &missing, &options).await.unwrap();
    assert!(!locks.join(format!("{}.lock", synced.id)).exists(), "the lock is let go");
}

/// A rollout that changed since its tip was read (Codex went on, or reverted) is not written to.
#[rstest]
#[case::appended_to(false)]
#[case::a_newer_segment(true)]
#[tokio::test]
async fn a_changed_rollout_is_refused(
    #[future] synced: Synced,
    home: TempDir,
    #[case] segment: bool,
) {
    let synced = synced.await;
    let path = local_copy(&home, &synced.id, synced.before(SECOND_TURN));
    let base = tip(&home, &synced.id);
    if segment {
        let newer = path.with_file_name(format!(
            "rollout-2099-01-01T00-00-00-{}_01a0ffff-0000-7000-8000-000000000000.jsonl",
            synced.id
        ));
        std::fs::copy(&path, newer).unwrap();
    } else {
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(&mut file, b"{}\n").unwrap();
    }
    let missing = synced.from(SECOND_TURN);
    let refused =
        append_in(home.path(), &synced.id, &base, &missing, &AppendOptions::default()).await;
    assert!(matches!(refused, Err(SyncError::Changed)), "{refused:?}");
}

/// A rollout in Codex's legacy history mode is caught up unnumbered, as Codex continues one,
/// but cannot take a branch.
#[rstest]
#[tokio::test]
async fn a_legacy_rollout_is_caught_up_but_never_branched(#[future] synced: Synced, home: TempDir) {
    let synced = synced.await;
    let path = local_copy(&home, &synced.id, synced.before(SECOND_TURN));
    // As a Codex from before paginated history writes it.
    let legacy: String = lines(&path)
        .into_iter()
        .map(|mut line| {
            let line = line.as_object_mut().unwrap();
            line.remove("ordinal");
            if let Some(payload) = line.get_mut("payload").and_then(Value::as_object_mut) {
                payload.remove("history_mode");
            }
            format!("{}\n", Value::Object(line.clone()))
        })
        .collect();
    std::fs::write(&path, legacy).unwrap();
    let base = tip(&home, &synced.id);
    let options = AppendOptions::default();
    let mut branch = other_turn(3);
    branch[0].parent_source_id = Some(synced.before(SECOND_TURN)[1].source_id.clone());
    let refused = append_in(home.path(), &synced.id, &base, &branch, &options).await;
    assert!(matches!(refused, Err(SyncError::Unsupported(_))), "{refused:?}");

    let missing = synced.from(SECOND_TURN);
    let outcome = append_in(home.path(), &synced.id, &base, &missing, &options).await.unwrap();
    assert_eq!(outcome.native_path, path);
    assert!(lines(&path).iter().all(|l| l.get("ordinal").is_none()));
    let again = recapture(&home, &synced.id).await;
    assert_nothing_new(&synced.rows, again.iter().map(|r| r.source_id.as_str()));
}

/// Rows captured before numbers were are caught up at free numbers past the copy's end.
#[rstest]
#[tokio::test]
async fn unnumbered_rows_are_numbered_on(#[future] synced: Synced, home: TempDir) {
    let synced = synced.await;
    let path = local_copy(&home, &synced.id, synced.before(SECOND_TURN));
    let last = *assert_paginated(&path).last().unwrap();
    let base = tip(&home, &synced.id);
    let mut missing = synced.from(SECOND_TURN);
    for row in &mut missing {
        row.seq = None;
    }
    append_in(home.path(), &synced.id, &base, &missing, &AppendOptions::default()).await.unwrap();
    let ordinals = assert_paginated(&path);
    assert_eq!(
        ordinals.iter().filter(|o| **o > last).count(),
        lines(&path).len() - ordinals.iter().filter(|o| **o <= last).count()
    );
    let again = recapture(&home, &synced.id).await;
    assert_nothing_new(&synced.rows, again.iter().map(|r| r.source_id.as_str()));
}

/// Codex's thread index is pointed at a new segment where it named the rollout the segment
/// continues; where it names another, the index moved on meanwhile, and nothing is left behind.
#[rstest]
#[case::named_what_it_continues(true)]
#[case::named_another(false)]
#[tokio::test]
async fn the_thread_index_follows_a_branch(
    #[future] synced: Synced,
    home: TempDir,
    #[case] current: bool,
) {
    use sqlx::Connection as _;
    let synced = synced.await;
    let path = original(&home, &synced.id);
    let other = home.path().join("elsewhere.jsonl");
    std::fs::write(&other, "{}\n").unwrap();
    let named = if current {
        path.clone()
    } else {
        other
    };
    let db = index(home.path(), None).await;
    let opts = sqlx::sqlite::SqliteConnectOptions::new().filename(&db);
    let mut conn = sqlx::SqliteConnection::connect_with(&opts).await.unwrap();
    crate::db::query::<sqlx::Sqlite>("INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)")
        .bind(&synced.id)
        .bind(named.to_string_lossy().into_owned())
        .execute(&mut conn)
        .await
        .unwrap();

    let base = tip(&home, &synced.id);
    let branch = other_turn(SECOND_TURN);
    let appended =
        append_in(home.path(), &synced.id, &base, &branch, &AppendOptions::default()).await;
    let stored = crate::db::query_scalar::<sqlx::Sqlite, String>(
        "SELECT rollout_path FROM threads WHERE id = ?1",
    )
    .bind(&synced.id)
    .fetch_one(&mut conn)
    .await
    .unwrap();
    if current {
        assert_eq!(stored, appended.unwrap().native_path.to_string_lossy());
    } else {
        assert!(matches!(appended, Err(SyncError::Changed)), "{appended:?}");
        assert_eq!(rollouts_of(&sessions(&home), &synced.id), vec![path]);
    }
}
