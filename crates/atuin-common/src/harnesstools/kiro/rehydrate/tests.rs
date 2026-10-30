use std::num::NonZeroUsize;

use futures::TryStreamExt;
use rstest::{fixture, rstest};
use serde_json::json;
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::Harness as _;
use crate::harnesstools::continuation::tests::{CLAUDE, rows_of};
use crate::harnesstools::continuation::{continued_from_message, marker_text};
use crate::harnesstools::kiro::session::{KiroMessage, KiroSession};
use crate::harnesstools::rehydrate::{ForkOf, MERGED_FIELD, is_uncaptured, testing};
use crate::harnesstools::session::{
    Message, ParentKind, Session, SessionId, TitleChange, TitleSource, ToolCallId,
};
use crate::sync::BlockingPool;

const RESUMED: &str = include_str!("../../../../tests/fixtures/kiro/resumed.jsonl");
const RESUMED_HEADER: &str = include_str!("../../../../tests/fixtures/kiro/resumed.session.json");
const COMPACTED: &str = include_str!("../../../../tests/fixtures/kiro/compacted.jsonl");
const COMPACTED_HEADER: &str =
    include_str!("../../../../tests/fixtures/kiro/compacted.session.json");
const ID: &str = "sess_5d1f0b1e-4a8e-4f1b-9d59-2c0f2f3f0a11";

/// A Kiro home of its own, removed on drop.
#[fixture]
fn home() -> TempDir {
    tempfile::tempdir().unwrap()
}

fn pool() -> BlockingPool {
    BlockingPool::new(NonZeroUsize::MIN)
}

/// The log at `path`, read the way live capture reads it.
async fn read_back(id: &str, path: &Path) -> Vec<KiroMessage> {
    KiroSession::open(SessionId::from(id.to_owned()), path.to_owned(), pool())
        .read()
        .try_collect()
        .await
        .unwrap()
}

/// The ids of the rows read back, the header's title row aside.
fn ids(read: &[KiroMessage]) -> Vec<String> {
    read.iter().filter(|m| m.title().is_none()).filter_map(|m| m.id().map(String::from)).collect()
}

/// The captured rows of a Kiro log `log` with its `header`, as live capture makes them.
async fn captured(log: &str, header: &str) -> Vec<RehydrateMessage> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("b").joined("s").joined(LOG_FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, log).unwrap();
    std::fs::write(path.with_file_name(HEADER_FILE), header).unwrap();
    rows_of(read_back("s", &path).await.as_slice())
}

fn restore(cwd: &Path, messages: Vec<RehydrateMessage>) -> RehydrateSession {
    RehydrateSession {
        id: ID.to_owned(),
        title: Some("a restored session".to_owned()),
        cwd: cwd.to_owned(),
        original_cwd: Some(PathBuf::from("/tmp/kiro-play")),
        git_branch: None,
        model: Some("auto".to_owned()),
        started_at: OffsetDateTime::UNIX_EPOCH,
        messages,
        fork_of: None,
    }
}

fn row(id: &str, role: Role, content: Vec<Content>) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: None,
        timestamp: OffsetDateTime::UNIX_EPOCH,
        role,
        content,
        model: None,
        provider: None,
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
    }
}

fn call(id: &str, name: &str, input: Value) -> Content {
    Content::ToolUse(ToolUse {
        id: ToolCallId::from(id.to_owned()),
        name: name.to_owned(),
        input,
    })
}

fn result(id: &str, output: &str, error: bool) -> Content {
    Content::ToolResult(ToolResult {
        call: ToolCallId::from(id.to_owned()),
        output: json!(output),
        error,
    })
}

/// What the log can carry of each row, by the rules in the module docs: `None` for a row with
/// nothing left.
fn carried(messages: &[RehydrateMessage]) -> Vec<(String, Role, Vec<Content>)> {
    messages
        .iter()
        .filter(|m| !matches!(m.role, Role::Other(_)))
        .filter_map(|m| {
            let content: Vec<Content> = m
                .content
                .iter()
                .filter(|c| !matches!(c, Content::Reasoning(_) | Content::ReasoningSummary { .. }))
                .filter(|c| !matches!(c, Content::Text(t) if t.is_empty()))
                .cloned()
                .collect();
            (!content.is_empty()).then(|| (m.source_id.clone(), m.role.clone(), content))
        })
        .collect()
}

/// The rows read back, but for the header's title row, as `(id, role, content)`.
fn rows(messages: &[KiroMessage]) -> Vec<(String, Role, Vec<Content>)> {
    messages
        .iter()
        .filter(|m| m.title().is_none())
        .filter_map(|m| Some((m.id()?.to_string(), m.role(), m.content())))
        .collect()
}

/// The lines of the log at `path`.
fn lines_of(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Every row the log can carry reads back from it under its own id, role and content, with its
/// turn and stop reason, so re-capturing the log dedups against the rows it came from; the
/// session is in the bucket of its directory, where `kiro-cli` looks first; and the header names
/// it.
#[rstest]
#[case::resumed(RESUMED, RESUMED_HEADER)]
#[case::compacted(COMPACTED, COMPACTED_HEADER)]
#[tokio::test]
async fn a_session_reads_back_as_it_was_captured(
    home: TempDir,
    #[case] log: &str,
    #[case] header: &str,
) {
    let cwd = home.path().join("my proj");
    let session = restore(&cwd, captured(log, header).await);

    let path = rehydrate_into(home.path(), &session).unwrap();
    let sessions = home.path().join("sessions");
    assert_eq!(path, sessions.join(workspace_hash(&cwd)).joined(ID).joined(LOG_FILE));
    assert_eq!(locate(&sessions, ID), Some(path.clone()));

    let read = read_back(ID, &path).await;
    pretty_assertions::assert_eq!(rows(&read), carried(&session.messages));
    let again = ids(&read);
    testing::assert_nothing_new(&session.messages, again.iter().map(String::as_str));
    assert!(read.iter().all(|m| m.parent_session().is_none()));
    assert_eq!(read.iter().filter_map(Message::title).collect::<Vec<_>>(), [TitleChange::new(
        TitleSource::Generated,
        "a restored session"
    )]);
    let turns: Vec<_> = read
        .iter()
        .filter(|m| m.role() == Role::Assistant)
        .map(|m| (m.turn_id(), m.stop_reason()))
        .collect();
    let wanted: Vec<_> = session
        .messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .filter(|m| !m.content.iter().all(|c| matches!(c, Content::ReasoningSummary { .. })))
        .map(|m| (m.turn_id.clone(), m.stop_reason.clone()))
        .collect();
    assert_eq!(turns, wanted);
    assert!(read.iter().all(|m| m.cwd().as_deref() == Some(cwd.as_path()) || m.title().is_some()));
    assert!(
        read.iter()
            .filter(|m| m.title().is_none())
            .all(|m| m.model().is_none() || m.model().as_deref() == Some("auto"))
    );
}

/// The header says where the session works, its title and model, and that it is idle; the
/// session's line is in the index Kiro reads before scanning.
#[rstest]
#[tokio::test]
async fn the_header_names_the_session_and_the_index_lists_it(home: TempDir) {
    let cwd = home.path().join("my \"proj\"");
    let session = restore(&cwd, captured(RESUMED, RESUMED_HEADER).await);
    let path = rehydrate_into(home.path(), &session).unwrap();

    let header: Value =
        serde_json::from_str(&std::fs::read_to_string(path.with_file_name(HEADER_FILE)).unwrap())
            .unwrap();
    assert_eq!(header["schemaVersion"], SCHEMA_VERSION);
    assert_eq!(header["id"], ID);
    assert_eq!(header["title"], "a restored session");
    assert_eq!(header["agentMode"], AGENT_MODE);
    assert_eq!(header["workspacePaths"], json!([cwd]));
    assert_eq!(header["rootPaths"], json!([cwd]));
    assert_eq!(header["modelId"], "auto");
    assert_eq!(header["status"], "idle");
    assert_eq!(header["createdAt"], "1970-01-01T00:00:00.000Z");
    assert!(header.get("parentSessionId").is_none(), "no fork");

    let bucket = workspace_hash(&cwd);
    let index = home.path().join("session-index").joined(format!("{bucket}.jsonl"));
    let line: Value =
        serde_json::from_str(std::fs::read_to_string(&index).unwrap().trim()).unwrap();
    assert_eq!(line["op"], "add");
    assert_eq!(line["sessionPath"], format!("{bucket}/{ID}"));
    assert_eq!(line["at"], 0);
}

/// The bucket is Kiro's: the hash of the directory with no trailing slash, and on Windows of
/// its lowercase form with forward slashes.
#[rstest]
fn the_bucket_is_the_directorys_hash() {
    let probe = "/private/tmp/claude-501/-Users-binarymuse-src-mono-atuin/\
                 e2cf9d4c-2d71-4590-85f1-815ee8031a93/scratchpad/kiro-play";
    if !cfg!(windows) {
        assert_eq!(workspace_hash(Path::new(probe)), "c15e787aea4828dd");
        assert_eq!(workspace_hash(Path::new(&format!("{probe}/"))), "c15e787aea4828dd");
    }
    assert_eq!(workspace_hash(Path::new("/")), workspace_hash(Path::new("/")));
    assert_eq!(
        workspace_hash(Path::new("C:\\Work\\Proj\\")),
        workspace_hash(Path::new(if cfg!(windows) {
            "c:/work/proj"
        } else {
            "C:/Work/Proj"
        }))
    );
}

/// A title reads back as it was written, under the same row.
#[rstest]
#[tokio::test]
async fn a_title_reads_back_as_it_was_written(home: TempDir) {
    let title = "Fix \"login\" in C:\\x\tnow";
    let session = RehydrateSession {
        title: Some(title.to_owned()),
        ..restore(home.path(), captured(RESUMED, RESUMED_HEADER).await)
    };
    let path = rehydrate_into(home.path(), &session).unwrap();
    let read = read_back(ID, &path).await;
    let titles: Vec<_> = read.iter().filter_map(Message::title).collect();
    assert_eq!(titles, [TitleChange::new(TitleSource::Generated, title)]);
}

#[rstest]
#[tokio::test]
async fn a_session_already_there_is_not_written_over(home: TempDir) {
    let restored = restore(home.path(), captured(RESUMED, RESUMED_HEADER).await);
    let path = rehydrate_into(home.path(), &restored).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        rehydrate_into(home.path(), &restored),
        Err(RehydrateError::AlreadyExists(existing)) if existing == path
    ));
    // In another bucket too: a session never moves buckets.
    let elsewhere = RehydrateSession {
        cwd: home.path().join("elsewhere"),
        ..restored.clone()
    };
    assert!(matches!(
        rehydrate_into(home.path(), &elsewhere),
        Err(RehydrateError::AlreadyExists(existing)) if existing == path
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let escaping = RehydrateSession {
        id: "../x".to_owned(),
        ..restored
    };
    assert!(matches!(rehydrate_into(home.path(), &escaping), Err(RehydrateError::Other(_))));
}

/// A fork names the session it was forked from in its header, out of the model's sight, and
/// every row read back names it as the fork's parent.
#[rstest]
#[tokio::test]
async fn a_fork_names_the_original_in_its_header(home: TempDir) {
    let mut session = restore(home.path(), captured(RESUMED, RESUMED_HEADER).await);
    session.fork_of = Some(ForkOf {
        id: "sess_7c362b91-6e72-4c10-be1f-76a31a8d5f48".to_owned(),
        atuin_id: Some("01a0d147e7457ae280000123456789ab".to_owned()),
        path: None,
    });
    let path = rehydrate_into(home.path(), &session).unwrap();
    let header: Value =
        serde_json::from_str(&std::fs::read_to_string(path.with_file_name(HEADER_FILE)).unwrap())
            .unwrap();
    assert_eq!(header["parentSessionId"], "sess_7c362b91-6e72-4c10-be1f-76a31a8d5f48");
    assert_eq!(header["forkedAtMessageId"], session.messages.last().unwrap().source_id);
    assert_eq!(header["_meta"]["kiro"]["atuin"]["atuinId"], "01a0d147e7457ae280000123456789ab");

    let read = read_back(ID, &path).await;
    let rows: Vec<_> = read.iter().filter(|m| m.title().is_none()).collect();
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|m| {
        m.parent_session().as_ref().map(AsRef::as_ref)
            == Some("sess_7c362b91-6e72-4c10-be1f-76a31a8d5f48")
            && m.parent_kind() == Some(ParentKind::Fork)
    }));
}

/// A continuation's marker, text the harness puts in the user's turn, is a note Kiro gives the
/// model: read back as a system row, from which capture reads the marker.
#[rstest]
#[tokio::test]
async fn a_marker_is_a_note_to_the_model(home: TempDir) {
    let marker = marker_text(CLAUDE, "4bd6b5bd-7d34-4a39-a2d6-2e6f6a6c0b1d", None);
    let mut messages = captured(RESUMED, RESUMED_HEADER).await;
    let marker_row = "7a9c7bd1-2b7e-4f8b-8c5e-5d1d1e7c0001";
    messages.insert(0, row(marker_row, Role::System, vec![Content::Text(marker.clone())]));
    let session = restore(home.path(), messages);
    let path = rehydrate_into(home.path(), &session).unwrap();

    let read = read_back(ID, &path).await;
    let first = read.iter().find(|m| m.title().is_none()).unwrap();
    assert_eq!(first.role(), Role::System);
    assert_eq!(first.content(), [Content::Text(marker)]);
    assert_eq!(first.id().unwrap().as_ref(), marker_row);
    let (from, id) = continued_from_message(first).expect("the marker links the continuation");
    assert_eq!((from.name(), id.as_str()), ("claude-code", "4bd6b5bd-7d34-4a39-a2d6-2e6f6a6c0b1d"));

    let line = &lines_of(&path)[0];
    assert_eq!(line["payload"]["type"], "agent_note");
    assert!(line["payload"]["content"].as_str().unwrap().starts_with("Continued from"));
}

/// Calls captured without their input become notes in their turn's text, their results
/// dropped, and the log still reads back under the rows' ids with nothing new.
#[rstest]
#[tokio::test]
async fn uncaptured_calls_become_notes(home: TempDir) {
    let messages = testing::synced(captured(RESUMED, RESUMED_HEADER).await);
    assert!(testing::uncaptured(&messages) > 0, "the fixture has calls to flatten");
    let session = restore(home.path(), messages);
    let path = rehydrate_into(home.path(), &session).unwrap();

    let read = read_back(ID, &path).await;
    let content: Vec<Content> = read.iter().flat_map(Message::content).collect();
    assert!(!content.iter().any(is_uncaptured));
    assert!(!content.iter().any(|c| matches!(c, Content::ToolResult(_))));
    assert!(
        content
            .iter()
            .any(|c| matches!(c, Content::Text(t) if t.contains("[ran a shell command]"))),
        "{content:?}"
    );
    let again = ids(&read);
    testing::assert_nothing_new(&session.messages, again.iter().map(String::as_str));
    let lines = lines_of(&path);
    assert!(lines.iter().any(|l| l.get(MERGED_FIELD).is_some()), "the rows merged are recorded");
    assert!(
        lines
            .iter()
            .all(|l| l["payload"]["type"] != "tool_call" || l["payload"]["status"] == CALL_STATUS)
    );
}

/// A Claude Code reply of text and several calls is a `Say` under the row's id and a
/// `tool_call` each, a row of several results a `tool_result` each: all in one turn, framed by
/// its start and end, each call carrying the status Kiro replays it under.
#[rstest]
#[tokio::test]
async fn a_row_of_text_and_calls_is_a_say_and_a_call_each(home: TempDir) {
    let calls = vec![
        Content::Text("Running both.".to_owned()),
        call("call_1", "bash", json!({"command": "echo 1"})),
        call("call_2", "bash", json!({"command": "echo 2"})),
    ];
    let mut reply = row("a1", Role::Assistant, calls);
    reply.stop_reason = Some(StopReason::ToolUse);
    reply.turn_id = Some("msg_01".to_owned());
    let mut done = row("a2", Role::Assistant, vec![Content::Text("Done.".to_owned())]);
    done.stop_reason = Some(StopReason::EndTurn);
    let messages = vec![
        row("u1", Role::User, vec![Content::Text("run both".to_owned())]),
        reply,
        row("t1", Role::Tool, vec![
            result("call_1", "out 1", false),
            result("call_2", "out 2", true),
        ]),
        done,
        row("u2", Role::User, vec![Content::Text("thanks".to_owned())]),
    ];
    let session = restore(home.path(), messages);
    let path = rehydrate_into(home.path(), &session).unwrap();

    let kinds: Vec<(String, String)> = lines_of(&path)
        .iter()
        .map(|l| {
            (
                l["id"].as_str().unwrap().to_owned(),
                l["payload"]["type"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let kinds: Vec<(&str, &str)> =
        kinds.iter().map(|(id, kind)| (id.as_str(), kind.as_str())).collect();
    assert_eq!(kinds[0], ("u1", "user"));
    assert_eq!(kinds[1], ("msg_01-turn-start", "turn_start"));
    assert_eq!(kinds[2], ("a1", "assistant"));
    assert_eq!((kinds[3].1, kinds[4].1), ("tool_call", "tool_call"));
    assert_eq!(kinds[5], ("t1", "tool_result"));
    assert_eq!(kinds[6].1, "tool_result");
    assert_eq!(kinds[7], ("a2", "assistant"));
    assert_eq!(kinds[8], ("msg_01-turn-end", "turn_end"));
    assert_eq!(kinds[9], ("u2", "user"));
    assert_eq!(kinds.len(), 10, "{kinds:?}");
    let lines = lines_of(&path);
    assert_eq!(lines[3]["payload"]["status"], CALL_STATUS);
    assert_eq!(lines[3]["payload"]["executionId"], "msg_01");
    assert_eq!(lines[6]["payload"]["success"], false);
    assert_eq!(lines[8]["payload"]["stopReason"], "end_turn");

    let read = read_back(ID, &path).await;
    let results: Vec<_> = read.iter().filter(|m| m.role() == Role::Tool).collect();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].id().unwrap().as_ref(), "t1");
    assert_ne!(results[1].id().unwrap().as_ref(), "t1");
    assert_eq!(results[0].content(), [result("call_1", "out 1", false)]);
    assert_eq!(results[1].content(), [result("call_2", "out 2", true)]);
    let calls: Vec<_> = read
        .iter()
        .flat_map(Message::content)
        .filter_map(|c| match c {
            Content::ToolUse(u) => Some(u.id),
            _ => None,
        })
        .collect();
    assert_eq!(calls, [
        ToolCallId::from("call_1".to_owned()),
        ToolCallId::from("call_2".to_owned())
    ]);
    assert_eq!(
        read.iter()
            .filter(|m| m.id().is_some_and(|id| id.as_ref() == "a2"))
            .map(|m| m.stop_reason())
            .collect::<Vec<_>>(),
        [Some(StopReason::EndTurn)]
    );
}

/// A compaction's summary covers the log before it: a tombstone from the log's first row, then
/// the summary, which reads back as the system row it was. A summary opening a log has nothing
/// to cover.
#[rstest]
#[tokio::test]
async fn a_summary_covers_the_rows_before_it(home: TempDir) {
    let messages = vec![
        row("u1", Role::User, vec![Content::Text("first".to_owned())]),
        row("a1", Role::Assistant, vec![Content::Text("ok".to_owned())]),
        row("s1", Role::System, vec![Content::Summary("we did the first thing".to_owned())]),
        row("u2", Role::User, vec![Content::Text("second".to_owned())]),
        row("a2", Role::Assistant, vec![Content::Text("done".to_owned())]),
    ];
    let session = restore(home.path(), messages);
    let path = rehydrate_into(home.path(), &session).unwrap();
    let lines = lines_of(&path);
    let tombstone = lines.iter().find(|l| l["payload"]["type"] == "tombstone").unwrap();
    assert_eq!(tombstone["payload"]["kind"], "summarization");
    assert_eq!(tombstone["payload"]["effectiveFromMessageId"], "u1");
    assert_eq!(tombstone["payload"]["metadata"]["truncatedMessageCount"], 4, "{lines:?}");
    let at = lines.iter().position(|l| l["id"] == "s1").unwrap();
    assert_eq!(lines[at - 1]["payload"]["type"], "tombstone");
    assert_eq!(lines[at - 2]["payload"]["type"], "turn_end", "the turn ends before the summary");
    assert_eq!(lines[at]["payload"]["operationType"], "Summary");

    let read = read_back(ID, &path).await;
    let summary = read.iter().find(|m| m.id().is_some_and(|id| id.as_ref() == "s1")).unwrap();
    assert_eq!(summary.role(), Role::System);
    assert_eq!(summary.content(), [Content::Summary("we did the first thing".to_owned())]);

    let opening = restore(home.path(), vec![
        row("s0", Role::System, vec![Content::Summary("before".to_owned())]),
        row("u1", Role::User, vec![Content::Text("go".to_owned())]),
    ]);
    let other = RehydrateSession {
        id: "sess_other".to_owned(),
        ..opening
    };
    let path = rehydrate_into(home.path(), &other).unwrap();
    assert!(lines_of(&path).iter().all(|l| l["payload"]["type"] != "tombstone"));
}
