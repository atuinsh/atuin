use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rstest::{fixture, rstest};
use serde_json::json;
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::rehydrate::testing;
use crate::harnesstools::session::Role;

const SESSION: &str = "5e55e55e-0000-4000-8000-000000000001";

#[fixture]
fn dir() -> TempDir {
    tempfile::tempdir().unwrap()
}

/// A transcript line: `kind` `uuid` hanging from `parent`, written at second `at`.
fn line(kind: &str, uuid: &str, parent: Option<&str>, at: u32) -> Value {
    let message = match kind {
        "user" => json!({"role": "user", "content": format!("prompt {uuid}")}),
        "assistant" => json!({
            "id": format!("msg_{uuid}"),
            "type": "message",
            "role": "assistant",
            "model": "claude-x",
            "content": [{"type": "text", "text": format!("reply {uuid}")}],
            "stop_reason": "end_turn",
        }),
        _ => Value::Null,
    };
    let mut line = json!({
        "parentUuid": parent,
        "isSidechain": false,
        "type": kind,
        "uuid": uuid,
        "timestamp": format!("2026-09-28T10:00:{at:02}.000Z"),
        "cwd": "/work/proj",
        "sessionId": SESSION,
    });
    if !message.is_null() {
        line["message"] = message;
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
#[case::walks_up_to_the_conversation_and_back(
    with(chain(), [
        line("system", "s1", Some("a2"), 5),
        line("attachment", "t1", Some("s1"), 6),
        last_prompt(Some("s1"), true),
    ]),
    Some("t1"),
)]
#[case::a_leaf_the_last_line_descends_from(with(chain(), [last_prompt(Some("u2"), false)]), Some("a2"))]
#[case::rewound(with(chain(), [last_prompt(Some("a1"), true)]), Some("a1"))]
#[case::rewound_then_continued(
    with(chain(), [
        last_prompt(Some("a1"), true),
        user("u3", Some("a1"), 5),
        assistant("a3", "u3", 6),
        last_prompt(Some("a3"), false),
    ]),
    Some("a3"),
)]
#[case::a_leaf_on_another_branch(
    with(chain(), [user("u3", Some("a1"), 5), assistant("a3", "u3", 6), last_prompt(Some("a2"), false)]),
    Some("a2"),
)]
#[case::the_last_line_on_another_branch_without_a_leaf(
    with(chain(), [user("u3", Some("a1"), 5), assistant("a3", "u3", 6)]),
    Some("a3"),
)]
#[case::explicit_until_a_line_follows(
    with(chain(), [last_prompt(Some("a1"), true), last_prompt(Some("a1"), false)]),
    Some("a1"),
)]
#[case::cleared(with(chain(), [last_prompt(None, true)]), None)]
#[case::a_null_leaf_that_is_not_explicit(with(chain(), [last_prompt(None, false)]), Some("a2"))]
#[case::a_leaf_that_is_not_there(with(chain(), [last_prompt(Some("gone"), true)]), Some("a2"))]
#[case::compaction_forgets_the_leaf_before_it(
    with(chain(), [
        last_prompt(Some("a1"), true),
        {
            let mut boundary = line("system", "b1", None, 5);
            boundary["subtype"] = json!("compact_boundary");
            boundary["logicalParentUuid"] = json!("a2");
            boundary
        },
        {
            let mut summary = user("sum", Some("b1"), 6);
            summary["isCompactSummary"] = json!(true);
            summary
        },
        user("u3", Some("sum"), 7),
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
#[case::without_a_leaf_the_newest_line_below_the_last(
    vec![user("u1", None, 1), assistant("a1", "u1", 2), assistant("a2", "u2", 9), user("u2", Some("a1"), 5)],
    Some("a2"),
)]
fn the_tip_is_where_claude_code_resumes(#[case] lines: Vec<Value>, #[case] expected: Option<&str>) {
    assert_eq!(tip(&jsonl(&lines)).as_deref(), expected);
}

/// A captured row for each line of `jsonl` with an id, as capture syncs them.
fn captured(jsonl: &[u8]) -> Vec<RehydrateMessage> {
    let rows = jsonl_lines(jsonl)
        .filter_map(|l| CcodeMessage::decode(l).ok())
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
                seq: None,
            })
        })
        .collect();
    testing::synced(rows)
}

/// The ids capture reads from the transcript at `path`, in order.
fn read_back(path: &Path) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap();
    jsonl_lines(&bytes)
        .filter_map(|l| CcodeMessage::decode(l).ok()?.id().map(String::from))
        .collect()
}

fn write(dir: &TempDir, id: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join(format!("{id}.jsonl"));
    std::fs::write(&path, bytes).unwrap();
    path
}

fn session_of(jsonl: &str) -> String {
    jsonl
        .lines()
        .find_map(|l| {
            serde_json::from_str::<Value>(l).ok()?["sessionId"].as_str().map(str::to_owned)
        })
        .unwrap()
}

/// A transcript cut short on this machine and gone on elsewhere: appending the synced rows it
/// lacks leaves it resuming from the last of them, and capture finds nothing new in it.
#[rstest]
#[case::session1(include_str!("../../../../tests/fixtures/ccode/session1.jsonl"))]
#[case::session2(include_str!("../../../../tests/fixtures/ccode/session2.jsonl"))]
#[case::session3(include_str!("../../../../tests/fixtures/ccode/session3.jsonl"))]
#[case::compacted(include_str!("../../../../tests/fixtures/ccode/session4.jsonl"))]
fn catching_up_a_transcript_pushes_nothing_new(
    dir: TempDir,
    #[case] fixture: &str,
    #[values(0.3, 0.6, 0.9)] cut: f64,
) {
    let id = session_of(fixture);
    let all: Vec<&str> = fixture.lines().collect();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    let keep = (all.len() as f64 * cut) as usize;
    let local = all[..keep].join("\n") + "\n";
    let path = write(&dir, &id, local.as_bytes());
    let base = read_tip(&path).unwrap();
    assert_eq!(base.known_source_ids, read_back(&path).into_iter().collect());

    let synced = captured(fixture.as_bytes());
    let missing: Vec<RehydrateMessage> =
        synced.iter().filter(|m| !base.known_source_ids.contains(&m.source_id)).cloned().collect();
    let outcome = append_to(&id, &base, &missing, &AppendOptions::default()).unwrap();

    testing::assert_nothing_new(&synced, read_back(&path).iter().map(String::as_str));
    let after = read_tip(&path).unwrap();
    assert_eq!(after.tip_source_id, outcome.tip_source_id);
    // It resumes from the last conversation line written.
    let bytes = std::fs::read(&path).unwrap();
    let last_conversation = jsonl_lines(&bytes)
        .rev()
        .filter_map(parse)
        .filter(|l| matches!(l["type"].as_str(), Some("user" | "assistant")))
        .find_map(|l| l["uuid"].as_str().map(str::to_owned));
    assert_eq!(after.tip_source_id, last_conversation);
    // Every line names the session its file is named after.
    for line in jsonl_lines(&bytes).filter_map(parse) {
        if let Some(session) = line["sessionId"].as_str() {
            assert_eq!(session, id);
        }
    }
}

/// A transcript restored from sync ending on calls folded into notes (merged into the line
/// before them, with the results between): catching it up appends none of them again, and a
/// branch hanging from one of them (Claude Code carried on elsewhere after it) hangs from the line
/// it went into; the transcript then holds every synced row once.
#[rstest]
#[case::session3(include_str!("../../../../tests/fixtures/ccode/session3.jsonl"))]
#[case::compacted(include_str!("../../../../tests/fixtures/ccode/session4.jsonl"))]
fn catching_up_a_flattened_restore_writes_nothing_twice(dir: TempDir, #[case] fixture: &str) {
    use crate::harnesstools::rehydrate::{Flatten, RehydrateSession};

    let id = session_of(fixture);
    let synced = captured(fixture.as_bytes());
    let cut = testing::after_merged(&synced, &Flatten::Runs).expect("calls folded into notes");
    let restored = RehydrateSession {
        id: id.clone(),
        title: None,
        cwd: dir.path().to_path_buf(),
        original_cwd: None,
        git_branch: None,
        model: None,
        started_at: OffsetDateTime::UNIX_EPOCH,
        messages: synced[..cut].to_vec(),
    };
    let path = rehydrate::rehydrate_into(&dir.path().join("projects"), &restored).unwrap();
    let base = read_tip(&path).unwrap();
    let merged_away = &synced[cut - 1].source_id;
    assert!(base.merged.contains_key(merged_away), "{:?}", base.merged);
    let into = base.merged[merged_away].clone();

    // Fast-forward.
    let missing = testing::missing(&synced, &base);
    assert_eq!(missing[0].source_id, synced[cut].source_id);
    append_to(&id, &base, &missing, &AppendOptions::default()).unwrap();
    let after = read_tip(&path).unwrap();
    testing::assert_caught_up(&synced, &after, &read_back(&path));

    // A branch off a row merged away.
    let branch = vec![
        row("u9", merged_away, Role::User, "elsewhere"),
        row("a9", "u9", Role::Assistant, "ok"),
    ];
    let options = AppendOptions {
        make_tip: true,
        ..AppendOptions::default()
    };
    let outcome = append_to(&id, &after, &branch, &options).unwrap();
    assert_eq!(outcome.appended, ["u9", "a9"]);
    assert_eq!(outcome.tip_source_id.as_deref(), Some("a9"));
    let bytes = std::fs::read(&path).unwrap();
    let u9 = jsonl_lines(&bytes).filter_map(parse).find(|l| l["uuid"] == "u9").unwrap();
    assert_eq!(u9["parentUuid"], into.as_str());
    let mut all = synced.clone();
    all.extend(branch);
    testing::assert_caught_up(&all, &read_tip(&path).unwrap(), &read_back(&path));
}

/// A synced row, `role` `id` hanging from `parent`.
fn row(id: &str, parent: &str, role: Role, text: &str) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: Some(parent.to_owned()),
        timestamp: OffsetDateTime::now_utc(),
        role,
        content: vec![crate::harnesstools::session::Content::Text(text.to_owned())],
        model: Some("claude-x".to_owned()),
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: Some(PathBuf::from("/Users/someone/proj")),
        git_branch: None,
        seq: None,
    }
}

fn branch() -> Vec<RehydrateMessage> {
    vec![row("u3", "a1", Role::User, "on the mac"), row("a3", "u3", Role::Assistant, "sure")]
}

/// A branch another machine grew from an older line becomes the tip through an explicit
/// `last-prompt`: the last line alone would not make it one when a `last-prompt` names the old
/// leaf, as Claude Code writes one every turn.
#[rstest]
#[case::the_old_leaf_named(with(chain(), [last_prompt(Some("a2"), false)]), true)]
#[case::no_leaf_named(chain(), false)]
fn a_sibling_branch_is_made_the_tip(dir: TempDir, #[case] local: Vec<Value>, #[case] marked: bool) {
    let path = write(&dir, SESSION, &jsonl(&local));
    let base = read_tip(&path).unwrap();
    assert_eq!(base.tip_source_id.as_deref(), Some("a2"));

    let outcome = append_to(SESSION, &base, &branch(), &AppendOptions::default()).unwrap();
    assert_eq!(outcome.appended, vec!["u3", "a3"]);
    assert_eq!(outcome.marked_tip, marked);
    assert_eq!(outcome.tip_source_id.as_deref(), Some("a3"));
    assert_eq!(read_tip(&path).unwrap().tip_source_id.as_deref(), Some("a3"));

    let bytes = std::fs::read(&path).unwrap();
    let written: Vec<Value> = jsonl_lines(&bytes).filter_map(parse).skip(local.len()).collect();
    assert_eq!(written[0]["parentUuid"], "a1");
    assert_eq!(written[0]["sessionId"], SESSION);
    // Written where this copy works, not where the other machine did.
    assert_eq!(written[0]["cwd"], "/work/proj");
    assert_eq!(written[1]["parentUuid"], "u3");
    if marked {
        assert_eq!(
            written[2],
            json!({"type": "last-prompt", "leafUuid": "a3", "explicit": true, "sessionId": SESSION})
        );
    }
}

#[rstest]
fn make_tip_always_marks(dir: TempDir) {
    let path = write(&dir, SESSION, &jsonl(&chain()));
    let base = read_tip(&path).unwrap();
    let rows = vec![row("u3", "a2", Role::User, "go on")];
    let options = AppendOptions {
        make_tip: true,
        ..AppendOptions::default()
    };
    let outcome = append_to(SESSION, &base, &rows, &options).unwrap();
    assert!(outcome.marked_tip);
    assert_eq!(outcome.tip_source_id.as_deref(), Some("u3"));
}

/// A branch this copy already holds is made the tip with no line of its own.
#[rstest]
fn a_branch_already_here_is_made_the_tip(dir: TempDir) {
    let local = with(chain(), [user("u3", Some("a1"), 5), assistant("a3", "u3", 6)]);
    let path = write(&dir, SESSION, &jsonl(&local));
    let base = read_tip(&path).unwrap();
    assert_eq!(base.tip_source_id.as_deref(), Some("a3"));

    let options = AppendOptions {
        make_tip: true,
        head: Some("a2"),
        ..AppendOptions::default()
    };
    let outcome = append_to(SESSION, &base, &[], &options).unwrap();
    assert!(outcome.appended.is_empty());
    assert!(outcome.marked_tip);
    assert_eq!(read_tip(&path).unwrap().tip_source_id.as_deref(), Some("a2"));
    let again = read_tip(&path).unwrap();
    assert_eq!(again.known_source_ids, base.known_source_ids);
}

#[rstest]
fn a_transcript_changed_since_it_was_read_is_left_alone(dir: TempDir) {
    let path = write(&dir, SESSION, &jsonl(&chain()));
    let base = read_tip(&path).unwrap();
    std::fs::write(&path, jsonl(&with(chain(), [user("u9", Some("a2"), 9)]))).unwrap();
    let before = std::fs::read(&path).unwrap();
    let err = append_to(SESSION, &base, &branch(), &AppendOptions::default()).unwrap_err();
    assert!(matches!(err, SyncError::Changed), "{err:?}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[rstest]
#[case::hangs_from_nothing_here(vec![row("u3", "elsewhere", Role::User, "?")], "Disconnected")]
#[case::already_here(vec![row("a2", "u2", Role::Assistant, "again")], "IdTaken")]
fn a_segment_that_does_not_fit_is_refused(
    dir: TempDir,
    #[case] rows: Vec<RehydrateMessage>,
    #[case] expected: &str,
) {
    let path = write(&dir, SESSION, &jsonl(&chain()));
    let base = read_tip(&path).unwrap();
    let err = append_to(SESSION, &base, &rows, &AppendOptions::default()).unwrap_err();
    assert!(format!("{err:?}").starts_with(expected), "{err:?}");
    assert_eq!(std::fs::read(&path).unwrap(), jsonl(&chain()));
}

#[rstest]
fn a_torn_last_line_stays_torn(dir: TempDir) {
    let mut bytes = jsonl(&chain());
    bytes.extend_from_slice(br#"{"type":"user","uuid":"torn"#);
    let path = write(&dir, SESSION, &bytes);
    let base = read_tip(&path).unwrap();
    append_to(SESSION, &base, &branch(), &AppendOptions::default()).unwrap();
    let after = std::fs::read(&path).unwrap();
    let ids: Vec<String> = read_back(&path);
    assert!(ids.ends_with(&["u3".to_owned(), "a3".to_owned()]), "{ids:?}");
    assert!(after.windows(6).any(|w| w == b"\"torn\n"));
}

/// A `/proc` of its own, holding the processes `running` names: `(pid, starttime)`.
fn procs(dir: &TempDir, running: &[(u32, &str)]) -> Processes {
    let root = dir.path().join("proc");
    for (pid, start) in running {
        let at = root.join(pid.to_string());
        std::fs::create_dir_all(&at).unwrap();
        let fields = vec!["0"; 18].join(" ");
        std::fs::write(at.join("stat"), format!("{pid} (claude) S {fields} {start} 0 0")).unwrap();
        std::fs::write(at.join("cmdline"), b"claude\0").unwrap();
    }
    std::fs::create_dir_all(&root).unwrap();
    Processes::ProcFs(root)
}

const DOMAIN: &str = "linux:machine:pid:[1]";

fn register(dir: &TempDir, pid: u32, record: &Value) -> PathBuf {
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join(format!("{pid}.json")), record.to_string()).unwrap();
    sessions
}

fn record(pid: u32, session: &str, start: &str) -> Value {
    // The shape Claude Code 2.1.284 writes.
    json!({
        "pid": pid, "sessionId": session, "cwd": "/work/proj", "startedAt": 1_790_636_694_148_u64,
        "procStart": start, "version": "2.1.284", "kind": "interactive", "entrypoint": "cli",
        "pidDomain": DOMAIN, "status": "busy",
    })
}

#[rstest]
#[case::running(record(41, SESSION, "900"), &[(41, "900")], Liveness::Live { pid: Some(41) })]
#[case::another_session(record(41, "other", "900"), &[(41, "900")], Liveness::NotLive)]
#[case::crashed(record(41, SESSION, "900"), &[], Liveness::NotLive)]
#[case::pid_reused(record(41, SESSION, "900"), &[(41, "901")], Liveness::NotLive)]
#[case::another_pid_namespace(
    { let mut r = record(41, SESSION, "900"); r["pidDomain"] = json!("linux:machine:pid:[2]"); r },
    &[(41, "900")],
    Liveness::Unknown,
)]
#[case::no_start_recorded(
    { let mut r = record(41, SESSION, "900"); r.as_object_mut().unwrap().remove("procStart"); r },
    &[(41, "900")],
    Liveness::Live { pid: Some(41) },
)]
fn a_registered_session_is_live_while_its_process_runs(
    dir: TempDir,
    #[case] record: Value,
    #[case] running: &[(u32, &str)],
    #[case] expected: Liveness,
) {
    let sessions = register(&dir, 41, &record);
    let procs = procs(&dir, running);
    assert_eq!(liveness(&sessions, SESSION, &procs, DOMAIN), expected);
}

#[rstest]
fn a_record_that_cannot_be_read_is_unknown_while_its_process_runs(dir: TempDir) {
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("41.json"), "{\"pid\":").unwrap();
    assert_eq!(liveness(&sessions, SESSION, &procs(&dir, &[(41, "1")]), DOMAIN), Liveness::Unknown);
    std::fs::remove_dir_all(dir.path().join("proc")).unwrap();
    assert_eq!(liveness(&sessions, SESSION, &procs(&dir, &[]), DOMAIN), Liveness::NotLive);
}

#[rstest]
fn no_sessions_directory_is_nothing_running(dir: TempDir) {
    let procs = procs(&dir, &[]);
    assert_eq!(liveness(&dir.path().join("none"), SESSION, &procs, DOMAIN), Liveness::NotLive);
}

#[rstest]
fn a_live_session_is_not_appended_to() {
    assert!(matches!(require_idle(Liveness::Live { pid: Some(7) }), Err(SyncError::Live(Some(7)))));
    assert!(matches!(require_idle(Liveness::Unknown), Err(SyncError::MaybeLive)));
    assert!(require_idle(Liveness::NotLive).is_ok());
}
