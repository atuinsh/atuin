use pretty_assertions::assert_eq;
use rstest::rstest;
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::rehydrate::testing;
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
