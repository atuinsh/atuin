use std::num::NonZeroUsize;

use futures::TryStreamExt;
use rstest::{fixture, rstest};
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::Harness as _;
use crate::harnesstools::continuation::tests::{CLAUDE, rows_of};
use crate::harnesstools::continuation::{continued_from_message, marker_text};
use crate::harnesstools::copilot::session::{CopilotMessage, CopilotSession};
use crate::harnesstools::rehydrate::{ForkOf, MERGED_FIELD, is_uncaptured, testing};
use crate::harnesstools::session::{
    Message, ParentKind, Session, SessionId, TitleChange, TitleSource, ToolCallId, ToolResult,
    ToolUse,
};
use crate::sync::BlockingPool;

const TASK_AND_EDIT: &str = include_str!("../../../../tests/fixtures/copilot/task-and-edit.jsonl");
const COMPACTED: &str = include_str!("../../../../tests/fixtures/copilot/compacted.jsonl");
const ID: &str = "5d1f0b1e-4a8e-4f1b-9d59-2c0f2f3f0a11";

/// A sessions directory of its own, removed on drop.
#[fixture]
fn sessions() -> TempDir {
    tempfile::tempdir().unwrap()
}

fn pool() -> BlockingPool {
    BlockingPool::new(NonZeroUsize::MIN)
}

/// The log at `path`, read the way live capture reads it.
async fn read_back(id: &str, path: &Path) -> Vec<CopilotMessage> {
    CopilotSession::open(SessionId::from(id.to_owned()), path.to_owned(), pool())
        .read()
        .try_collect()
        .await
        .unwrap()
}

/// The ids of the rows read back, the sidecar's title row aside.
fn ids(read: &[CopilotMessage]) -> Vec<String> {
    read.iter().filter(|m| m.title().is_none()).filter_map(|m| m.id().map(String::from)).collect()
}

/// The captured rows of a Copilot log `events` (without a sidecar), as live capture makes them.
async fn captured(events: &str) -> Vec<RehydrateMessage> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s").joined(EVENTS_FILE);
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    std::fs::write(&path, events).unwrap();
    rows_of(read_back("s", &path).await.as_slice())
}

fn restore(cwd: &Path, messages: Vec<RehydrateMessage>) -> RehydrateSession {
    RehydrateSession {
        id: ID.to_owned(),
        title: Some("a restored session".to_owned()),
        cwd: cwd.to_owned(),
        original_cwd: Some(PathBuf::from("/tmp/copilot-play")),
        git_branch: Some("main".to_owned()),
        model: None,
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

/// The rows read back, but for the sidecar's title row, as `(id, role, content)`.
fn rows(messages: &[CopilotMessage]) -> Vec<(String, Role, Vec<Content>)> {
    messages
        .iter()
        .filter(|m| m.title().is_none())
        .filter_map(|m| Some((m.id()?.to_string(), m.role(), m.content())))
        .collect()
}

/// Every row the log can carry reads back from it under its own id, role and content (an edit's
/// patch included), so re-capturing the log dedups against the rows it came from; the session
/// is under its id, where `copilot --resume` looks; and the sidecar names it.
#[rstest]
#[case::task_and_edit(TASK_AND_EDIT)]
#[case::compacted(COMPACTED)]
#[tokio::test]
async fn a_session_reads_back_as_it_was_captured(sessions: TempDir, #[case] events: &str) {
    let cwd = sessions.path().join("my proj");
    let session = restore(&cwd, captured(events).await);

    let path = rehydrate_into(sessions.path(), &session).unwrap();
    assert_eq!(path, sessions.path().join(ID).joined(EVENTS_FILE));
    assert_eq!(locate(sessions.path(), ID), Some(path.clone()));

    let read = read_back(ID, &path).await;
    assert_eq!(rows(&read), carried(&session.messages));
    let again = ids(&read);
    testing::assert_nothing_new(&session.messages, again.iter().map(String::as_str));
    assert!(read.iter().all(|m| m.parent_session().is_none()));
    assert_eq!(read.iter().filter_map(Message::title).collect::<Vec<_>>(), [TitleChange::new(
        TitleSource::Named,
        "a restored session"
    )]);
    let replies: Vec<_> = read.iter().filter(|m| m.role() == Role::Assistant).collect();
    assert!(replies.iter().all(|m| m.usage().is_some() && m.model().is_some()));
    let turns: Vec<_> = replies.iter().map(|m| m.turn_id()).collect();
    let wanted: Vec<_> = session
        .messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .map(|m| m.turn_id.clone())
        .collect();
    assert_eq!(turns, wanted);
    assert!(read.iter().all(|m| m.cwd().as_deref() == Some(cwd.as_path()) || m.title().is_some()));
}

/// The first event says where the session works and who wrote the log, and the sidecar names
/// the session as the user's own name, so Copilot keeps it.
#[rstest]
#[tokio::test]
async fn the_log_opens_with_its_start_and_the_sidecar_names_it(sessions: TempDir) {
    let cwd = sessions.path().join("my \"proj\"");
    let session = restore(&cwd, captured(TASK_AND_EDIT).await);
    let path = rehydrate_into(sessions.path(), &session).unwrap();

    let text = std::fs::read_to_string(&path).unwrap();
    let first: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(first["type"], "session.start");
    assert_eq!(first["parentId"], serde_json::Value::Null);
    assert_eq!(first["data"]["sessionId"], ID);
    assert_eq!(first["data"]["producer"], PRODUCER);
    assert_eq!(first["data"]["copilotVersion"], COPILOT_VERSION);
    assert_eq!(first["data"]["context"]["cwd"], cwd.to_string_lossy().as_ref());
    assert_eq!(first["data"]["context"]["branch"], "main");
    assert!(first["data"].get("atuin").is_none(), "no fork");

    let sidecar = std::fs::read_to_string(path.with_file_name(SIDECAR_FILE)).unwrap();
    assert!(sidecar.contains(&format!("id: {ID}\n")), "{sidecar}");
    assert!(sidecar.contains("name: \"a restored session\"\nuser_named: true\n"), "{sidecar}");
    let quoted = yaml_text(&cwd.to_string_lossy());
    assert!(sidecar.contains(&format!("cwd: {quoted}\n")), "{sidecar}");
    assert!(sidecar.contains("branch: \"main\"\n"), "{sidecar}");
}

/// A title with the characters the sidecar escapes reads back as it was, under the same row.
#[rstest]
#[tokio::test]
async fn a_title_reads_back_as_it_was_written(sessions: TempDir) {
    let title = "Fix \"login\" in C:\\x\tnow";
    let session = RehydrateSession {
        title: Some(title.to_owned()),
        ..restore(sessions.path(), captured(TASK_AND_EDIT).await)
    };
    let path = rehydrate_into(sessions.path(), &session).unwrap();
    let read = read_back(ID, &path).await;
    let titles: Vec<_> = read.iter().filter_map(Message::title).collect();
    assert_eq!(titles, [TitleChange::new(TitleSource::Named, title)]);
}

#[rstest]
#[tokio::test]
async fn a_session_already_there_is_not_written_over(sessions: TempDir) {
    let restored = restore(sessions.path(), captured(TASK_AND_EDIT).await);
    let path = rehydrate_into(sessions.path(), &restored).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        rehydrate_into(sessions.path(), &restored),
        Err(RehydrateError::AlreadyExists(existing)) if existing == path
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let escaping = RehydrateSession {
        id: "../x".to_owned(),
        ..restored
    };
    assert!(matches!(rehydrate_into(sessions.path(), &escaping), Err(RehydrateError::Other(_))));
}

/// A fork names the session it was forked from in its start event, out of the model's sight,
/// and every row read back names it as the fork's parent.
#[rstest]
#[tokio::test]
async fn a_fork_names_the_original_in_its_start(sessions: TempDir) {
    let mut session = restore(sessions.path(), captured(TASK_AND_EDIT).await);
    session.fork_of = Some(ForkOf {
        id: "8623b9dc-b2aa-40dc-a8ce-1f3738d4d4db".to_owned(),
        atuin_id: Some("01a0d147e7457ae280000123456789ab".to_owned()),
        path: None,
    });
    let path = rehydrate_into(sessions.path(), &session).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let first: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(first["data"]["atuin"]["forkedFrom"], "8623b9dc-b2aa-40dc-a8ce-1f3738d4d4db");
    assert_eq!(first["data"]["atuin"]["atuinId"], "01a0d147e7457ae280000123456789ab");

    let read = read_back(ID, &path).await;
    assert!(!read.is_empty());
    assert!(read.iter().all(|m| {
        m.parent_session().as_ref().map(AsRef::as_ref)
            == Some("8623b9dc-b2aa-40dc-a8ce-1f3738d4d4db")
            && m.parent_kind() == Some(ParentKind::Fork)
    }));
}

/// A continuation's marker, text the harness puts in the user's turn, is a prompt the user
/// didn't type: read back as a system row, from which capture reads the marker.
#[rstest]
#[tokio::test]
async fn a_marker_is_a_prompt_the_user_did_not_type(sessions: TempDir) {
    let marker = marker_text(CLAUDE, "4bd6b5bd-7d34-4a39-a2d6-2e6f6a6c0b1d", None);
    let mut messages = captured(TASK_AND_EDIT).await;
    let marker_row = "7a9c7bd1-2b7e-4f8b-8c5e-5d1d1e7c0001";
    messages.insert(0, row(marker_row, Role::System, vec![Content::Text(marker.clone())]));
    let session = restore(sessions.path(), messages);
    let path = rehydrate_into(sessions.path(), &session).unwrap();

    let read = read_back(ID, &path).await;
    let first = read.iter().find(|m| m.title().is_none()).unwrap();
    assert_eq!(first.role(), Role::System);
    assert_eq!(first.content(), [Content::Text(marker)]);
    assert_eq!(first.id().unwrap().as_ref(), marker_row);
    let (from, id) = continued_from_message(first).expect("the marker links the continuation");
    assert_eq!((from.name(), id.as_str()), ("claude-code", "4bd6b5bd-7d34-4a39-a2d6-2e6f6a6c0b1d"));

    let text = std::fs::read_to_string(&path).unwrap();
    let line: serde_json::Value = serde_json::from_str(text.lines().nth(1).unwrap()).unwrap();
    assert_eq!(line["data"]["source"], ATUIN_SOURCE);
    assert_eq!(line["data"]["content"], "");
    assert!(line["data"]["transformedContent"].as_str().unwrap().starts_with("Continued from"));
}

/// Calls captured without their input become notes in their turn's text, their results
/// dropped, and the log still reads back under the rows' ids with nothing new.
#[rstest]
#[tokio::test]
async fn uncaptured_calls_become_notes(sessions: TempDir) {
    let messages = testing::synced(captured(TASK_AND_EDIT).await);
    assert!(testing::uncaptured(&messages) > 0, "the fixture has calls to flatten");
    let session = restore(sessions.path(), messages);
    let path = rehydrate_into(sessions.path(), &session).unwrap();

    let read = read_back(ID, &path).await;
    let content: Vec<Content> = read.iter().flat_map(Message::content).collect();
    assert!(!content.iter().any(is_uncaptured));
    assert!(!content.iter().any(|c| matches!(c, Content::ToolResult(_))));
    assert!(
        content.iter().any(|c| matches!(c, Content::Text(t) if t.contains("[ran a shell command"))),
        "{content:?}"
    );
    let again = ids(&read);
    testing::assert_nothing_new(&session.messages, again.iter().map(String::as_str));
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(lines.iter().any(|l| l.get(MERGED_FIELD).is_some()), "the rows merged are recorded");
}

/// A row of several results (Claude Code's) is several events: the first under the row's id,
/// the rest under ids of their own, each reading back as a result.
#[rstest]
#[tokio::test]
async fn a_row_of_several_results_is_several_events(sessions: TempDir) {
    let result = |n: u32| {
        Content::ToolResult(ToolResult {
            call: ToolCallId::from(format!("call_{n}")),
            output: serde_json::json!(format!("out {n}")),
            error: n == 2,
        })
    };
    let calls = (1..=2)
        .map(|n| {
            Content::ToolUse(ToolUse {
                id: ToolCallId::from(format!("call_{n}")),
                name: "bash".to_owned(),
                input: serde_json::json!({"command": format!("echo {n}")}),
            })
        })
        .collect();
    let messages = vec![
        row("u1", Role::User, vec![Content::Text("run both".to_owned())]),
        row("a1", Role::Assistant, calls),
        row("t1", Role::Tool, vec![result(1), result(2)]),
    ];
    let session = restore(sessions.path(), messages);
    let path = rehydrate_into(sessions.path(), &session).unwrap();

    let read = read_back(ID, &path).await;
    let results: Vec<_> = read.iter().filter(|m| m.role() == Role::Tool).collect();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].id().unwrap().as_ref(), "t1");
    assert_ne!(results[1].id().unwrap().as_ref(), "t1");
    assert_eq!(results[0].content(), [result(1)]);
    assert_eq!(results[1].content(), [result(2)]);
}

/// A call id too long for the model API (a Codex one is 64 characters; the API takes 64 with
/// Copilot's own prefix) is written as a shorter one derived from it, on the call and its result
/// alike; one that fits is written as it is.
#[rstest]
#[tokio::test]
async fn a_call_id_too_long_for_the_api_is_shortened(sessions: TempDir) {
    let long = format!("call_{}_fc_{}", "0".repeat(32), "1".repeat(23));
    assert_eq!(long.len(), 64);
    let call = |id: &str| {
        Content::ToolUse(ToolUse {
            id: ToolCallId::from(id.to_owned()),
            name: "bash".to_owned(),
            input: serde_json::json!({"command": "ls"}),
        })
    };
    let result = |id: &str| {
        Content::ToolResult(ToolResult {
            call: ToolCallId::from(id.to_owned()),
            output: serde_json::json!("out"),
            error: false,
        })
    };
    let messages = vec![
        row("u1", Role::User, vec![Content::Text("run".to_owned())]),
        row("a1", Role::Assistant, vec![call(&long), call("call_short")]),
        row("t1", Role::Tool, vec![result(&long), result("call_short")]),
    ];
    let session = restore(sessions.path(), messages);
    let path = rehydrate_into(sessions.path(), &session).unwrap();

    let read = read_back(ID, &path).await;
    let calls: Vec<ToolCallId> = read
        .iter()
        .flat_map(|m| m.content())
        .filter_map(|c| match c {
            Content::ToolUse(u) => Some(u.id),
            _ => None,
        })
        .collect();
    let results: Vec<ToolCallId> = read
        .iter()
        .flat_map(|m| m.content())
        .filter_map(|c| match c {
            Content::ToolResult(r) => Some(r.call),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls, results);
    assert_ne!(calls[0].as_ref(), long);
    assert!(calls[0].as_ref().len() <= TOOL_CALL_ID_MAX, "{:?}", calls[0]);
    assert_eq!(calls[1].as_ref(), "call_short");
}
