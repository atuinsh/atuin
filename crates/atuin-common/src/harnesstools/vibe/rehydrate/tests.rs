use std::path::PathBuf;

use jiff::Timestamp;
use rstest::rstest;
use serde_json::{Value, json};

use super::*;
use crate::harnesstools::session::{ToolCallId, ToolResult, ToolUse};

const AT: Timestamp = Timestamp::constant(1_795_003_200, 0);

fn row(id: &str, role: Role, content: Vec<Content>) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: None,
        timestamp: AT,
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

fn text(t: &str) -> Content {
    Content::Text(t.to_owned())
}

fn call(id: &str, command: &str) -> Content {
    Content::ToolUse(ToolUse {
        id: ToolCallId::from(id.to_owned()),
        name: "bash".to_owned(),
        input: json!({ "command": command }),
    })
}

fn result(id: &str, output: &str) -> Content {
    Content::ToolResult(ToolResult {
        call: ToolCallId::from(id.to_owned()),
        output: json!(output),
        error: false,
    })
}

fn session(messages: Vec<RehydrateMessage>) -> RehydrateSession {
    RehydrateSession {
        id: "a7e0f00d-1111-4222-8333-944455556666".to_owned(),
        title: Some("A title".to_owned()),
        cwd: PathBuf::from("/w"),
        original_cwd: None,
        git_branch: Some("main".to_owned()),
        model: None,
        started_at: AT,
        messages,
        fork_of: None,
    }
}

/// What the Mistral API checks of a history: a tool message follows the assistant message that
/// made its call (or another tool message), and every call is answered before the next message.
pub fn assert_the_api_takes(messages: &[Value]) {
    let mut open: Vec<String> = Vec::new();
    for (n, message) in messages.iter().enumerate() {
        match message["role"].as_str().unwrap() {
            "tool" => {
                let id = message["tool_call_id"].as_str().unwrap();
                let at = open.iter().position(|c| c == id);
                assert!(at.is_some(), "message {n} answers no open call: {messages:#?}");
                open.remove(at.unwrap());
            }
            role => {
                assert!(open.is_empty(), "message {n} ({role}) comes before {open:?} are answered");
                if let Some(calls) = message["tool_calls"].as_array() {
                    open = calls.iter().map(|c| c["id"].as_str().unwrap().to_owned()).collect();
                }
            }
        }
    }
    assert!(open.is_empty(), "{open:?} are never answered");
}

#[rstest]
fn a_turns_rows_become_one_message_with_its_calls_answered() {
    let rows = vec![
        row("u1", Role::User, vec![text("list it")]),
        row("a1", Role::Assistant, vec![Content::Reasoning("think".into()), call("c1", "ls")]),
        row("a2", Role::Assistant, vec![call("c2", "pwd")]),
        row("t1", Role::Tool, vec![result("c1", "a\nb")]),
        row("t2", Role::Tool, vec![result("c2", "/w")]),
        row("a3", Role::Assistant, vec![text("done")]),
    ];
    let messages = messages(&rows);
    assert_the_api_takes(&messages);
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[1]["reasoning_content"], "think");
    assert_eq!(messages[1]["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(messages[1]["tool_calls"][0]["function"]["arguments"], r#"{"command":"ls"}"#);
    assert_eq!(messages[2]["content"], "a\nb");
    assert_eq!(messages[4]["content"], "done");
}

#[rstest]
#[case::unanswered_at_the_end(vec![
    row("u1", Role::User, vec![text("hi")]),
    row("a1", Role::Assistant, vec![call("c1", "ls")]),
])]
#[case::unanswered_before_a_prompt(vec![
    row("u1", Role::User, vec![text("hi")]),
    row("a1", Role::Assistant, vec![call("c1", "ls")]),
    row("u2", Role::User, vec![text("never mind")]),
    row("a2", Role::Assistant, vec![text("ok")]),
])]
#[case::an_orphan_result(vec![
    row("u1", Role::User, vec![text("hi")]),
    row("t1", Role::Tool, vec![result("nope", "x")]),
    row("a1", Role::Assistant, vec![text("ok")]),
])]
#[case::a_result_with_a_prompt(vec![
    row("a1", Role::Assistant, vec![call("c1", "ls")]),
    row("u1", Role::User, vec![result("c1", "a"), text("and then?")]),
])]
#[case::a_call_id_twice(vec![
    row("a1", Role::Assistant, vec![call("c1", "ls"), call("c1", "ls")]),
    row("t1", Role::Tool, vec![result("c1", "a")]),
    row("t2", Role::Tool, vec![result("c1", "a")]),
])]
#[case::a_call_captured_without_its_input(vec![
    row("u1", Role::User, vec![text("hi")]),
    row("a1", Role::Assistant, vec![Content::ToolUse(ToolUse { id: ToolCallId::from("c1".to_owned()), name: "bash".into(), input: Value::Null })]),
    row("t1", Role::Tool, vec![result("c1", "a")]),
])]
fn the_api_takes_whatever_was_captured(#[case] rows: Vec<RehydrateMessage>) {
    assert_the_api_takes(&messages(&rows));
}

#[rstest]
fn an_unanswered_call_says_no_result_was_recorded() {
    let rows = vec![row("a1", Role::Assistant, vec![call("c1", "ls")])];
    let messages = messages(&rows);
    assert_eq!(messages[1], json!({ "role": "tool", "tool_call_id": "c1", "content": NO_RESULT }));
}

/// Vibe's importer drops system messages, so a system row's text is sent as the user's.
#[rstest]
fn a_system_rows_text_goes_in_as_the_users() {
    let rows = vec![
        row("s1", Role::System, vec![Content::Summary("we discussed MANGO".into())]),
        row("u1", Role::User, vec![text("what fruit?")]),
    ];
    let messages = messages(&rows);
    assert_eq!(messages[0], json!({ "role": "user", "content": "we discussed MANGO" }));
}

#[rstest]
#[tokio::test]
async fn writes_a_legacy_folder_vibe_imports_once() {
    let root = tempfile::tempdir().unwrap();
    let rows = vec![
        row("u1", Role::User, vec![text("hi")]),
        row("a1", Role::Assistant, vec![text("hello")]),
    ];
    let session = session(rows);
    let folder = rehydrate(root.path(), &session).await.unwrap();
    assert_eq!(folder, root.path().join("session_20261118_120000_a7e0f00d"));

    let meta: Value =
        serde_json::from_slice(&std::fs::read(folder.join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["session_id"], session.id);
    assert_eq!(meta["environment"]["working_directory"], "/w");
    assert_eq!(meta["title"], "A title");
    assert_eq!(meta["git_branch"], "main");
    assert!(meta.get(ATUIN_KEY).is_some());
    let written = super::super::session::read_written(&folder).unwrap();
    assert_eq!(written.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["u1", "a1"]);
    assert_eq!(written[1].content, [text("hello")]);
    let lines: Vec<Value> = std::fs::read_to_string(folder.join("messages.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines, [
        json!({ "role": "user", "content": "hi" }),
        json!({ "role": "assistant", "content": "hello" })
    ]);

    assert_eq!(locate(root.path(), &session.id), Some(folder.clone()));
    assert!(matches!(
        rehydrate(root.path(), &session).await,
        Err(RehydrateError::AlreadyExists(at)) if at == folder
    ));
}

#[rstest]
#[case::empty("")]
#[case::short("abc")]
#[case::a_path("../../a7e0f00d")]
#[tokio::test]
async fn refuses_an_id_vibe_cant_resume(#[case] id: &str) {
    let root = tempfile::tempdir().unwrap();
    let mut session = session(Vec::new());
    session.id = id.to_owned();
    assert!(matches!(rehydrate(root.path(), &session).await, Err(RehydrateError::Other(_))));
}

/// A folder Vibe hasn't imported yet takes more rows, and records them with the rows written
/// before; one that changed since it was read is left alone.
#[rstest]
#[tokio::test]
async fn rows_append_to_a_folder_vibe_has_not_imported() {
    use crate::harnesstools::sync::{Stamp, SyncError};

    let root = tempfile::tempdir().unwrap();
    let session = session(vec![row("u1", Role::User, vec![text("hi")])]);
    let folder = rehydrate(root.path(), &session).await.unwrap();
    let transcript = folder.join("messages.jsonl");
    let stamp = Stamp::of(&std::fs::read(&transcript).unwrap());

    let more = vec![
        row("a1", Role::Assistant, vec![call("c1", "ls")]),
        row("t1", Role::Tool, vec![result("c1", "a")]),
    ];
    assert_eq!(append(&folder, stamp, &more).unwrap(), ["a1", "t1"]);
    let lines: Vec<Value> = std::fs::read_to_string(&transcript)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_the_api_takes(&lines);
    assert_eq!(lines.len(), 3);
    let (rows, cwd) = super::super::session::written_rows(&folder).unwrap();
    assert_eq!(rows, ["u1", "a1", "t1"]);
    assert_eq!(cwd, Some(PathBuf::from("/w")));

    assert!(matches!(append(&folder, stamp, &more), Err(SyncError::Changed)));
    let (rows, _) = super::super::session::written_rows(&folder).unwrap();
    assert_eq!(rows, ["u1", "a1", "t1"], "a refused append records no row");

    // An append that recorded its rows but failed before writing the transcript: the rows are
    // not held, and the next append goes on from those that are.
    let mut recorded = std::fs::read(folder.join(WRITTEN_FILE)).unwrap();
    recorded.extend(written(&[row("lost", Role::User, vec![text("lost")])]).unwrap());
    std::fs::write(folder.join(WRITTEN_FILE), recorded).unwrap();
    let (rows, _) = super::super::session::written_rows(&folder).unwrap();
    assert_eq!(rows, ["u1", "a1", "t1"]);
    let stamp = Stamp::of(&std::fs::read(&transcript).unwrap());
    append(&folder, stamp, &[row("u2", Role::User, vec![text("more")])]).unwrap();
    let (rows, _) = super::super::session::written_rows(&folder).unwrap();
    assert_eq!(rows, ["u1", "a1", "t1", "u2"]);

    // A row that changes nothing the model sees is held all the same.
    let before = std::fs::read(&transcript).unwrap();
    append(&folder, Stamp::of(&before), &[row("r1", Role::Assistant, vec![text(" ")])]).unwrap();
    assert_eq!(std::fs::read(&transcript).unwrap(), before);
    let (rows, _) = super::super::session::written_rows(&folder).unwrap();
    assert_eq!(rows, ["u1", "a1", "t1", "u2", "r1"]);
}

/// A result appended after its call was written answers the call, in place of the note that
/// none was recorded.
#[rstest]
#[tokio::test]
async fn a_result_appended_after_its_call_answers_it() {
    use crate::harnesstools::sync::Stamp;

    let root = tempfile::tempdir().unwrap();
    let session = session(vec![
        row("u1", Role::User, vec![text("list it")]),
        row("a1", Role::Assistant, vec![call("c1", "ls")]),
    ]);
    let folder = rehydrate(root.path(), &session).await.unwrap();
    let transcript = folder.join("messages.jsonl");
    let stamp = Stamp::of(&std::fs::read(&transcript).unwrap());

    let more = [
        row("t1", Role::Tool, vec![result("c1", "a b")]),
        row("a2", Role::Assistant, vec![text("two files")]),
    ];
    append(&folder, stamp, &more).unwrap();
    let lines: Vec<Value> = std::fs::read_to_string(&transcript)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_the_api_takes(&lines);
    assert_eq!(lines[2], json!({ "role": "tool", "tool_call_id": "c1", "content": "a b" }));
    assert_eq!(lines.len(), 4);
}
