use rstest::rstest;
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::session::Role;
use crate::harnesstools::sync::liveness::tests::{dir, entry, table};

fn row(id: &str, parent: Option<&str>) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: parent.map(str::to_owned),
        timestamp: OffsetDateTime::UNIX_EPOCH,
        role: Role::User,
        content: Vec::new(),
        model: None,
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
    }
}

/// A transcript holding `a` then its tip `b`, and the call `c` merged into `b`'s line.
fn base() -> LocalTip {
    LocalTip {
        native_path: PathBuf::new(),
        known_source_ids: ["a", "b", "c"].map(str::to_owned).into(),
        merged: HashMap::from([("c".to_owned(), "b".to_owned())]),
        tip_source_id: Some("b".to_owned()),
        stamp: Stamp::of(b""),
        modified: None,
        cwd: None,
        unswitchable: None,
    }
}

#[rstest]
#[case::from_the_tip(vec![row("d", Some("b")), row("e", Some("d"))], Links::Parents, None)]
#[case::from_a_row_merged_into_it(vec![row("d", Some("c"))], Links::Parents, None)]
#[case::from_an_older_line(vec![row("d", Some("a"))], Links::Parents, Some("Unsupported"))]
#[case::from_nothing(vec![row("d", None)], Links::Parents, Some("Unsupported"))]
#[case::chained_from_nothing(vec![row("d", None), row("e", None)], Links::Chained, None)]
#[case::chained_from_an_older_line(vec![row("d", Some("a"))], Links::Chained, Some("Unsupported"))]
#[case::in_order(vec![row("d", Some("zz"))], Links::Linear, None)]
#[case::an_id_held(vec![row("a", Some("b"))], Links::Parents, Some("IdTaken"))]
#[case::an_id_twice(vec![row("d", Some("b")), row("d", Some("d"))], Links::Parents, Some("IdTaken"))]
#[case::beside_the_tree(vec![row("syn-1", None), row("d", Some("b"))], Links::Parents, None)]
#[case::beside_the_tree_after_a_row(vec![row("d", Some("b")), row("syn-1", None)], Links::Parents,
    None)]
fn only_rows_continuing_the_tip_fast_forward(
    #[case] lines: Vec<RehydrateMessage>,
    #[case] links: Links,
    #[case] refused: Option<&str>,
) {
    let found = check_segment(&lines, &base(), None, links);
    match refused {
        None => assert!(found.is_ok(), "{found:?}"),
        Some(why) => assert!(format!("{found:?}").starts_with(&format!("Err({why}")), "{found:?}"),
    }
}

/// An append checks for a harness where the transcript records the session working and where
/// the caller last saw it, never only the first: and anywhere when either is not known, the
/// recorded directory gone included.
#[rstest]
#[case::in_the_recorded_directory(Some("rec"), Some("given"), "rec", Liveness::Unknown)]
#[case::in_the_callers_directory(Some("rec"), Some("given"), "given", Liveness::Unknown)]
#[case::in_neither(Some("rec"), Some("given"), "other", Liveness::NotLive)]
#[case::in_the_same_one(Some("rec"), Some("rec"), "rec", Liveness::Unknown)]
#[case::elsewhere_than_the_same_one(Some("rec"), Some("rec"), "other", Liveness::NotLive)]
#[case::caller_saw_nowhere(Some("rec"), None, "other", Liveness::Unknown)]
#[case::recorded_gone(Some("gone"), Some("given"), "other", Liveness::Unknown)]
#[case::recorded_gone_caller_saw_nowhere(Some("gone"), None, "other", Liveness::Unknown)]
#[case::recorded_nowhere(None, Some("given"), "other", Liveness::Unknown)]
#[case::known_nowhere(None, None, "other", Liveness::Unknown)]
fn an_append_looks_where_the_transcript_and_its_caller_work(
    dir: TempDir,
    #[case] recorded: Option<&str>,
    #[case] given: Option<&str>,
    #[case] works_in: &str,
    #[case] expected: Liveness,
) {
    for sub in ["rec", "given", "other"] {
        std::fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    let base = LocalTip {
        cwd: recorded.map(|d| dir.path().join(d)),
        // Not changing: only where a harness works tells.
        modified: Some(SystemTime::now() - 2 * RECENT),
        ..base()
    };
    let given = given.map(|d| dir.path().join(d));
    let seen = base.seen(given.as_deref(), Vec::new());
    let procs = table(&[TableEntry {
        cwd: Some(dir.path().join(works_in)),
        ..entry(7, "pi", &["pi"], 1)
    }]);
    assert_eq!(liveness::agent_running(&procs, "pi", &[], &seen), expected);
}

/// A switched transcript resumes from the head when the harness's tip is it, or the line it was
/// merged into; any other line is refused, as is an empty transcript.
#[rstest]
#[case::the_tip("b", Some("b"), true)]
#[case::merged_into_the_tip("c", Some("b"), true)]
#[case::another_row("a", Some("b"), false)]
#[case::empty("b", None, false)]
fn a_switch_resumes_from_the_head(
    #[case] head: &str,
    #[case] tip: Option<&str>,
    #[case] resumes: bool,
) {
    let merged = base().merged;
    assert_eq!(resumes_from(tip, &merged, head).is_ok(), resumes);
}

/// opencode keeps its sessions in a database: never switched, whatever is asked.
#[rstest]
#[tokio::test]
async fn opencode_is_never_switched() {
    let backups = tempfile::tempdir().unwrap();
    let harness = AnyHarness::Opencode(crate::harnesstools::opencode::Opencode);
    let options = AppendOptions::default();
    let (base, branch) = (base(), [row("a", None)]);
    let replaced = harness.replace("ses_1", &base, &branch, &options, backups.path());
    let err = replaced.await.unwrap_err();
    assert!(matches!(err, SyncError::Unsupported(_)), "{err:?}");
    assert_eq!(std::fs::read_dir(backups.path()).unwrap().count(), 0);
}

/// A backup is named for the transcript and when it was switched, in the backups directory, and
/// never written over another: a name taken (two switches in the same millisecond) gets a number.
#[rstest]
fn backups_never_overwrite_each_other() {
    let backups = tempfile::tempdir().unwrap();
    let dir = backups.path().join("claude-code");
    let path = Path::new("/home/me/.claude/projects/-work/5e55.jsonl");
    let at = time::macros::datetime!(2026-10-05 12:34:56.789 UTC);
    let first = keep_backup(path, b"first\n", &dir, at).unwrap();
    assert_eq!(first, dir.join("5e55-20261005T123456.789Z.jsonl"));
    let second = keep_backup(path, b"second\n", &dir, at).unwrap();
    assert_eq!(second, dir.join("5e55-20261005T123456.789Z-1.jsonl"));
    assert_eq!(std::fs::read(&first).unwrap(), b"first\n");
    assert_eq!(std::fs::read(&second).unwrap(), b"second\n");
    // A rollout keeps its whole name, and its extension.
    let rollout = Path::new("/c/sessions/2026/10/05/rollout-2026-10-05T12-00-00-0199.jsonl");
    let named = backup_path(rollout, &dir, at, 0).unwrap();
    assert_eq!(named, dir.join("rollout-2026-10-05T12-00-00-0199-20261005T123456.789Z.jsonl"));
}

/// Where a branch leaves a transcript: after the first of its rows the transcript holds, at the
/// line the last of them is or was merged into; refused when it holds none, or holds rows of the
/// branch past where it leaves. Rows beside the tree (`syn-`: titles, pi's prompts from before
/// ids, which go with a branch without being on it) say nothing of where it leaves, held or not;
/// Codex's content-keyed rows are on its tree like any other.
#[rstest]
#[case::after_the_tip(&["a", "b", "d"], Links::Parents, Ok((2, "b")))]
#[case::merged_into_a_line(&["a", "b", "c", "d"], Links::Parents, Ok((3, "b")))]
#[case::shares_nothing(&["x", "y"], Links::Parents, Err("shares nothing"))]
#[case::holds_rows_past_it(&["a", "x", "b"], Links::Parents, Err("past where it leaves"))]
#[case::beside_the_tree_not_held(&["a", "syn-t", "b", "d"], Links::Parents, Ok((3, "b")))]
#[case::beside_the_tree_after_it(&["a", "b", "syn-t", "d"], Links::Parents, Ok((2, "b")))]
#[case::beside_the_tree_first(&["syn-p", "a", "b", "d"], Links::Parents, Ok((3, "b")))]
#[case::only_beside_the_tree(&["syn-p", "x"], Links::Parents, Err("shares nothing"))]
#[case::codex_content_keyed(&["a", "syn-t", "b"], Links::Chained, Err("past where it leaves"))]
fn a_branch_leaves_a_transcript_where_it_stops_holding_it(
    #[case] branch: &[&str],
    #[case] links: Links,
    #[case] expected: Result<(usize, &str), &str>,
) {
    let rows: Vec<RehydrateMessage> = branch.iter().map(|id| row(id, None)).collect();
    match (branch_point(&base(), &rows, links), expected) {
        (Ok((n, line)), Ok((want, at))) => assert_eq!((n, line.as_str()), (want, at)),
        (Err(err), Err(why)) => assert!(err.to_string().contains(why), "{err}"),
        (got, want) => panic!("{got:?}, not {want:?}"),
    }
}

/// The rows of a branch past where it leaves a transcript that a switch writes: all but those the
/// lines it keeps hold already, each beside the tree hung from the row before it; and it resumes
/// from the last of them, else from where it leaves.
#[rstest]
fn a_switch_writes_the_rows_past_the_branch_point_the_copy_kept_none_of() {
    let branch = [
        row("a", None),
        row("b", Some("a")),
        row("syn-t", None),
        row("syn-mine", None),
        row("d", Some("b")),
        row("syn-u", None),
    ];
    let kept = LocalTip {
        known_source_ids: ["a", "b", "syn-mine"].map(str::to_owned).into(),
        ..base()
    };
    let written = past(&branch, 2, &kept, Links::Parents);
    let ids: Vec<(&str, Option<&str>)> =
        written.iter().map(|m| (m.source_id.as_str(), m.parent_source_id.as_deref())).collect();
    assert_eq!(ids, [("syn-t", Some("b")), ("d", Some("b")), ("syn-u", Some("d"))]);
    assert_eq!(switched_to(&written, "b"), "syn-u");
    assert_eq!(switched_to(&[], "b"), "b");
}

/// The transcript as it was is backed up before it is replaced: removed again only when the
/// replace did not happen, and kept, the switch made, when only syncing its directory failed
/// after the new file was in place.
#[rstest]
#[case::replaced(None, true, None)]
#[case::changed_meanwhile(Some(false), false, Some("Changed"))]
#[case::failed_before_the_rename(Some(true), false, Some("Io"))]
fn a_switch_keeps_its_backup_unless_nothing_was_replaced(
    #[case] fails: Option<bool>,
    #[case] kept: bool,
    #[case] refused: Option<&str>,
) {
    use crate::fs::ReplaceError;

    let dir = tempfile::tempdir().unwrap();
    let backups = dir.path().join("backups");
    let path = dir.path().join("s.jsonl");
    std::fs::write(&path, b"old\n").unwrap();
    let stamp = Stamp::of(b"old\n");
    let replaced = replace_jsonl_with(&path, stamp, b"old\n", b"new\n", &backups, |p, b, still| {
        assert!(still(b"old\n"));
        std::fs::write(p, b).unwrap();
        match fails {
            None => Err(ReplaceError::Unsynced(std::io::Error::other("injected"))),
            Some(false) => {
                std::fs::write(p, b"old\n").unwrap();
                Err(ReplaceError::NotReplaced(std::io::ErrorKind::Interrupted.into()))
            }
            Some(true) => {
                std::fs::write(p, b"old\n").unwrap();
                Err(ReplaceError::NotReplaced(std::io::Error::other("injected")))
            }
        }
    });
    let found: Vec<PathBuf> =
        std::fs::read_dir(&backups).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(!found.is_empty(), kept, "{found:?}");
    match refused {
        None => {
            let (backup, warning) = replaced.unwrap();
            assert_eq!(found, std::slice::from_ref(&backup));
            assert_eq!(std::fs::read(&backup).unwrap(), b"old\n");
            let warning = warning.expect("the failed sync is said");
            assert!(warning.contains("syncing its directory failed"), "{warning}");
        }
        Some(why) => {
            let err = replaced.unwrap_err();
            assert!(format!("{err:?}").starts_with(why), "{err:?}");
        }
    }
}
