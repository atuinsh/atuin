use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::json;
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::session::{Content, Role};
use crate::harnesstools::sync::TableEntry;
use crate::harnesstools::sync::liveness::tests::{self as live, Fake, dir, procs, table};

fn header(version: u64) -> Value {
    json!({"type": "session", "version": version, "id": "5e55e55e-0000-4000-8000-000000000002",
           "timestamp": "2026-09-28T10:00:00.000Z", "cwd": "/work/proj"})
}

fn entry(id: &str, parent: Option<&str>, role: &str) -> Value {
    json!({"type": "message", "id": id, "parentId": parent, "timestamp": "2026-09-28T10:00:01.000Z",
           "message": {"role": role, "content": [{"type": "text", "text": id}], "timestamp": 1}})
}

/// A pi file of `version`: a prompt and its answer.
fn local(version: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for line in [
        header(version),
        entry("aaaa0001", None, "user"),
        entry("aaaa0002", Some("aaaa0001"), "assistant"),
    ] {
        out.extend(line.to_string().bytes());
        out.push(b'\n');
    }
    out
}

fn row(id: &str, parent: &str, role: Role) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: Some(parent.to_owned()),
        timestamp: OffsetDateTime::from_unix_timestamp(1_790_000_000).unwrap(),
        role,
        content: vec![Content::Text(format!("said {id}"))],
        model: Some("m".to_owned()),
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
    }
}

fn ahead() -> Vec<RehydrateMessage> {
    vec![row("bbbb0001", "aaaa0002", Role::User), row("bbbb0002", "bbbb0001", Role::Assistant)]
}

fn write(dir: &TempDir, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join("2026-09-28T10-00-00-000Z_5e55e55e.jsonl");
    std::fs::write(&path, bytes).unwrap();
    path
}

/// The copy works where its header says.
#[rstest]
fn the_tip_records_where_the_copy_works(dir: TempDir) {
    let path = write(&dir, &local(3));
    assert_eq!(File::read(&path).unwrap().tip(&path).cwd, Some(PathBuf::from("/work/proj")));
}

/// Appended entries hang from the last one, and pi continues from the last of them.
#[rstest]
fn a_fast_forward_resumes_from_the_last_entry(dir: TempDir) {
    let path = write(&dir, &local(3));
    let base = File::read(&path).unwrap().tip(&path);
    assert_eq!(base.tip_source_id.as_deref(), Some("aaaa0002"));

    let outcome = append_to(&base, &ahead(), None).unwrap();

    assert_eq!(outcome.appended, ["bbbb0001", "bbbb0002"]);
    assert_eq!(outcome.tip_source_id.as_deref(), Some("bbbb0002"));
    let after = File::read(&path).unwrap().tip(&path);
    assert_eq!(after.tip_source_id.as_deref(), Some("bbbb0002"));
    for id in ["aaaa0001", "aaaa0002", "bbbb0001", "bbbb0002"] {
        assert!(after.known_source_ids.contains(id), "{id} reads back");
    }
    let bytes = std::fs::read(&path).unwrap();
    let first: Value = serde_json::from_slice(jsonl_lines(&bytes).nth(3).unwrap()).unwrap();
    assert_eq!(first["parentId"], "aaaa0002");
}

#[rstest]
#[case::off_an_older_entry(3, vec![row("bbbb0001", "aaaa0001", Role::User)], "Unsupported")]
#[case::an_old_file(1, ahead(), "Unsupported")]
#[case::an_id_the_file_holds(3, vec![row("aaaa0001", "aaaa0002", Role::User)], "IdTaken")]
#[case::changed(3, ahead(), "Changed")]
fn what_is_no_clean_fast_forward_is_refused(
    dir: TempDir,
    #[case] version: u64,
    #[case] rows: Vec<RehydrateMessage>,
    #[case] expected: &str,
) {
    let path = write(&dir, &local(version));
    let base = File::read(&path).unwrap().tip(&path);
    if expected == "Changed" {
        std::fs::write(&path, local(2)).unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    let err = append_to(&base, &rows, None).unwrap_err();
    assert!(format!("{err:?}").starts_with(expected), "{err:?}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[rstest]
#[case::a_pi_running(b"pi\0".as_slice(), Liveness::Unknown)]
#[case::none(b"bash\0".as_slice(), Liveness::NotLive)]
fn any_pi_running_may_be_writing(dir: TempDir, #[case] cmdline: &[u8], #[case] expected: Liveness) {
    let procs = procs(dir.path(), &[Fake::running(9, cmdline, "1")]);
    assert_eq!(liveness(&procs, &Seen::default()), expected);
}

/// pi hides its command line behind its name: which session a pi works on is told only by where
/// it works.
#[rstest]
#[case::in_the_sessions_directory("proj", Liveness::Unknown)]
#[case::elsewhere("other", Liveness::NotLive)]
fn a_pi_is_told_apart_by_where_it_works(
    dir: TempDir,
    #[case] works_in: &str,
    #[case] expected: Liveness,
) {
    let procs = table(&[TableEntry {
        cwd: Some(dir.path().join(works_in)),
        ..live::entry(9, "pi", &["pi"], 1)
    }]);
    assert_eq!(liveness(&procs, &live::cwd(Some(dir.path().join("proj")))), expected);
}

/// A pi working elsewhere may have the session open from there (`pi --session <file>`): it counts
/// when its command line names the session's file, or while the file was modified lately.
#[rstest]
#[case::changed_lately_named_nowhere(10, false, Liveness::Unknown)]
#[case::changed_long_ago_named_nowhere(3600, false, Liveness::NotLive)]
#[case::changed_long_ago_named(3600, true, Liveness::Unknown)]
fn a_pi_elsewhere_counts_when_named_or_writing(
    dir: TempDir,
    #[case] ago: u64,
    #[case] named: bool,
    #[case] expected: Liveness,
) {
    let path = write(&dir, &local(2));
    let at = std::time::SystemTime::now() - std::time::Duration::from_secs(ago);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
    let mut base = File::read(&path).unwrap().tip(&path);
    assert_eq!(base.modified, Some(at));
    // Where the header records it working, as a directory still there.
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    base.cwd = Some(proj.clone());
    let file = path.display().to_string();
    let cmd: &[&str] = if named {
        &["pi", "--session", &file]
    } else {
        &["pi"]
    };
    let procs = table(&[TableEntry {
        cwd: Some(dir.path().join("other")),
        ..live::entry(9, "pi", cmd, 1)
    }]);
    let seen = appending(&base, "5e55e55e-0000-4000-8000-000000000002", Some(&proj));
    assert_eq!(liveness(&procs, &seen), expected);
}
