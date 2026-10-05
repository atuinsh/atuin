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
