use pretty_assertions::assert_eq;
use rstest::rstest;
use serde_json::json;
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::rehydrate::testing;
use crate::harnesstools::session::{Content, Role};
use crate::harnesstools::sync::liveness::tests::{Fake, dir, entry, procs, table};

const SESSION: &str = "5e55e55e-0000-4000-8000-000000000001";

/// A transcript line: `kind` `uuid` hanging from `parent`, written at second `at`.
fn line(kind: &str, uuid: &str, parent: Option<&str>, at: u32) -> Value {
    let mut line = json!({
        "parentUuid": parent,
        "isSidechain": false,
        "type": kind,
        "uuid": uuid,
        "timestamp": format!("2026-09-28T10:00:{at:02}.000Z"),
        "cwd": "/work/proj",
        "sessionId": SESSION,
    });
    match kind {
        "user" => line["message"] = json!({"role": "user", "content": format!("prompt {uuid}")}),
        "assistant" => {
            line["message"] = json!({
                "id": format!("msg_{uuid}"),
                "type": "message",
                "role": "assistant",
                "model": "claude-x",
                "content": [{"type": "text", "text": format!("reply {uuid}")}],
                "stop_reason": "end_turn",
            });
        }
        _ => {}
    }
    line
}

fn user(uuid: &str, parent: Option<&str>, at: u32) -> Value {
    line("user", uuid, parent, at)
}

fn assistant(uuid: &str, parent: &str, at: u32) -> Value {
    line("assistant", uuid, Some(parent), at)
}

fn last_prompt(leaf: Option<&str>, explicit: bool) -> Value {
    let mut line = json!({"type": "last-prompt", "leafUuid": leaf, "sessionId": SESSION});
    if explicit {
        line["explicit"] = json!(true);
    }
    line
}

fn jsonl(lines: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in lines {
        out.extend(line.to_string().bytes());
        out.push(b'\n');
    }
    out
}

/// The chain u1 → a1 → u2 → a2.
fn chain() -> Vec<Value> {
    vec![
        user("u1", None, 1),
        assistant("a1", "u1", 2),
        user("u2", Some("a1"), 3),
        assistant("a2", "u2", 4),
    ]
}

fn with(mut lines: Vec<Value>, more: impl IntoIterator<Item = Value>) -> Vec<Value> {
    lines.extend(more);
    lines
}

#[rstest]
#[case::linear(chain(), Some("a2"))]
#[case::the_lines_hanging_from_the_leaf_follow_it(
    with(chain(), [line("system", "s1", Some("a2"), 5), line("attachment", "t1", Some("s1"), 6)]),
    Some("t1"),
)]
#[case::in_timestamp_order(
    with(chain(), [line("attachment", "t2", Some("a2"), 7), line("attachment", "t1", Some("a2"), 6)]),
    Some("t2"),
)]
#[case::a_leaf_the_last_line_descends_from(with(chain(), [last_prompt(Some("u2"), false)]), Some("a2"))]
#[case::rewound(with(chain(), [last_prompt(Some("a1"), true)]), Some("a1"))]
#[case::a_leaf_on_another_branch(
    with(chain(), [user("u3", Some("a1"), 5), assistant("a3", "u3", 6), last_prompt(Some("a2"), false)]),
    Some("a2"),
)]
#[case::the_last_line_on_another_branch_without_a_leaf(
    with(chain(), [user("u3", Some("a1"), 5), assistant("a3", "u3", 6)]),
    Some("a3"),
)]
#[case::cleared(with(chain(), [last_prompt(None, true)]), None)]
#[case::a_leaf_that_is_not_there(with(chain(), [last_prompt(Some("gone"), true)]), Some("a2"))]
#[case::compaction_forgets_the_leaf_before_it(
    with(chain(), [
        last_prompt(Some("a1"), true),
        {
            let mut boundary = line("system", "b1", None, 5);
            boundary["subtype"] = json!("compact_boundary");
            boundary
        },
        user("u3", Some("b1"), 7),
        assistant("a3", "u3", 8),
    ]),
    Some("a3"),
)]
#[case::progress_lines_are_skipped_over(
    with(chain(), [
        json!({"type": "progress", "uuid": "p1", "parentUuid": "a2", "sessionId": SESSION}),
        user("u3", Some("p1"), 5),
        last_prompt(Some("a2"), false),
    ]),
    Some("u3"),
)]
#[case::sidechains_are_not_the_main_chain(
    with(chain(), [{
        let mut side = user("side", Some("a2"), 5);
        side["isSidechain"] = json!(true);
        side
    }]),
    Some("a2"),
)]
fn the_tip_is_where_claude_code_resumes(#[case] lines: Vec<Value>, #[case] expected: Option<&str>) {
    assert_eq!(tip(&jsonl(&lines)).as_deref(), expected);
}

/// A synced row, `role` `id` hanging from `parent`, written at second `at`.
fn row(id: &str, parent: &str, role: Role, at: i64) -> RehydrateMessage {
    let content = match role {
        Role::User | Role::Assistant => vec![Content::Text(format!("said {id}"))],
        _ => Vec::new(),
    };
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: Some(parent.to_owned()),
        timestamp: OffsetDateTime::from_unix_timestamp(1_790_000_000 + at).unwrap(),
        role,
        content,
        model: Some("claude-x".to_owned()),
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: Some(PathBuf::from("/Users/someone/proj")),
        git_branch: None,
    }
}

/// The ids capture reads from the transcript at `path`, in order.
fn read_back(path: &Path) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap();
    jsonl_lines(&bytes)
        .filter_map(|l| CcodeMessage::decode(l).ok()?.id().map(String::from))
        .collect()
}

fn write(dir: &TempDir, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join(format!("{SESSION}.jsonl"));
    std::fs::write(&path, bytes).unwrap();
    path
}

/// The rows another machine went on with from a2: appended, they hang from it, and Claude Code
/// resumes from the last of them; capture reads back nothing it did not sync. A torn last line
/// stays torn.
/// The copy works where the last of its lines that says records: Claude Code resumes it there.
#[rstest]
fn the_tip_records_where_the_copy_works(dir: TempDir) {
    let mut moved = user("u3", Some("a2"), 5);
    moved["cwd"] = json!("/work/moved");
    let path = write(&dir, &jsonl(&with(chain(), [moved, last_prompt(Some("u3"), false)])));
    assert_eq!(read_tip(&path).unwrap().cwd, Some(PathBuf::from("/work/moved")));
}

#[rstest]
#[case::linear(jsonl(&chain()))]
#[case::rewound_here_to_the_tip(jsonl(&with(chain(), [user("u3", Some("a2"), 5), last_prompt(Some("a2"), true)])))]
#[case::after_a_torn_line({
    let mut bytes = jsonl(&chain());
    bytes.extend_from_slice(br#"{"type":"user","uuid":"torn"#);
    bytes
})]
fn a_fast_forward_resumes_from_the_last_row(dir: TempDir, #[case] local: Vec<u8>) {
    let path = write(&dir, &local);
    let base = read_tip(&path).unwrap();
    assert_eq!(base.tip_source_id.as_deref(), Some("a2"));
    let rows = vec![row("u4", "a2", Role::User, 10), row("a4", "u4", Role::Assistant, 11)];

    let outcome = append_to(SESSION, &base, &rows, None).unwrap();

    assert_eq!(outcome.appended, ["u4", "a4"]);
    assert_eq!(outcome.tip_source_id.as_deref(), Some("a4"));
    let after = read_tip(&path).unwrap();
    assert_eq!(after.tip_source_id.as_deref(), Some("a4"));
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(&local));
    let written: Vec<Value> = jsonl_lines(&bytes[local.len()..]).filter_map(parse).collect();
    assert_eq!(written[0]["parentUuid"], "a2");
    assert_eq!(written[0]["sessionId"], SESSION);
    // Written where this copy works, not where the other machine did, in this platform's form,
    // as Claude Code writes it.
    let here: PathBuf = Path::new("/work/proj").components().collect();
    assert_eq!(written[0]["cwd"], here.to_string_lossy().as_ref());
    assert_eq!(written[1]["parentUuid"], "u4");
    let mut synced: Vec<RehydrateMessage> =
        base.known_source_ids.iter().map(|id| row(id, "", Role::User, 0)).collect();
    synced.extend(rows);
    testing::assert_nothing_new(&synced, read_back(&path).iter().map(String::as_str));
}

#[rstest]
#[case::off_an_older_line(chain(), vec![row("u4", "a1", Role::User, 10)], "Unsupported")]
#[case::a_root(chain(), vec![RehydrateMessage { parent_source_id: None, ..row("u4", "", Role::User, 10) }], "Unsupported")]
#[case::a_cleared_transcript(with(chain(), [last_prompt(None, true)]), vec![row("u4", "a2", Role::User, 10)], "Unsupported")]
#[case::already_here(chain(), vec![row("a1", "a2", Role::Assistant, 10)], "IdTaken")]
// A line Claude Code loads but capture can't read took the id: the loader keeps that line.
#[case::the_last_row_would_not_be_resumed(
    with(chain(), [json!({"type": "user", "uuid": "a4", "parentUuid": "a2", "isSidechain": true, "timestamp": 12})]),
    vec![row("u4", "a2", Role::User, 10), row("a4", "u4", Role::Assistant, 11)],
    "Unsupported",
)]
fn what_is_no_clean_fast_forward_is_refused(
    dir: TempDir,
    #[case] local: Vec<Value>,
    #[case] rows: Vec<RehydrateMessage>,
    #[case] expected: &str,
) {
    let path = write(&dir, &jsonl(&local));
    let base = read_tip(&path).unwrap();
    let err = append_to(SESSION, &base, &rows, None).unwrap_err();
    assert!(format!("{err:?}").starts_with(expected), "{err:?}");
    assert_eq!(std::fs::read(&path).unwrap(), jsonl(&local));
}

#[rstest]
fn a_transcript_changed_since_it_was_read_is_left_alone(dir: TempDir) {
    let path = write(&dir, &jsonl(&chain()));
    let base = read_tip(&path).unwrap();
    let changed = jsonl(&with(chain(), [user("u9", Some("a2"), 9)]));
    std::fs::write(&path, &changed).unwrap();
    let err = append_to(SESSION, &base, &[row("u4", "a2", Role::User, 10)], None).unwrap_err();
    assert!(matches!(err, SyncError::Changed), "{err:?}");
    assert_eq!(std::fs::read(&path).unwrap(), changed);
}

const DOMAIN: &str = "linux:machine:pid:[1]";

fn record(session: &str, start: &str) -> Value {
    // The shape Claude Code 2.1.284 writes.
    json!({
        "pid": 41, "sessionId": session, "cwd": "/work/proj", "startedAt": 1_790_636_694_148_u64,
        "procStart": start, "version": "2.1.284", "kind": "interactive", "entrypoint": "cli",
        "pidDomain": DOMAIN, "status": "busy",
    })
}

#[rstest]
#[case::running(record(SESSION, "900").to_string(), Some("900"), Liveness::Live { pid: Some(41) })]
#[case::another_session(record("other", "900").to_string(), Some("900"), Liveness::NotLive)]
#[case::crashed(record(SESSION, "900").to_string(), None, Liveness::NotLive)]
#[case::pid_reused(record(SESSION, "900").to_string(), Some("901"), Liveness::NotLive)]
#[case::another_pid_namespace(
    { let mut r = record(SESSION, "900"); r["pidDomain"] = json!("linux:machine:pid:[2]"); r.to_string() },
    Some("900"),
    Liveness::Unknown,
)]
#[case::unreadable_while_running("{\"pid\":".to_owned(), Some("1"), Liveness::Unknown)]
#[case::unreadable_and_gone("{\"pid\":".to_owned(), None, Liveness::NotLive)]
fn a_registered_session_is_live_while_its_process_runs(
    dir: TempDir,
    #[case] record: String,
    #[case] running: Option<&str>,
    #[case] expected: Liveness,
) {
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("41.json"), record).unwrap();
    let running: Vec<Fake<'_>> =
        running.map(|start| Fake::running(41, b"claude\0", start)).into_iter().collect();
    let procs = procs(dir.path(), &running);
    assert_eq!(liveness(&sessions, SESSION, &procs, DOMAIN), expected);
    assert_eq!(liveness(&dir.path().join("none"), SESSION, &procs, DOMAIN), Liveness::NotLive);
}

/// A registered process that is there but whose `stat` can't be read may have the session open.
#[rstest]
#[case::its_record_readable(record(SESSION, "900").to_string())]
#[case::its_record_unreadable("{\"pid\":".to_owned())]
fn a_registered_process_that_cant_be_read_is_unknown(dir: TempDir, #[case] record: String) {
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("41.json"), record).unwrap();
    let procs = procs(dir.path(), &[Fake {
        unreadable: &["stat"],
        ..Fake::running(41, b"claude\0", "900")
    }]);
    assert_eq!(liveness(&sessions, SESSION, &procs, DOMAIN), Liveness::Unknown);
}

/// What Claude Code registers off Linux, for a process that started at 2026-10-05T12:34:56Z:
/// on macOS `ps -o lstart` (a `start` with spaces), on Windows `procStartFt` (a `FILETIME`).
fn record_elsewhere(start: &str, domain: &str) -> Value {
    let mut r = record(SESSION, start);
    r["pidDomain"] = json!(domain);
    if !start.contains(' ') {
        r.as_object_mut().unwrap().remove("procStart");
        r["procStartFt"] = json!(start);
    }
    r
}

const LSTART: &str = "Mon Oct  5 12:34:56 2026";
const FILETIME: &str = "134356772961234567";
const STARTED: u64 = 1_791_203_696;

/// Off Linux, the start time Claude Code registers is compared with the process table's, to
/// the second; one that can't be compared is not, and the pid's process is taken for it.
#[rstest]
#[case::macos_running(LSTART, Some(STARTED), Liveness::Live { pid: Some(41) })]
#[case::macos_pid_reused(LSTART, Some(STARTED + 1), Liveness::NotLive)]
#[case::macos_crashed(LSTART, None, Liveness::NotLive)]
#[case::macos_no_start_read(LSTART, Some(0), Liveness::Live { pid: Some(41) })]
#[case::macos_unparsed("Mon 5 Oct 2026, 12:34:56 PM", Some(STARTED + 1), Liveness::Live { pid: Some(41) })]
#[case::windows_running(FILETIME, Some(STARTED), Liveness::Live { pid: Some(41) })]
#[case::windows_pid_reused(FILETIME, Some(STARTED - 1), Liveness::NotLive)]
#[case::windows_unparsed("soon", Some(STARTED + 1), Liveness::Live { pid: Some(41) })]
fn a_registered_session_is_checked_against_the_process_table(
    dir: TempDir,
    #[case] start: &str,
    #[case] running: Option<u64>,
    #[case] expected: Liveness,
) {
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let record = record_elsewhere(start, "darwin");
    std::fs::write(sessions.join("41.json"), record.to_string()).unwrap();
    let mut entries = vec![entry(7, "zsh", &["-zsh"], 1)];
    entries.extend(running.map(|at| entry(41, "claude", &["claude"], at)));
    assert_eq!(liveness(&sessions, SESSION, &table(&entries), "macos"), expected);
}

/// Off Linux, a domain names only the platform: only a Linux pid namespace's is another's.
#[rstest]
#[case::the_platform("darwin", Liveness::Live { pid: Some(41) })]
#[case::windows_and_its_host("win32:host", Liveness::Live { pid: Some(41) })]
#[case::a_linux_namespace("linux:machine:pid:[1]", Liveness::Unknown)]
fn a_domain_off_linux_is_this_machines_unless_a_linux_namespaces(
    dir: TempDir,
    #[case] domain: &str,
    #[case] expected: Liveness,
) {
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let record = record_elsewhere(LSTART, domain);
    std::fs::write(sessions.join("41.json"), record.to_string()).unwrap();
    let procs = table(&[entry(41, "claude", &["claude"], STARTED)]);
    assert_eq!(liveness(&sessions, SESSION, &procs, "macos"), expected);
}
