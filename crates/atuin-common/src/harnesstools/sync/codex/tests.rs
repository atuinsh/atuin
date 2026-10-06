use pretty_assertions::assert_eq;
use rstest::rstest;
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::rehydrate::testing;
use crate::harnesstools::session::Message as _;
use crate::harnesstools::sync::TableEntry;
use crate::harnesstools::sync::liveness::tests::{Fake, cwd, dir, entry, procs, table};

const PAGINATED: &str = "paginated-compacted.jsonl";
const LEGACY: &str = "legacy-forked-subagent.jsonl";

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex").join(name);
    std::fs::read_to_string(path).unwrap()
}

/// The thread a rollout's first line (its own `session_meta`) names.
fn thread(text: &str) -> String {
    let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    first["payload"]["id"].as_str().unwrap().to_owned()
}

/// Write the first `lines` lines of `text` as thread `thread`'s rollout under Codex home `home`.
fn rollout(home: &Path, thread: &str, text: &str, lines: usize) -> PathBuf {
    let dir = home.join("sessions/2026/09/24");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("rollout-2026-09-24T02-47-48-{thread}.jsonl"));
    let kept: String = text.lines().take(lines).map(|l| format!("{l}\n")).collect();
    std::fs::write(&path, kept).unwrap();
    path
}

/// The rows capture syncs of the rollout at `path`, each linked to the one before it.
async fn synced(path: &Path) -> Vec<RehydrateMessage> {
    let session = session_id_of(path.file_stem().unwrap().to_str().unwrap());
    let pool = BlockingPool::new(std::num::NonZeroUsize::MIN);
    let lines: Vec<_> =
        CodexSession::open(session.clone(), path.to_path_buf(), pool).read().collect().await;
    let mut occurrences = HashMap::new();
    let mut rows: Vec<RehydrateMessage> = Vec::new();
    let mut at = OffsetDateTime::UNIX_EPOCH;
    for m in lines.into_iter().map(Result::unwrap) {
        let Some(id) = capture_key(&session, &m, &mut occurrences) else {
            continue;
        };
        at = m.timestamp().unwrap_or(at);
        rows.push(RehydrateMessage {
            source_id: id,
            parent_source_id: rows.last().map(|r| r.source_id.clone()),
            timestamp: at,
            role: m.role(),
            content: m.content(),
            model: m.model(),
            usage: m.usage(),
            stop_reason: m.stop_reason(),
            turn_id: m.turn_id(),
            cwd: m.cwd(),
            git_branch: m.git_branch(),
        });
    }
    testing::synced(rows)
}

/// The ordinals of the rollout at `path`, in order.
fn ordinals(path: &Path) -> Vec<Option<u64>> {
    let bytes = std::fs::read(path).unwrap();
    jsonl_lines(&bytes)
        .map(|l| serde_json::from_slice::<Value>(l).unwrap()["ordinal"].as_u64())
        .collect()
}

/// A home holding the paginated fixture cut after `cut` lines, the whole of it synced: the
/// rollout, its tip, and the rows past the tip.
async fn behind(home: &Path, cut: usize) -> (String, PathBuf, LocalTip, Vec<RehydrateMessage>) {
    let text = fixture(PAGINATED);
    let thread = thread(&text);
    let whole = tempfile::tempdir().unwrap();
    let all = synced(&rollout(whole.path(), &thread, &text, usize::MAX)).await;
    let path = rollout(home, &thread, &text, cut);
    let base = local_tip_in(home, &thread).await.unwrap().unwrap();
    let tip = base.tip_source_id.clone().unwrap();
    let at = all.iter().position(|r| r.source_id == tip).expect("the tip is synced");
    (thread, path, base, all[at + 1..].to_vec())
}

/// The copy works where its newest segment's `session_meta` says.
#[rstest]
#[tokio::test]
async fn the_tip_records_where_the_copy_works(dir: TempDir) {
    let (_, _, base, _) = behind(dir.path(), 20).await;
    assert_eq!(base.cwd, Some(PathBuf::from("/work")));
}

/// Appending the rows past the tip numbers each new line on from the rollout's last; Codex
/// resumes from the last row, and capture reads back nothing it did not sync. The thread's
/// writer lock is taken while writing, where Codex keeps them, and let go after.
#[rstest]
#[case::mid_session(20, false)]
#[case::near_the_end(45, false)]
#[case::under_the_writer_lock(30, true)]
#[tokio::test]
async fn a_fast_forward_numbers_on_from_the_last_line(
    dir: TempDir,
    #[case] cut: usize,
    #[case] locks: bool,
) {
    if locks {
        std::fs::create_dir_all(dir.path().join(LOCKS)).unwrap();
    }
    let (thread, path, base, missing) = behind(dir.path(), cut).await;
    assert!(!missing.is_empty());
    let before = ordinals(&path);

    let outcome = append_in(dir.path(), &thread, &base, &missing, None).unwrap();

    let after = ordinals(&path);
    let last = before.last().copied().flatten().unwrap();
    let expected: Vec<Option<u64>> =
        (last + 1..).take(after.len() - before.len()).map(Some).collect();
    assert_eq!(after[before.len()..], expected[..]);
    let caught_up = local_tip_in(dir.path(), &thread).await.unwrap().unwrap();
    assert_eq!(caught_up.tip_source_id, outcome.tip_source_id);
    assert_eq!(outcome.tip_source_id.as_ref(), outcome.appended.last());
    let mut all: Vec<RehydrateMessage> = base
        .known_source_ids
        .iter()
        .map(|id| RehydrateMessage {
            source_id: id.clone(),
            ..missing[0].clone()
        })
        .collect();
    all.extend(missing);
    let again = synced(&path).await;
    testing::assert_nothing_new(&all, again.iter().map(|r| r.source_id.as_str()));
    if locks {
        assert!(!dir.path().join(LOCKS).join(format!("{thread}.lock")).exists(), "let go");
    }
}

/// A Codex holding the thread's writer lock keeps it from being written.
#[rstest]
#[tokio::test]
async fn a_thread_codex_holds_is_left_alone(dir: TempDir) {
    let locks = dir.path().join(LOCKS);
    std::fs::create_dir_all(&locks).unwrap();
    let (thread, path, base, missing) = behind(dir.path(), 20).await;
    let held = File::create(locks.join(format!("{thread}.lock"))).unwrap();
    held.lock().unwrap();
    let before = std::fs::read(&path).unwrap();
    let err = append_in(dir.path(), &thread, &base, &missing, None).unwrap_err();
    assert!(matches!(err, SyncError::Live(None)), "{err:?}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let procs = procs(dir.path(), &[]);
    assert_eq!(liveness(dir.path(), &thread, &procs, &Seen::default()), Liveness::Live {
        pid: None
    });
}

#[rstest]
#[case::changed_since("Changed")]
#[case::off_an_older_row("Unsupported")]
#[tokio::test]
async fn what_is_no_clean_fast_forward_is_refused(dir: TempDir, #[case] expected: &str) {
    let (thread, path, base, mut missing) = behind(dir.path(), 20).await;
    if expected == "Changed" {
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(b"{}\n");
        std::fs::write(&path, bytes).unwrap();
    } else {
        missing[0].parent_source_id = base
            .known_source_ids
            .iter()
            .find(|id| Some(*id) != base.tip_source_id.as_ref())
            .cloned();
    }
    let before = std::fs::read(&path).unwrap();
    let err = append_in(dir.path(), &thread, &base, &missing, None).unwrap_err();
    assert!(format!("{err:?}").starts_with(expected), "{err:?}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// Another machine's branch of the paginated fixture's thread: its rows up to where the rollout
/// cut after `cut` lines ends, then a reply of its own; and that reply, the head.
async fn their_branch(cut: usize) -> (Vec<RehydrateMessage>, String) {
    let elsewhere = tempfile::tempdir().unwrap();
    let (_, _, base, _) = behind(elsewhere.path(), cut).await;
    let text = fixture(PAGINATED);
    let all = synced(&rollout(elsewhere.path(), &thread(&text), &text, usize::MAX)).await;
    let tip = base.tip_source_id.unwrap();
    let at = all.iter().position(|r| r.source_id == tip).unwrap();
    let mut rows = all[..=at].to_vec();
    let mut reply = all
        .iter()
        .find(|r| {
            r.role == crate::harnesstools::session::Role::Assistant
                && !r.source_id.starts_with("syn-")
        })
        .unwrap()
        .clone();
    reply.source_id = "msg_theirs".to_owned();
    reply.parent_source_id = Some(tip);
    reply.content = vec![crate::harnesstools::session::Content::Text("their reply".to_owned())];
    rows.push(reply);
    (rows, "msg_theirs".to_owned())
}

/// Switching the copy (which went on here past where the other branch leaves it) to another
/// machine's branch keeps its rollout up to the line where the branch leaves it as it was, byte
/// for byte (its `session_meta`, and what capture never kept: a reasoning item's encrypted
/// content), drops what it went on with here, and appends the branch's lines past it, numbered on
/// from the line kept last; Codex resumes from the branch's head, and capture reads back nothing
/// the branch doesn't hold. In place, under the thread's writer lock where Codex keeps them, with
/// the rollout as it was kept whole outside Codex's sessions.
#[rstest]
#[case::without_locks(false)]
#[case::under_the_writer_lock(true)]
#[tokio::test]
async fn a_switch_writes_the_rollout_out_along_the_branch_in_place(
    dir: TempDir,
    #[case] locks: bool,
) {
    if locks {
        std::fs::create_dir_all(dir.path().join(LOCKS)).unwrap();
    }
    let backups = tempfile::tempdir().unwrap();
    let (thread, path, base, _) = behind(dir.path(), 45).await;
    let before = std::fs::read_to_string(&path).unwrap();
    let (rows, head) = their_branch(30).await;

    let outcome =
        replace_in(dir.path(), &thread, &base, &rows, None, backups.path()).await.unwrap();

    assert_eq!(outcome.native_path, path);
    assert_eq!(outcome.appended, [head.as_str()]);
    assert_eq!(outcome.tip_source_id.as_deref(), Some(head.as_str()));
    let text = std::fs::read_to_string(&path).unwrap();
    // The branch leaves the copy at its 30th line, the last it shares.
    let shared: String = before.lines().take(30).map(|l| format!("{l}\n")).collect();
    assert!(text.starts_with(&shared), "the shared lines, byte for byte");
    assert!(shared.contains(r#""encrypted_content":"ZW5jcnlwdGVk""#));
    assert_eq!(text.lines().count(), 31, "then the branch's reply");
    assert_eq!(outcome.backup.parent(), Some(backups.path()));
    assert_eq!(std::fs::read_to_string(&outcome.backup).unwrap(), before);
    assert!(!outcome.backup.starts_with(dir.path()), "not among Codex's sessions");
    let numbers: Vec<u64> = ordinals(&path).into_iter().map(Option::unwrap).collect();
    assert_eq!(numbers, (0..numbers.len() as u64).collect::<Vec<_>>());
    let switched = local_tip_in(dir.path(), &thread).await.unwrap().unwrap();
    assert_eq!(switched.tip_source_id.as_deref(), Some(head.as_str()));
    assert_eq!(switched.native_path, path);
    let again = synced(&path).await;
    testing::assert_nothing_new(&rows, again.iter().map(|r| r.source_id.as_str()));
    if locks {
        assert!(!dir.path().join(LOCKS).join(format!("{thread}.lock")).exists(), "let go");
    }
}

/// Nothing is written to a thread kept in several rollouts (reverted, or named by a segment of
/// it), to a rollout changed since it was read, or one Codex holds the writer lock of.
#[rstest]
#[case::two_rollouts("two rollouts", "Unsupported")]
#[case::a_segment("a segment", "Unsupported")]
#[case::changed("changed", "Changed")]
#[case::locked("locked", "Live")]
#[tokio::test]
async fn a_switch_that_cant_be_made_writes_nothing(
    dir: TempDir,
    #[case] how: &str,
    #[case] expected: &str,
) {
    let text = fixture(PAGINATED);
    let thread = thread(&text);
    if how == "two rollouts" {
        // The thread's first rollout, which a newer one continues.
        let older = dir.path().join("sessions/2026/09/23");
        std::fs::create_dir_all(&older).unwrap();
        std::fs::write(older.join(format!("rollout-2026-09-23T00-00-00-{thread}.jsonl")), &text)
            .unwrap();
    }
    let (_, path, base, _) = behind(dir.path(), 45).await;
    let mut id = thread.clone();
    let mut held = None;
    match how {
        "a segment" => id = format!("{thread}_01a0d14f-0000-7000-8000-000000000000"),
        "changed" => {
            let mut bytes = std::fs::read(&path).unwrap();
            bytes.extend_from_slice(b"{}\n");
            std::fs::write(&path, bytes).unwrap();
        }
        "locked" => {
            let locks = dir.path().join(LOCKS);
            std::fs::create_dir_all(&locks).unwrap();
            let lock = File::create(locks.join(format!("{thread}.lock"))).unwrap();
            lock.lock().unwrap();
            held = Some(lock);
        }
        _ => {}
    }
    let before = std::fs::read(&path).unwrap();
    let (rows, _) = their_branch(30).await;
    let backups = tempfile::tempdir().unwrap();
    let err = replace_in(dir.path(), &id, &base, &rows, None, backups.path()).await.unwrap_err();
    assert!(format!("{err:?}").starts_with(expected), "{err:?}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(backups.path()).unwrap().count(), 0, "no backup left");
    drop(held);
}

/// A rollout in the legacy history mode numbers no lines: Codex couldn't continue it with
/// numbered ones.
#[rstest]
#[tokio::test]
async fn a_legacy_rollout_is_not_appended_to(dir: TempDir) {
    let text = fixture(LEGACY);
    let thread = thread(&text);
    let path = rollout(dir.path(), &thread, &text, 20);
    let base = local_tip_in(dir.path(), &thread).await.unwrap().unwrap();
    let mut row = synced(&path).await.pop().unwrap();
    row.source_id = "msg_new".to_owned();
    row.parent_source_id.clone_from(&base.tip_source_id);
    let err = append_in(dir.path(), &thread, &base, &[row], None).unwrap_err();
    assert!(matches!(err, SyncError::Unsupported(_)), "{err:?}");
}

#[rstest]
#[case::locks_kept_none_held(true, b"codex\0".as_slice(), Liveness::NotLive)]
#[case::no_locks_a_codex_running(false, b"codex\0".as_slice(), Liveness::Unknown)]
#[case::no_locks_none_running(false, b"bash\0".as_slice(), Liveness::NotLive)]
fn liveness_is_the_writer_lock_or_any_codex(
    dir: TempDir,
    #[case] locks: bool,
    #[case] cmdline: &[u8],
    #[case] expected: Liveness,
) {
    if locks {
        std::fs::create_dir_all(dir.path().join(LOCKS)).unwrap();
    }
    let procs = procs(dir.path(), &[Fake::running(3, cmdline, "1")]);
    let thread = "01a0d14f-e276-77d3-b955-89d5b0151306";
    // On Windows, a lock nothing holds falls back on the processes: a Codex runs.
    let expected = if locks && cfg!(windows) {
        Liveness::Unknown
    } else {
        expected
    };
    assert_eq!(liveness(dir.path(), thread, &procs, &Seen::default()), expected);
}

/// Without locks, a Codex counts only where the session works.
#[rstest]
#[case::in_the_sessions_directory("proj", Liveness::Unknown)]
#[case::elsewhere("other", Liveness::NotLive)]
fn without_locks_a_codex_counts_where_the_session_works(
    dir: TempDir,
    #[case] works_in: &str,
    #[case] expected: Liveness,
) {
    let procs = table(&[TableEntry {
        cwd: Some(dir.path().join(works_in)),
        ..entry(3, "codex", &["codex"], 1)
    }]);
    let thread = "01a0d14f-e276-77d3-b955-89d5b0151306";
    assert_eq!(liveness(dir.path(), thread, &procs, &cwd(Some(dir.path().join("proj")))), expected);
}

/// Without locks, a Codex working elsewhere may have the thread loaded from there (`codex resume
/// <thread>`): it counts when its command line names the thread, or while the rollout was
/// modified lately.
#[rstest]
#[case::changed_lately_named_nowhere(10, false, Liveness::Unknown)]
#[case::changed_long_ago_named_nowhere(3600, false, Liveness::NotLive)]
#[case::changed_long_ago_named(3600, true, Liveness::Unknown)]
#[tokio::test]
async fn without_locks_a_codex_elsewhere_counts_when_named_or_writing(
    dir: TempDir,
    #[case] ago: u64,
    #[case] named: bool,
    #[case] expected: Liveness,
) {
    let text = fixture(PAGINATED);
    let thread = thread(&text);
    let path = rollout(dir.path(), &thread, &text, 20);
    let at = std::time::SystemTime::now() - std::time::Duration::from_secs(ago);
    File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
    let mut base = local_tip_in(dir.path(), &thread).await.unwrap().unwrap();
    assert_eq!(base.modified, Some(at));
    // Where the copy works, made to exist: the fixture records `/work`, which a missing directory
    // would leave unrestricted, and which exists on some machines but not others.
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    base.cwd = Some(proj);
    let cmd: &[&str] = if named {
        &["codex", "resume", &thread]
    } else {
        &["codex"]
    };
    let procs = table(&[TableEntry {
        cwd: Some(dir.path().join("other")),
        ..entry(3, "codex", cmd, 1)
    }]);
    let seen = appending(&base, &thread, Some(&dir.path().join("proj")));
    assert_eq!(liveness(dir.path(), &thread, &procs, &seen), expected);
}

/// What a switch would refuse whatever the branch is known from the copy alone, so none is
/// offered: a thread kept in several rollouts (or named by a segment of it), a segment continuing
/// another (`history_base`), a rollout in the legacy history mode.
#[rstest]
#[case::one_rollout("one rollout", None)]
#[case::two_rollouts("two rollouts", Some(SEGMENTED))]
#[case::a_segment("a segment", Some(SEGMENTED))]
#[case::continuing_another("history base", Some(SEGMENTED))]
#[case::legacy("legacy", Some(LEGACY_SWITCH))]
#[tokio::test]
async fn the_copy_says_up_front_when_it_cant_be_switched(
    dir: TempDir,
    #[case] how: &str,
    #[case] why: Option<&str>,
) {
    let text = fixture(if how == "legacy" {
        LEGACY
    } else {
        PAGINATED
    });
    let thread = thread(&text);
    let mut id = thread.clone();
    let mut text = text;
    match how {
        "two rollouts" => {
            let older = dir.path().join("sessions/2026/09/23");
            std::fs::create_dir_all(&older).unwrap();
            let name = format!("rollout-2026-09-23T00-00-00-{thread}.jsonl");
            std::fs::write(older.join(name), &text).unwrap();
        }
        "a segment" => id = format!("{thread}_01a0d14f-0000-7000-8000-000000000000"),
        "history base" => {
            let mut lines: Vec<Value> =
                text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
            lines[0]["payload"]["history_base"] =
                json!({"thread_id": "01a0d14f-0000-7000-8000-000000000001", "end_byte_offset": 9});
            text = lines.iter().map(|l| format!("{l}\n")).collect();
        }
        _ => {}
    }
    rollout(dir.path(), &thread, &text, 20);
    let tip = local_tip_in(dir.path(), &id).await.unwrap().unwrap();
    assert_eq!(tip.unswitchable, why);
}
