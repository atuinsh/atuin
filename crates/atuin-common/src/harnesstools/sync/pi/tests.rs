use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rstest::{fixture, rstest};
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::rehydrate::testing;
use crate::harnesstools::session::{Content, Role};
use crate::harnesstools::sync::ProcessInfo;

const MOCK: &str = include_str!("../../../../tests/fixtures/pi/session-mock-0.85.jsonl");
const SESSION: &str = "01a0d14e-b611-7276-866f-7c5995c1e8fd";

#[fixture]
fn dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

/// A captured row for each line of `jsonl` capture keys on an id, as capture syncs them.
fn captured(jsonl: &str) -> Vec<RehydrateMessage> {
    let rows = jsonl
        .lines()
        .filter_map(|l| crate::json::js::from_slice::<PiMessage>(l.as_bytes()).ok())
        .filter_map(|m| {
            Some(RehydrateMessage {
                source_id: m.id()?.to_string(),
                parent_source_id: m.parent_id().map(|p| p.to_string()),
                timestamp: m.timestamp().unwrap_or(OffsetDateTime::UNIX_EPOCH),
                role: m.role(),
                content: m.content(),
                model: m.model(),
                usage: m.usage(),
                stop_reason: m.stop_reason(),
                turn_id: m.turn_id(),
                cwd: m.cwd(),
                git_branch: m.git_branch(),
            })
        })
        .collect();
    testing::synced(rows)
}

fn read_back(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|l| crate::json::js::from_slice::<PiMessage>(l.as_bytes()).ok()?.id())
        .map(String::from)
        .collect()
}

fn write(dir: &TempDir, text: &str) -> PathBuf {
    let path = dir.path().join(format!("2026-09-28T10-00-00-000Z_{SESSION}.jsonl"));
    std::fs::write(&path, text).unwrap();
    path
}

fn tip_of(path: &Path) -> LocalTip {
    File::read(path).unwrap().tip(path)
}

fn lines_of(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[rstest]
fn the_tip_is_the_last_entry(dir: TempDir) {
    let path = write(&dir, MOCK);
    let tip = tip_of(&path);
    assert_eq!(tip.tip_source_id.as_deref(), Some("e28a7d96"));
    assert_eq!(tip.known_source_ids, read_back(&path).into_iter().collect());
    assert!(tip.known_source_ids.contains(SESSION), "the header is a row of its own");
}

#[rstest]
fn a_label_last_is_the_tip(dir: TempDir) {
    let cut: Vec<&str> = MOCK.lines().take(33).collect();
    let path = write(&dir, &(cut.join("\n") + "\n"));
    assert_eq!(tip_of(&path).tip_source_id.as_deref(), Some("d45e83d7"));
}

/// A file cut short on this machine and gone on elsewhere, down whichever branch: appending the
/// synced rows it lacks leaves pi resuming from the last one, and capture finds nothing new.
#[rstest]
fn catching_up_a_file_pushes_nothing_new(
    dir: TempDir,
    #[values(4, 9, 16, 26, 30, 33, 37)] keep: usize,
) {
    let local: Vec<&str> = MOCK.lines().take(keep).collect();
    let path = write(&dir, &(local.join("\n") + "\n"));
    let base = tip_of(&path);
    let synced = captured(MOCK);
    let missing: Vec<RehydrateMessage> =
        synced.iter().filter(|m| !base.known_source_ids.contains(&m.source_id)).cloned().collect();

    let outcome = append_to(&base, &missing, &AppendOptions::default()).unwrap();

    testing::assert_nothing_new(&synced, read_back(&path).iter().map(String::as_str));
    assert!(!outcome.marked_tip);
    let after = tip_of(&path);
    assert_eq!(after.tip_source_id, outcome.tip_source_id);
    assert_eq!(after.tip_source_id.as_ref(), outcome.appended.last());
    // Each written entry hangs from an entry before it.
    let lines = lines_of(&path);
    let mut seen = HashSet::new();
    for line in &lines[1..] {
        if let Some(parent) = line["parentId"].as_str() {
            assert!(seen.contains(parent), "{line}");
        }
        seen.insert(line["id"].as_str().unwrap().to_owned());
    }
}

/// A compaction appended keeps from the first entry since the compaction before it on its
/// path.
#[rstest]
fn a_compaction_appended_keeps_what_its_path_kept(dir: TempDir) {
    let local: Vec<&str> = MOCK.lines().take(28).collect();
    let path = write(&dir, &(local.join("\n") + "\n"));
    let base = tip_of(&path);
    let summary = RehydrateMessage {
        source_id: "c0ffee00".to_owned(),
        parent_source_id: Some("a118cfd8".to_owned()),
        timestamp: OffsetDateTime::now_utc(),
        role: Role::System,
        content: vec![Content::Summary("what happened".to_owned())],
        model: None,
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
    };
    append_to(&base, &[summary], &AppendOptions::default()).unwrap();
    let last = lines_of(&path).pop().unwrap();
    assert_eq!(last["type"], "compaction");
    // No compaction before it on its path: it keeps from the first entry.
    assert_eq!(last["firstKeptEntryId"], "9d7931ea");
    assert_eq!(last["parentId"], "a118cfd8");
}

/// A branch this machine already holds is made the tip with a label under its leaf, which keeps
/// the label the leaf had.
#[rstest]
#[case::unlabelled("cd8b6bf1", None)]
#[case::labelled("b46d4a90", Some("bookmark-1"))]
fn a_branch_already_here_is_made_the_tip(
    dir: TempDir,
    #[case] head: &str,
    #[case] label: Option<&str>,
) {
    let path = write(&dir, MOCK);
    let base = tip_of(&path);
    let options = AppendOptions {
        make_tip: true,
        head: Some(head),
        ..AppendOptions::default()
    };
    let outcome = append_to(&base, &[], &options).unwrap();
    assert!(outcome.marked_tip);
    let last = lines_of(&path).pop().unwrap();
    assert_eq!(last["type"], "label");
    assert_eq!(last["parentId"], head);
    assert_eq!(last["targetId"], head);
    assert_eq!(last["label"].as_str(), label);
    let id = last["id"].as_str().unwrap();
    assert_eq!(id.len(), 8);
    assert!(!base.known_source_ids.contains(id));
    assert_eq!(tip_of(&path).tip_source_id.as_deref(), Some(id));
    assert_eq!(outcome.tip_source_id.as_deref(), Some(id));
}

#[rstest]
fn the_tip_already_last_needs_no_label(dir: TempDir) {
    let path = write(&dir, MOCK);
    let base = tip_of(&path);
    let options = AppendOptions {
        make_tip: true,
        head: Some("e28a7d96"),
        ..AppendOptions::default()
    };
    let outcome = append_to(&base, &[], &options).unwrap();
    assert!(!outcome.marked_tip);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), MOCK);
}

fn user_row(id: &str, parent: &str) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: Some(parent.to_owned()),
        timestamp: OffsetDateTime::now_utc(),
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
#[case::in_the_file(user_row("cd8b6bf1", "e28a7d96"), None, "IdTaken")]
#[case::taken_elsewhere(user_row("0badf00d", "e28a7d96"), Some("0badf00d"), "IdTaken")]
#[case::hangs_from_nothing_here(user_row("0badf00d", "nowhere0"), None, "Disconnected")]
fn a_segment_that_does_not_fit_is_refused(
    dir: TempDir,
    #[case] row: RehydrateMessage,
    #[case] taken: Option<&str>,
    #[case] expected: &str,
) {
    let path = write(&dir, MOCK);
    let base = tip_of(&path);
    let taken: HashSet<String> = taken.into_iter().map(str::to_owned).collect();
    let options = AppendOptions {
        taken_ids: Some(&taken),
        ..AppendOptions::default()
    };
    let err = append_to(&base, &[row], &options).unwrap_err();
    assert!(format!("{err:?}").starts_with(expected), "{err:?}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), MOCK);
}

#[rstest]
fn a_file_changed_since_it_was_read_is_left_alone(dir: TempDir) {
    let path = write(&dir, MOCK);
    let base = tip_of(&path);
    std::fs::write(&path, format!("{MOCK}\n")).unwrap();
    let err = append_to(&base, &[user_row("0badf00d", "e28a7d96")], &AppendOptions::default())
        .unwrap_err();
    assert!(matches!(err, SyncError::Changed), "{err:?}");
}

#[rstest]
fn a_version_1_file_is_not_appended_to(dir: TempDir) {
    let path = write(&dir, include_str!("../../../../tests/fixtures/pi/session-v1.jsonl"));
    let base = tip_of(&path);
    let err =
        append_to(&base, &[user_row("0badf00d", "x")], &AppendOptions::default()).unwrap_err();
    assert!(matches!(err, SyncError::Unsupported(_)), "{err:?}");
}

fn process(argv: &[&str], cwd: &Path) -> ProcessInfo {
    ProcessInfo {
        pid: 77,
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        name: argv.first().map(|a| (*a).to_owned()),
        cwd: Some(cwd.to_owned()),
    }
}

/// A `/proc` of its own holding `processes`.
fn procs(dir: &TempDir, processes: &[ProcessInfo]) -> Processes {
    let root = dir.path().join("proc");
    std::fs::create_dir_all(&root).unwrap();
    for p in processes {
        let at = root.join(p.pid.to_string());
        std::fs::create_dir_all(&at).unwrap();
        let mut cmdline = p.argv.join("\0").into_bytes();
        cmdline.push(0);
        std::fs::write(at.join("cmdline"), cmdline).unwrap();
        std::fs::write(at.join("comm"), format!("{}\n", p.name.clone().unwrap_or_default()))
            .unwrap();
        std::fs::write(at.join("stat"), format!("{} (x) S 1", p.pid)).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(p.cwd.as_ref().unwrap(), at.join("cwd")).unwrap();
    }
    Processes::ProcFs(root)
}

#[rstest]
#[case::pi_in_its_directory(&["pi"], true, Liveness::Unknown)]
#[case::pi_elsewhere(&["pi"], false, Liveness::NotLive)]
#[case::pi_naming_it(&["node", "/usr/bin/pi", "--session", SESSION], false, Liveness::Live { pid: Some(77) })]
#[case::pi_naming_its_file(
    &["node", "/usr/lib/pi-coding-agent/dist/cli.js", "--session", "/x/2026_01a0d14e-b611-7276-866f-7c5995c1e8fd.jsonl"],
    true,
    Liveness::Live { pid: Some(77) },
)]
#[case::pi_naming_another(&["pi", "--session", "0199ffff"], true, Liveness::NotLive)]
#[case::pi_forking_it(&["pi", "--fork", SESSION], true, Liveness::NotLive)]
#[case::pi_without_sessions(&["pi", "--no-session"], true, Liveness::NotLive)]
#[case::something_else(&["vim"], true, Liveness::NotLive)]
fn a_pi_process_is_looked_for(
    dir: TempDir,
    #[case] argv: &[&str],
    #[case] same_dir: bool,
    #[case] expected: Liveness,
) {
    let work = dir.path().join("work");
    let other = dir.path().join("other");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let cwd = if same_dir {
        &work
    } else {
        &other
    };
    let procs = procs(&dir, &[process(argv, cwd)]);
    assert_eq!(liveness(&procs, SESSION, Some(&work), None), expected);
}
