use std::collections::HashSet;
use std::num::NonZeroUsize;

use futures::TryStreamExt;
use rstest::{fixture, rstest};
use serde_json::json;
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::pi::session::{PiMessage, PiSession};
use crate::harnesstools::rehydrate::testing;
use crate::harnesstools::session::{Message, Session, SessionId};
use crate::sync::BlockingPool;

#[fixture]
fn sessions() -> TempDir {
    tempfile::tempdir().unwrap()
}

/// A captured row for each line of `jsonl`, as live capture makes them: the entry's id, or a
/// stand-in for one without (capture derives those from the line's content).
fn captured(jsonl: &str) -> Vec<RehydrateMessage> {
    jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<PiMessage>(l).expect("fixture line parses"))
        .enumerate()
        .map(|(n, m)| RehydrateMessage {
            source_id: m.id().map_or_else(|| format!("synthetic-{n}"), |id| id.to_string()),
            parent_source_id: m.parent_id().map(|p| p.to_string()),
            timestamp: m.timestamp().unwrap_or(OffsetDateTime::UNIX_EPOCH),
            role: m.role(),
            content: m.content(),
            model: m.model(),
            provider: None,
            usage: m.usage(),
            stop_reason: m.stop_reason(),
            turn_id: m.turn_id(),
            cwd: m.cwd(),
            git_branch: m.git_branch(),
        })
        .collect()
}

fn session(id: &str, cwd: &Path, messages: Vec<RehydrateMessage>) -> RehydrateSession {
    RehydrateSession {
        id: id.to_owned(),
        title: Some("restored".to_owned()),
        cwd: cwd.to_owned(),
        original_cwd: Some(PathBuf::from("/home/u/proj")),
        git_branch: None,
        model: None,
        started_at: OffsetDateTime::UNIX_EPOCH,
        messages,
        fork_of: None,
    }
}

/// What the file can carry of each row, by the rules in the module docs.
fn carried(messages: &[RehydrateMessage]) -> Vec<(String, Role, Vec<Content>)> {
    messages
        .iter()
        .filter(|m| match &m.role {
            Role::Other(_) => false,
            Role::System => m.content.iter().any(|c| matches!(c, Content::Summary(_))),
            _ => true,
        })
        .filter_map(|m| {
            let content: Vec<Content> = m
                .content
                .iter()
                .filter_map(|c| match c {
                    Content::Reasoning(_) | Content::ReasoningSummary { .. } => None,
                    Content::Text(t) if t.is_empty() && m.role != Role::Assistant => None,
                    // A v1 `!command` had no id to name its result after; it has now.
                    Content::ToolResult(r) if r.call.as_ref().starts_with("bash:") => {
                        Some(Content::ToolResult(ToolResult {
                            call: m.source_id.clone().into(),
                            ..r.clone()
                        }))
                    }
                    other => Some(other.clone()),
                })
                .collect();
            (!content.is_empty()).then(|| (m.source_id.clone(), m.role.clone(), content))
        })
        .collect()
}

async fn read_back(path: &Path) -> Vec<PiMessage> {
    let pool = BlockingPool::new(NonZeroUsize::MIN);
    PiSession::open(SessionId::from("s".to_owned()), path.to_owned(), pool)
        .read()
        .try_collect()
        .await
        .unwrap()
}

/// Every row the file can carry reads back under its own id, role and content (so a re-capture
/// dedups), the header names the session and the directory it resumes in, and the file is where
/// `locate` finds it.
#[rstest]
#[case::mock(include_str!("../../../../tests/fixtures/pi/session-mock-0.85.jsonl"))]
#[case::session1(include_str!("../../../../tests/fixtures/pi/session1.jsonl"))]
#[case::session2(include_str!("../../../../tests/fixtures/pi/session2.jsonl"))]
#[case::v1(include_str!("../../../../tests/fixtures/pi/session-v1.jsonl"))]
#[tokio::test]
async fn a_session_reads_back_as_it_was_captured(sessions: TempDir, #[case] jsonl: &str) {
    let cwd = sessions.path().join("here");
    let synced = testing::synced_as(captured(jsonl), true);
    let session = session("0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000", &cwd, synced);
    let dir = sessions.path().join("--here--");
    let path = rehydrate_into(Flavour::Pi, sessions.path(), &dir, &session, None).unwrap();
    assert_eq!(path.parent(), Some(dir.as_path()));
    assert_eq!(locate(sessions.path(), &session.id), Some(path.clone()));

    let read = read_back(&path).await;
    let header = &read[0];
    assert_eq!(header.id().map(|id| id.to_string()), Some(session.id.clone()));
    assert_eq!(header.cwd(), Some(cwd));
    let rows: Vec<_> = read[1..]
        .iter()
        .filter(|m| m.title().is_none())
        .map(|m| (m.id().unwrap().to_string(), m.role(), m.content()))
        .collect();
    assert_eq!(rows, carried(&session.messages));
    let title = read.iter().find_map(Message::title).and_then(|t| t.text);
    assert_eq!(title.as_deref(), Some("restored"));
}

fn message(id: &str, parent: Option<&str>, role: Role, content: Vec<Content>) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: parent.map(str::to_owned),
        timestamp: OffsetDateTime::UNIX_EPOCH,
        role,
        content,
        model: Some("claude-sonnet-5".to_owned()),
        provider: None,
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
    }
}

fn lines(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Thinking and settings entries are dropped and the tree relinked over them; tool results
/// name their tool; a compaction keeps everything since the one before.
#[rstest]
fn entries_are_linked_named_and_compacted(sessions: TempDir) {
    let call = crate::harnesstools::session::ToolUse {
        id: crate::harnesstools::session::ToolCallId::from("t1".to_owned()),
        name: "bash".to_owned(),
        input: json!({"command": "ls"}),
    };
    let session = session("s-tree", sessions.path(), vec![
        message("m0", None, Role::Other("model_change".to_owned()), vec![]),
        message("u1", Some("m0"), Role::User, vec![Content::Text("hi".to_owned())]),
        message("a1", Some("u1"), Role::Assistant, vec![
            Content::ReasoningSummary { tokens: None },
            Content::ToolUse(call),
        ]),
        message("r1", Some("a1"), Role::Tool, vec![Content::ToolResult(ToolResult {
            call: crate::harnesstools::session::ToolCallId::from("t1".to_owned()),
            output: json!([{"type": "text", "text": "a b"}]),
            error: false,
        })]),
        message("c1", Some("r1"), Role::System, vec![Content::Summary("so far".to_owned())]),
    ]);
    let path =
        rehydrate_into(Flavour::Pi, sessions.path(), sessions.path(), &session, None).unwrap();
    let lines = lines(&path);
    let by_id = |id: &str| lines.iter().find(|l| l["id"] == id).cloned().unwrap();

    assert!(lines.iter().all(|l| l["id"] != "m0"));
    assert_eq!(by_id("u1")["parentId"], serde_json::Value::Null);
    let reply = by_id("a1");
    assert_eq!(
        reply["message"]["content"],
        json!([{
            "type": "toolCall", "id": "t1", "name": "bash", "arguments": {"command": "ls"},
        }])
    );
    assert_eq!(reply["message"]["provider"], "anthropic");
    assert_eq!(by_id("r1")["message"]["toolName"], "bash");
    let compaction = by_id("c1");
    assert_eq!(compaction["type"], "compaction");
    assert_eq!(compaction["firstKeptEntryId"], "u1");
    // The title comes last, so the conversation stays on the path to pi's leaf.
    assert_eq!(lines.last().unwrap()["type"], "session_info");
    assert_eq!(lines.last().unwrap()["parentId"], "c1");
}

/// An entry hangs from its nearest written ancestor even when the rows came out of tree order.
#[rstest]
fn the_tree_is_relinked_whatever_the_order(sessions: TempDir) {
    let session = session("s-order", sessions.path(), vec![
        message("a0", Some("u1"), Role::Assistant, vec![Content::Text("early".to_owned())]),
        message("x1", Some("u1"), Role::Other("label".to_owned()), vec![]),
        message("u1", None, Role::User, vec![Content::Text("hi".to_owned())]),
        message("a1", Some("x1"), Role::Assistant, vec![Content::Text("hello".to_owned())]),
    ]);
    let path =
        rehydrate_into(Flavour::Pi, sessions.path(), sessions.path(), &session, None).unwrap();
    let lines = lines(&path);
    let parent = |id: &str| lines.iter().find(|l| l["id"] == id).unwrap()["parentId"].clone();
    assert_eq!(parent("a0"), "u1");
    assert_eq!(parent("u1"), serde_json::Value::Null);
    assert_eq!(parent("a1"), "u1");
}

/// A parent cycle (a corrupt session's) never makes an entry its own parent or one written after
/// it: it hangs from the entry before.
#[rstest]
fn a_parent_cycle_hangs_from_the_entry_before(sessions: TempDir) {
    let session = session("s-cycle", sessions.path(), vec![
        message("u1", None, Role::User, vec![Content::Text("hi".to_owned())]),
        message("a1", Some("x1"), Role::Assistant, vec![Content::Text("hello".to_owned())]),
        message("x1", Some("u2"), Role::Other("label".to_owned()), vec![]),
        message("u2", Some("a1"), Role::User, vec![Content::Text("again".to_owned())]),
    ]);
    let path =
        rehydrate_into(Flavour::Pi, sessions.path(), sessions.path(), &session, None).unwrap();
    let lines = lines(&path);
    let parent = |id: &str| lines.iter().find(|l| l["id"] == id).unwrap()["parentId"].clone();

    assert_eq!(parent("a1"), "u1");
    assert_eq!(parent("u2"), "a1");
}

/// A parent cycle (`c`, `d`) under an entry stamped before it (`x`) closes no cycle among the
/// written entries: it is cut where it starts.
#[rstest]
fn a_cycle_closed_by_clock_skew_is_cut(sessions: TempDir) {
    let session = session("s-skew", sessions.path(), vec![
        message("u1", None, Role::User, vec![Content::Text("hi".to_owned())]),
        message("x", Some("c"), Role::User, vec![Content::Text("skewed".to_owned())]),
        message("w", Some("x"), Role::Assistant, vec![Content::Text("reply".to_owned())]),
        message("c", Some("d"), Role::User, vec![Content::Text("cycle".to_owned())]),
        message("d", Some("c"), Role::Other("label".to_owned()), vec![]),
    ]);
    let path =
        rehydrate_into(Flavour::Pi, sessions.path(), sessions.path(), &session, None).unwrap();
    let lines = lines(&path);
    let parent = |id: &str| lines.iter().find(|l| l["id"] == id).unwrap()["parentId"].clone();

    assert_eq!(parent("c"), "w");
    assert_eq!(parent("w"), "x");
    assert_eq!(parent("x"), "u1");
}

/// A session is never written twice, wherever pi keeps it.
#[rstest]
fn never_overwrites_a_session(sessions: TempDir) {
    let messages = vec![message("u1", None, Role::User, vec![Content::Text("hi".to_owned())])];
    let s = session("s-once", sessions.path(), messages);
    let path =
        rehydrate_into(Flavour::Pi, sessions.path(), &sessions.path().join("a"), &s, None).unwrap();
    let err = rehydrate_into(Flavour::Pi, sessions.path(), &sessions.path().join("b"), &s, None)
        .unwrap_err();
    assert!(matches!(&err, RehydrateError::AlreadyExists(p) if *p == path), "{err:?}");
    assert!(!sessions.path().join("b").exists());
}

/// A `!command` goes back as the shell entry it was captured from, and omp's `$code` as its
/// Python one.
#[rstest]
#[case::bash(Flavour::Pi, "!!echo hi", "bashExecution", "command")]
#[case::python(Flavour::Omp, "$$echo hi", "pythonExecution", "code")]
fn a_bang_command_is_a_bash_execution(
    sessions: TempDir,
    #[case] flavour: Flavour,
    #[case] typed: &str,
    #[case] role: &str,
    #[case] key: &str,
) {
    let session = session("s-bash", sessions.path(), vec![message("b1", None, Role::User, vec![
        Content::Text(typed.to_owned()),
        Content::ToolResult(ToolResult {
            call: crate::harnesstools::session::ToolCallId::from("b1".to_owned()),
            output: json!("hi\n"),
            error: true,
        }),
    ])]);
    let path = rehydrate_into(flavour, sessions.path(), sessions.path(), &session, None).unwrap();
    let entry = lines(&path).into_iter().find(|e| e["type"] == "message").unwrap();
    assert_eq!(entry["message"]["role"], role);
    assert_eq!(entry["message"][key], "echo hi");
    assert_eq!(entry["message"]["excludeFromContext"], true);
    assert_eq!(entry["message"]["exitCode"], 1);
}

/// Checks a session file as strictly as the APIs behind pi check what pi-ai builds from it: an
/// assistant message's `toolCall`s have an object for their `arguments` (pi-ai sends a missing
/// one as `{}` to Anthropic and `"null"` to OpenAI) and are answered by the `toolResult`s right
/// after it, each result answers a call of the assistant message before it and says something,
/// and no two assistant messages pi sends come in a row (pi-ai sends each as a message of its
/// own; it leaves out the failed ones).
fn assert_the_api_takes(lines: &[serde_json::Value]) {
    let messages =
        lines.iter().filter(|l| l["type"] == "message").map(|l| &l["message"]).filter(|m| {
            m["role"] != "assistant"
                || !matches!(m["stopReason"].as_str(), Some("error" | "aborted"))
        });
    let mut calls: HashSet<String> = HashSet::new();
    let mut last_role = "";
    for m in messages {
        let role = m["role"].as_str().unwrap();
        match role {
            "assistant" => {
                assert!(calls.is_empty(), "calls {calls:?} not answered");
                assert_ne!(last_role, "assistant", "two assistant messages in a row: {m}");
                for block in m["content"].as_array().unwrap() {
                    if block["type"] == "toolCall" {
                        assert!(
                            block["arguments"].is_object(),
                            "a call without arguments: {block}"
                        );
                        calls.insert(block["id"].as_str().unwrap().to_owned());
                    }
                }
            }
            "toolResult" => {
                let id = m["toolCallId"].as_str().unwrap();
                assert!(calls.remove(id), "result {id} answers no call before it");
                assert!(m["content"].as_array().is_some_and(|c| !c.is_empty()), "{m}");
            }
            _ => assert!(calls.is_empty(), "calls {calls:?} not answered before a {role}"),
        }
        last_role = role;
    }
}

/// What capture syncs keeps no call's input and no output. Written back, each such call is a
/// note in its turn's text, which the APIs take; re-captured, every entry reads back under an id
/// already synced (so nothing is pushed): a turn's last message with its notes, the messages
/// merged into it and the results not at all.
#[rstest]
#[case::mock(include_str!("../../../../tests/fixtures/pi/session-mock-0.85.jsonl"))]
#[case::session1(include_str!("../../../../tests/fixtures/pi/session1.jsonl"))]
#[case::session2(include_str!("../../../../tests/fixtures/pi/session2.jsonl"))]
#[case::v1(include_str!("../../../../tests/fixtures/pi/session-v1.jsonl"))]
#[tokio::test]
async fn synced_calls_come_back_as_notes_the_api_takes(sessions: TempDir, #[case] jsonl: &str) {
    let mut synced = testing::synced(captured(jsonl));
    assert!(testing::uncaptured(&synced) > 0, "the fixture makes calls");
    // Some fixtures had their ids redacted: a chain of ids of their own stands in.
    if synced.iter().any(|m| m.source_id == "<redacted>") {
        for (n, m) in synced.iter_mut().enumerate() {
            m.source_id = format!("{n:08x}");
            m.parent_source_id = n.checked_sub(1).map(|p| format!("{p:08x}"));
        }
    }
    let session = session("0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000", sessions.path(), synced.clone());
    let path =
        rehydrate_into(Flavour::Pi, sessions.path(), sessions.path(), &session, None).unwrap();
    let written = lines(&path);
    assert_the_api_takes(&written);
    assert!(!written.iter().any(|l| l["message"]["content"].to_string().contains("toolCall")));

    let read = read_back(&path).await;
    let again: Vec<(String, Role, Vec<Content>)> = read[1..]
        .iter()
        .filter(|m| m.title().is_none())
        .map(|m| {
            let role = m.role();
            let content = testing::sanitize(&role, &m.content());
            (m.id().unwrap().to_string(), role, content)
        })
        .filter(|(_, _, content)| !content.is_empty())
        .collect();
    testing::assert_nothing_new(&synced, again.iter().map(|(id, ..)| id.as_str()));
    let flattened = merge_uncaptured_runs(&synced).0;
    assert_eq!(again, carried(&flattened));
}

/// Tool output capture never kept is written as saying so, as a result's and a `!command`'s.
#[rstest]
fn output_not_captured_says_so(sessions: TempDir) {
    let call = crate::harnesstools::session::ToolCallId::from("t1".to_owned());
    let session = session("s-output", sessions.path(), vec![
        message("b1", None, Role::User, vec![
            Content::Text("!ls".to_owned()),
            Content::ToolResult(ToolResult {
                call: "b1".to_owned().into(),
                output: serde_json::Value::Null,
                error: false,
            }),
        ]),
        message("a1", Some("b1"), Role::Assistant, vec![Content::ToolUse(
            crate::harnesstools::session::ToolUse {
                id: call.clone(),
                name: "bash".to_owned(),
                input: json!({"command": "ls"}),
            },
        )]),
        message("r1", Some("a1"), Role::Tool, vec![Content::ToolResult(ToolResult {
            call,
            output: serde_json::Value::Null,
            error: false,
        })]),
    ]);
    let path =
        rehydrate_into(Flavour::Pi, sessions.path(), sessions.path(), &session, None).unwrap();
    let lines = lines(&path);
    assert_the_api_takes(&lines);
    assert_eq!(lines[1]["message"]["output"], UNCAPTURED_OUTPUT);
    assert_eq!(
        lines[3]["message"]["content"],
        json!([{"type": "text", "text": UNCAPTURED_OUTPUT}])
    );
}

/// An `edit` result whose patch capture kept gets back the `details` pi shows its diff from,
/// and reads back as the same patch.
#[rstest]
fn an_edits_result_gets_its_details_back(sessions: TempDir) {
    use crate::harnesstools::session::{Change, FilePatch, Hunk, ToolCallId, ToolUse};

    let call = ToolCallId::from("t1".to_owned());
    let patch = Patch {
        call: call.clone(),
        files: vec![FilePatch {
            path: "src/a.ts".to_owned(),
            change: Change::Update,
            moved_to: None,
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 2,
                new_start: 1,
                new_lines: 2,
                lines: vec![" a".into(), "-b".into(), "+c".into()],
            }],
        }],
    };
    let session = session("s-edit", sessions.path(), vec![
        message("u1", None, Role::User, vec![Content::Text("edit it".to_owned())]),
        message("a1", Some("u1"), Role::Assistant, vec![Content::ToolUse(ToolUse {
            id: call.clone(),
            name: "edit".to_owned(),
            input: json!({"path": "src/a.ts", "edits": [{"oldText": "b", "newText": "c"}]}),
        })]),
        message("r1", Some("a1"), Role::Tool, vec![
            Content::ToolResult(ToolResult {
                call,
                output: json!([{"type": "text", "text": "Successfully replaced 1 block(s)."}]),
                error: false,
            }),
            Content::Patch(patch.clone()),
        ]),
    ]);
    let path =
        rehydrate_into(Flavour::Pi, sessions.path(), sessions.path(), &session, None).unwrap();
    let lines = lines(&path);
    let result = lines.iter().find(|l| l["id"] == "r1").unwrap();
    assert_eq!(
        result["message"]["details"],
        json!({"diff": " 1 a\n-2 b\n+2 c", "patch": "--- src/a.ts\n+++ src/a.ts\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n", "firstChangedLine": 2})
    );
    let read: PiMessage = serde_json::from_value(result.clone()).unwrap();
    assert!(read.content().contains(&Content::Patch(patch)));
}

/// A reply names the provider capture kept (omp's), else one guessed from its model, as rows
/// captured before providers were do.
#[rstest]
#[case::captured("gpt-5.6-luna", Some("openai-codex"), Some("openai-codex"))]
#[case::guessed("gpt-5.6-luna", None, Some("openai"))]
#[case::unknown("llama-4", None, None)]
fn a_reply_names_its_provider(
    sessions: TempDir,
    #[case] model: &str,
    #[case] provider: Option<&str>,
    #[case] want: Option<&str>,
) {
    let mut reply = message("a1", None, Role::Assistant, vec![Content::Text("hi".to_owned())]);
    reply.model = Some(model.to_owned());
    reply.provider = provider.map(str::to_owned);
    let session = session("s-provider", sessions.path(), vec![reply]);
    let path =
        rehydrate_into(Flavour::Pi, sessions.path(), sessions.path(), &session, None).unwrap();
    let reply = lines(&path).into_iter().find(|l| l["id"] == "a1").unwrap();
    assert_eq!(reply["message"]["provider"].as_str(), want);
}

/// A restored omp session starts with omp's 256-byte title slot, so it reads back as omp's where
/// pi and omp share a session directory; a pi one has none.
#[rstest]
fn an_omp_session_starts_with_its_title_slot(
    sessions: TempDir,
    #[values(Flavour::Pi, Flavour::Omp)] flavour: Flavour,
) {
    let session = session("s-slot", sessions.path(), vec![message("u1", None, Role::User, vec![
        Content::Text("hi".to_owned()),
    ])]);
    let path = rehydrate_into(flavour, sessions.path(), sessions.path(), &session, None).unwrap();
    let body = std::fs::read_to_string(&path).unwrap();
    let first = body.split_inclusive('\n').next().unwrap();
    let first_type = serde_json::from_str::<serde_json::Value>(first).unwrap()["type"].clone();
    match flavour {
        Flavour::Pi => assert_eq!(first_type, "session"),
        Flavour::Omp => {
            assert_eq!((first_type.as_str(), first.len()), (Some("title"), 256));
            assert_eq!(locate(sessions.path(), "s-slot"), Some(path));
        }
    }
}

/// A new file opens on the agent's configured default model, in the agent's own `model_change`
/// shape, and the first entry hangs from it: omp won't open a session whose turns name no model
/// it can use unless a `model_change` on the path does.
#[rstest]
fn a_new_file_opens_on_the_agents_default_model(
    sessions: TempDir,
    #[values(Flavour::Pi, Flavour::Omp)] flavour: Flavour,
) {
    let agent = tempfile::tempdir().unwrap();
    match flavour {
        Flavour::Pi => std::fs::write(
            agent.path().join("settings.json"),
            r#"{"defaultProvider": "openai-codex", "defaultModel": "gpt-5.6-luna"}"#,
        )
        .unwrap(),
        Flavour::Omp => std::fs::write(
            agent.path().join("config.yml"),
            "setupVersion: 2\nmodelRoles:\n  default: openai-codex/gpt-5.6-luna\n  smol: x/y\n",
        )
        .unwrap(),
    }
    let start = StartModel::configured_in(flavour, agent.path()).unwrap();
    assert_eq!((start.provider.as_str(), start.id.as_str()), ("openai-codex", "gpt-5.6-luna"));

    let session = session("s-start", sessions.path(), vec![
        message("u1", None, Role::User, vec![Content::Text("hi".to_owned())]),
        message("a1", Some("u1"), Role::Assistant, vec![Content::Text("hello".to_owned())]),
    ]);
    let path =
        rehydrate_into(flavour, sessions.path(), sessions.path(), &session, Some(&start)).unwrap();
    let lines = lines(&path);
    let opening = lines.iter().find(|l| l["type"] == "model_change").unwrap();
    assert_eq!(opening["parentId"], serde_json::Value::Null);
    match flavour {
        Flavour::Pi => assert_eq!(
            (opening["provider"].as_str(), opening["modelId"].as_str()),
            (Some("openai-codex"), Some("gpt-5.6-luna"))
        ),
        Flavour::Omp => assert_eq!(opening["model"].as_str(), Some("openai-codex/gpt-5.6-luna")),
    }
    let first = lines.iter().find(|l| l["id"] == "u1").unwrap();
    assert_eq!(first["parentId"], opening["id"]);
    assert_eq!(lines.iter().find(|l| l["id"] == "a1").unwrap()["parentId"], "u1");
}

/// Without a configured default there is nothing to open on, and rows appended to a file never
/// open it again.
#[rstest]
fn no_opening_model_without_a_default_or_on_an_append(sessions: TempDir) {
    let agent = tempfile::tempdir().unwrap();
    assert_eq!(StartModel::configured_in(Flavour::Pi, agent.path()), None);
    assert_eq!(StartModel::configured_in(Flavour::Omp, agent.path()), None);
    std::fs::write(agent.path().join("settings.json"), r#"{"defaultProvider": "openai"}"#).unwrap();
    assert_eq!(StartModel::configured_in(Flavour::Pi, agent.path()), None);

    let start = StartModel {
        flavour: Flavour::Pi,
        provider: "openai".to_owned(),
        id: "gpt-5.6-luna".to_owned(),
    };
    let session =
        session("s-append", sessions.path(), vec![message("u2", Some("u1"), Role::User, vec![
            Content::Text("more".to_owned()),
        ])]);
    let appended = super::lines(
        &session,
        &Continuing {
            after: Some("u1"),
            kept_from: None,
        },
        false,
        Some(&start),
    );
    assert!(appended.iter().all(|l| l["type"] != "model_change"), "{appended:?}");
    assert_eq!(appended[0]["parentId"], "u1");
}

#[rstest]
#[case::plain("modelRoles:\n  default: a/b\n", Some("a/b"))]
#[case::quoted("modelRoles:\n  default: \"a/b\"\n", Some("a/b"))]
#[case::comment("modelRoles:\n  default: a/b # the one\n", Some("a/b"))]
#[case::other_block("retry:\n  default: a/b\nmodelRoles:\n  smol: c/d\n", None)]
#[case::missing("setupVersion: 2\n", None)]
fn omp_config_names_its_default_model(#[case] yaml: &str, #[case] want: Option<&str>) {
    assert_eq!(yaml_model_role_default(yaml).as_deref(), want);
}
