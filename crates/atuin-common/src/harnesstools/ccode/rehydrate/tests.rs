use std::collections::HashSet;
use std::num::NonZeroUsize;

use futures::TryStreamExt;
use rstest::{fixture, rstest};
use serde_json::json;
use tempfile::TempDir;
use time::OffsetDateTime;

use super::*;
use crate::harnesstools::ccode::session::{CcodeMessage, CcodeSession};
use crate::harnesstools::rehydrate::{is_uncaptured, testing};
use crate::harnesstools::session::{Message, Session, SessionId, ToolCallId, ToolUse};
use crate::sync::BlockingPool;

/// A projects directory of its own, removed on drop.
#[fixture]
fn projects() -> TempDir {
    tempfile::tempdir().unwrap()
}

/// A captured row for each line of `jsonl` with an id, as live capture makes them.
fn captured(jsonl: &str) -> Vec<RehydrateMessage> {
    jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<CcodeMessage>(l).expect("fixture line parses"))
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
        .collect()
}

fn session(id: &str, cwd: &Path, messages: Vec<RehydrateMessage>) -> RehydrateSession {
    RehydrateSession {
        id: id.to_owned(),
        title: Some("a restored session".to_owned()),
        cwd: cwd.to_owned(),
        original_cwd: Some(PathBuf::from("/elsewhere/proj")),
        git_branch: Some("main".to_owned()),
        model: None,
        started_at: OffsetDateTime::UNIX_EPOCH,
        messages,
        fork_of: None,
    }
}

/// What the transcript can carry of each row, by the rules in the module docs: `None` for a row
/// with nothing left.
fn carried(messages: &[RehydrateMessage]) -> Vec<(String, Role, Vec<Content>)> {
    let server: HashSet<&ToolCallId> = messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .flat_map(|m| &m.content)
        .filter_map(|c| match c {
            Content::ToolResult(r) => Some(&r.call),
            _ => None,
        })
        .collect();
    messages
        .iter()
        .filter(|m| !matches!(m.role, Role::Other(_)))
        .filter_map(|m| {
            let content: Vec<Content> = m
                .content
                .iter()
                .filter_map(|c| match c {
                    Content::Text(t) if t.is_empty() => None,
                    Content::Reasoning(_) | Content::ReasoningSummary { .. } => None,
                    Content::ToolUse(u) if server.contains(&u.id) => None,
                    Content::ToolResult(_) if m.role == Role::Assistant => None,
                    // Capture keeps no raw block.
                    Content::Other(_) => None,
                    other => Some(other.clone()),
                })
                .collect();
            let replied = content.iter().any(|c| !matches!(c, Content::Error(_)));
            let content = if m.role == Role::Assistant && replied {
                content.into_iter().filter(|c| !matches!(c, Content::Error(_))).collect()
            } else {
                content
            };
            (!content.is_empty()).then(|| (m.source_id.clone(), m.role.clone(), content))
        })
        .collect()
}

/// The transcript at `path`, read the way live capture reads it.
async fn read_back(id: &str, path: &Path) -> Vec<CcodeMessage> {
    let pool = BlockingPool::new(NonZeroUsize::MIN);
    CcodeSession::open(SessionId::from(id.to_owned()), path.to_owned(), pool)
        .read()
        .try_collect()
        .await
        .unwrap()
}

fn rows(messages: &[CcodeMessage]) -> Vec<(String, Role, Vec<Content>)> {
    messages.iter().filter_map(|m| Some((m.id()?.to_string(), m.role(), m.content()))).collect()
}

/// Every row the transcript can carry reads back from it under its own id, role and content,
/// so re-capturing the file dedups against the rows it came from; and it's in the project of
/// the directory it resumes in, under the session's id.
#[rstest]
#[case::session1(include_str!("../../../../tests/fixtures/ccode/session1.jsonl"))]
#[case::session2(include_str!("../../../../tests/fixtures/ccode/session2.jsonl"))]
#[case::session3(include_str!("../../../../tests/fixtures/ccode/session3.jsonl"))]
#[case::compacted(include_str!("../../../../tests/fixtures/ccode/session4.jsonl"))]
#[tokio::test]
async fn a_session_reads_back_as_it_was_captured(projects: TempDir, #[case] jsonl: &str) {
    let messages = captured(jsonl);
    let cwd = projects.path().join("my proj");
    let session = session("5d1f0b1e-4a8e-4f1b-9d59-2c0f2f3f0a11", &cwd, messages);

    let path = rehydrate_into(projects.path(), &session).unwrap();
    assert_eq!(
        path,
        projects.path().join(project_dir_name(&cwd)).join(format!("{}.jsonl", session.id))
    );
    assert_eq!(locate(projects.path(), &session.id), Some(path.clone()));

    let read = read_back(&session.id, &path).await;
    assert_eq!(rows(&read), carried(&session.messages));
    assert!(read.iter().all(|m| m.parent_session().is_none_or(|s| s.as_ref() == session.id)));
    let title = read.iter().find_map(Message::title).and_then(|t| t.text);
    assert_eq!(title.as_deref(), Some("a restored session"));
}

/// Checks a transcript as strictly as the Messages API checks the request Claude Code builds
/// from it: Claude Code sends the lines of one message (`message.id`) as one, and consecutive
/// user lines as one, so two assistant lines in a row must be the same message; every
/// `tool_use` has an object for its `input` and is answered, in the user message right after it,
/// by a `tool_result` of its id; and every `tool_result` answers a `tool_use` of the assistant
/// message right before it.
pub fn assert_the_api_takes(lines: &[serde_json::Value]) {
    use serde_json::Value;
    let mut messages: Vec<(&str, Vec<Value>)> = Vec::new();
    let mut last_id: Option<&str> = None;
    for line in lines {
        let Some(role @ ("user" | "assistant")) = line["type"].as_str() else {
            continue;
        };
        if line["isApiErrorMessage"] == true {
            continue;
        }
        let blocks = match &line["message"]["content"] {
            Value::String(text) => vec![json!({"type": "text", "text": text})],
            Value::Array(blocks) => blocks.clone(),
            other => panic!("content of a line is {other}"),
        };
        let id = line["message"]["id"].as_str();
        match messages.last_mut() {
            Some((last, merged)) if *last == role => {
                assert!(
                    role == "user" || id == last_id,
                    "two assistant messages in a row: {last_id:?}, then {id:?}"
                );
                merged.extend(blocks);
            }
            _ => messages.push((role, blocks)),
        }
        if role == "assistant" {
            last_id = id;
        }
    }
    let ids = |blocks: &[Value], kind: &str, key: &str| -> HashSet<String> {
        blocks
            .iter()
            .filter(|b| b["type"] == kind)
            .map(|b| b[key].as_str().expect("a block names its call").to_owned())
            .collect()
    };
    for (n, (role, blocks)) in messages.iter().enumerate() {
        for block in blocks.iter().filter(|b| b["type"] == "tool_use") {
            assert!(block["input"].is_object(), "a tool_use without an object input: {block}");
        }
        let uses = ids(blocks, "tool_use", "id");
        if !uses.is_empty() {
            let answered = messages
                .get(n + 1)
                .map(|(_, next)| ids(next, "tool_result", "tool_use_id"))
                .unwrap_or_default();
            assert!(uses.is_subset(&answered), "calls {uses:?} not answered by {answered:?}");
        }
        let results = ids(blocks, "tool_result", "tool_use_id");
        if !results.is_empty() {
            assert_eq!(*role, "user");
            let asked =
                n.checked_sub(1).map(|p| ids(&messages[p].1, "tool_use", "id")).unwrap_or_default();
            assert!(results.is_subset(&asked), "results {results:?} answer no call of {asked:?}");
        }
    }
}

/// What capture syncs with `ai.capture_tools` keeps every call's input and output: written back,
/// each is a call and its result again, which the API takes. (Not session2: its call ids were
/// redacted alike, so its results can't be told apart.)
#[rstest]
#[case::session1(include_str!("../../../../tests/fixtures/ccode/session1.jsonl"))]
#[case::session3(include_str!("../../../../tests/fixtures/ccode/session3.jsonl"))]
#[case::compacted(include_str!("../../../../tests/fixtures/ccode/session4.jsonl"))]
fn calls_synced_with_their_payloads_come_back_as_calls_the_api_takes(
    projects: TempDir,
    #[case] jsonl: &str,
) {
    let mut messages = captured(jsonl);
    // The fixtures' message ids were redacted: each run of assistant lines gets one, as Claude
    // Code writes the lines of one response.
    let mut turn = None;
    for m in &mut messages {
        if m.role != Role::Assistant {
            turn = None;
        } else if m.turn_id.as_deref() == Some("<redacted>") {
            m.turn_id = Some(turn.get_or_insert_with(|| format!("msg_{}", m.source_id)).clone());
        }
    }
    let session = session("5d1f0b1e-4a8e-4f1b-9d59-2c0f2f3f0a11", projects.path(), messages);
    let written = lines(&rehydrate_into(projects.path(), &session).unwrap());
    assert_the_api_takes(&written);
    let blocks = |kind: &str| {
        written
            .iter()
            .filter_map(|l| l["message"]["content"].as_array())
            .flatten()
            .filter(|b| b["type"] == kind)
            .count()
    };
    assert!(blocks("tool_use") > 0 && blocks("tool_result") > 0, "the calls stay calls");
    assert!(!written.iter().any(|l| l.to_string().contains(UNCAPTURED_OUTPUT)));
}

/// What capture syncs keeps no call's input and no output. Written back, each such call is a
/// note in its turn's text, which the API takes; re-captured, every line reads back under an id
/// already synced (so nothing is pushed): a turn's first line with its notes, the lines merged
/// into it and the results not at all.
#[rstest]
#[case::session1(include_str!("../../../../tests/fixtures/ccode/session1.jsonl"))]
#[case::session2(include_str!("../../../../tests/fixtures/ccode/session2.jsonl"))]
#[case::session3(include_str!("../../../../tests/fixtures/ccode/session3.jsonl"))]
#[case::compacted(include_str!("../../../../tests/fixtures/ccode/session4.jsonl"))]
#[tokio::test]
async fn synced_calls_come_back_as_notes_the_api_takes(projects: TempDir, #[case] jsonl: &str) {
    let mut synced = testing::synced(captured(jsonl));
    assert!(testing::uncaptured(&synced) > 0, "the fixture makes calls");
    // The fixtures' message ids were redacted, which would make every assistant line one
    // message: each row's own stands in, so none is taken for another's.
    for m in synced.iter_mut().filter(|m| m.turn_id.as_deref() == Some("<redacted>")) {
        m.turn_id = Some(format!("msg_{}", m.source_id));
    }
    let session = session("5d1f0b1e-4a8e-4f1b-9d59-2c0f2f3f0a11", projects.path(), synced.clone());
    let path = rehydrate_into(projects.path(), &session).unwrap();
    let written = lines(&path);
    assert_the_api_takes(&written);
    assert!(written.iter().any(|l| l["message"]["content"].to_string().contains("[ran ")));

    let read = read_back(&session.id, &path).await;
    let again: Vec<(String, Role, Vec<Content>)> = rows(&read)
        .into_iter()
        .map(|(id, role, content)| {
            let content = testing::sanitize(&role, &content);
            (id, role, content)
        })
        .filter(|(_, _, content)| !content.is_empty())
        .collect();
    testing::assert_nothing_new(&synced, again.iter().map(|(id, ..)| id.as_str()));
    let flattened = flatten_uncaptured_calls(&synced, &Flatten::Runs);
    assert_eq!(again, carried(&flattened));
    for row in synced.iter().filter(|m| m.content.iter().any(is_uncaptured)) {
        if let Some((_, _, content)) = again.iter().find(|(id, ..)| *id == row.source_id) {
            assert!(content.iter().all(|c| matches!(c, Content::Text(_))), "{content:?}");
        }
    }
}

/// Usage, models, turns and stop reasons survive, so a re-capture counts each call as before.
#[rstest]
#[tokio::test]
async fn calls_keep_their_turn_usage_and_model(projects: TempDir) {
    let messages = captured(include_str!("../../../../tests/fixtures/ccode/session4.jsonl"));
    let session = session("s-calls", projects.path(), messages);
    let path = rehydrate_into(projects.path(), &session).unwrap();
    let read = read_back(&session.id, &path).await;

    type Call = (Option<String>, Option<String>, Option<Usage>, Option<StopReason>);
    let calls =
        |ms: Vec<Call>| ms.into_iter().filter(|(turn, ..)| turn.is_some()).collect::<Vec<_>>();
    let before = calls(
        session
            .messages
            .iter()
            .filter(|m| read.iter().any(|r| r.id().is_some_and(|id| id.as_ref() == m.source_id)))
            .map(|m| (m.turn_id.clone(), m.model.clone(), m.usage, m.stop_reason.clone()))
            .collect(),
    );
    let after =
        calls(read.iter().map(|m| (m.turn_id(), m.model(), m.usage(), m.stop_reason())).collect());
    assert!(!before.is_empty());
    assert_eq!(after, before);
}

fn message(id: &str, parent: Option<&str>, role: Role, content: Vec<Content>) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: parent.map(str::to_owned),
        timestamp: OffsetDateTime::UNIX_EPOCH,
        role,
        content,
        model: None,
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
    }
}

pub fn lines(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Thinking is never written back (capture has no text or signature to replay), nor are server
/// tool calls, and the lines under a skipped row hang from its nearest written ancestor.
#[rstest]
fn thinking_and_server_tools_are_dropped_and_the_tree_relinked(projects: TempDir) {
    let search = ToolUse {
        id: ToolCallId::from("srvtoolu_1".to_owned()),
        name: "web_search".to_owned(),
        input: json!({"query": "q"}),
    };
    let session = session("s-drop", projects.path(), vec![
        message("u1", None, Role::User, vec![Content::Text("hi".to_owned())]),
        message("a1", Some("u1"), Role::Assistant, vec![Content::ReasoningSummary {
            tokens: Some(9),
        }]),
        message("a2", Some("a1"), Role::Assistant, vec![
            Content::ToolUse(search),
            Content::ToolResult(ToolResult {
                call: ToolCallId::from("srvtoolu_1".to_owned()),
                output: json!([]),
                error: false,
            }),
            Content::Text("found it".to_owned()),
        ]),
        message("x1", Some("a2"), Role::Other("attachment".to_owned()), vec![]),
        message("u2", Some("x1"), Role::User, vec![Content::Text("and now?".to_owned())]),
    ]);
    let path = rehydrate_into(projects.path(), &session).unwrap();
    let lines = lines(&path);
    let by_uuid = |id: &str| lines.iter().find(|l| l["uuid"] == id).cloned();

    assert!(by_uuid("a1").is_none() && by_uuid("x1").is_none());
    let reply = by_uuid("a2").unwrap();
    assert_eq!(reply["parentUuid"], "u1");
    assert_eq!(reply["message"]["content"], json!([{"type": "text", "text": "found it"}]));
    let prompt = by_uuid("u2").unwrap();
    assert_eq!(prompt["parentUuid"], "a2");
    assert_eq!(prompt["message"]["content"], "and now?");
    assert!(!std::fs::read_to_string(&path).unwrap().contains("thinking"));
}

/// A line hangs from its nearest written ancestor even when the rows came in another order than
/// the tree's (capture orders by time, and a hook's attachment can be stamped before the prompt
/// it hangs from).
#[rstest]
fn the_tree_is_relinked_whatever_the_order(projects: TempDir) {
    let session = session("s-order", projects.path(), vec![
        message("a0", None, Role::Assistant, vec![Content::Text("hello".to_owned())]),
        message("a9", Some("u1"), Role::Assistant, vec![Content::Text("early".to_owned())]),
        message("x1", Some("u1"), Role::Other("attachment".to_owned()), vec![]),
        message("u1", Some("a0"), Role::User, vec![Content::Text("hi".to_owned())]),
        message("a1", Some("x1"), Role::Assistant, vec![Content::Text("again".to_owned())]),
        message("a2", Some("never-synced"), Role::Assistant, vec![Content::Text("x".to_owned())]),
    ]);
    let path = rehydrate_into(projects.path(), &session).unwrap();
    let lines = lines(&path);
    let parent = |id: &str| lines.iter().find(|l| l["uuid"] == id).unwrap()["parentUuid"].clone();
    assert_eq!(parent("a0"), serde_json::Value::Null);
    assert_eq!(parent("a9"), "u1");
    assert_eq!(parent("u1"), "a0");
    assert_eq!(parent("a1"), "u1");
    assert_eq!(parent("a2"), "a1", "a parent never synced stands for the line before");
}

/// A compaction is written as Claude Code writes one: a boundary starting a new tree, whose
/// logical parent is the line before it, then the summary under it.
#[rstest]
fn a_compaction_starts_a_new_tree(projects: TempDir) {
    let session = session("s-compact", projects.path(), vec![
        message("u1", None, Role::User, vec![Content::Text("hi".to_owned())]),
        message("b1", Some("u1"), Role::System, vec![Content::Text(
            "Conversation compacted".to_owned(),
        )]),
        message("s1", Some("b1"), Role::System, vec![Content::Summary("we said hi".to_owned())]),
        message("u2", Some("s1"), Role::User, vec![Content::Text("again".to_owned())]),
    ]);
    let path = rehydrate_into(projects.path(), &session).unwrap();
    let lines = lines(&path);
    assert_eq!(lines[1]["subtype"], "compact_boundary");
    assert_eq!(lines[1]["parentUuid"], serde_json::Value::Null);
    assert_eq!(lines[1]["logicalParentUuid"], "u1");
    assert_eq!(lines[2]["isCompactSummary"], true);
    assert_eq!(lines[2]["parentUuid"], "b1");
}

/// A compaction synced as a parent cycle: the boundary's parent came from `logicalParentUuid`,
/// and newer Claude Code writes attachments between it and the summary. Every line still hangs
/// from one written before it, the boundary is still one, and the leaf reaches the first prompt.
#[rstest]
fn a_compaction_synced_as_a_cycle_keeps_its_history(projects: TempDir) {
    let attachment = || Role::Other("attachment".to_owned());
    let session = session("s-cycle", projects.path(), vec![
        message("u1", None, Role::User, vec![Content::Text("hi".to_owned())]),
        message("a1", Some("u1"), Role::Assistant, vec![Content::Text("hello".to_owned())]),
        message("590a", Some("3a06"), attachment(), vec![]),
        // Capture drops a boundary's text.
        message("8750", Some("590a"), Role::System, vec![]),
        message("b118", Some("8750"), attachment(), vec![]),
        message("ec2c", Some("b118"), attachment(), vec![]),
        message("3a06", Some("ec2c"), Role::System, vec![Content::Summary(
            "we said hi".to_owned(),
        )]),
        message("u2", Some("3a06"), Role::User, vec![Content::Text("again".to_owned())]),
        message("a2", Some("u2"), Role::Assistant, vec![Content::Text("still here".to_owned())]),
    ]);
    let path = rehydrate_into(projects.path(), &session).unwrap();
    let lines: Vec<_> = lines(&path).into_iter().filter(|l| l["uuid"].is_string()).collect();
    let up = |l: &serde_json::Value| {
        Some(&l["parentUuid"]).filter(|p| !p.is_null()).or(l.get("logicalParentUuid")).cloned()
    };

    for (i, line) in lines.iter().enumerate() {
        let Some(parent) = up(line).filter(|p| !p.is_null()) else {
            continue;
        };
        assert!(
            lines[..i].iter().any(|l| l["uuid"] == parent),
            "{line} hangs from no line before it"
        );
    }

    let boundary = lines.iter().find(|l| l["uuid"] == "8750").unwrap();
    assert_eq!(boundary["subtype"], "compact_boundary");
    assert_eq!(boundary["parentUuid"], serde_json::Value::Null);
    assert_eq!(boundary["logicalParentUuid"], "a1");

    let mut chain = Vec::new();
    let mut at = lines.last().cloned();
    while let Some(line) = at {
        chain.push(line["uuid"].as_str().unwrap().to_owned());
        at = up(&line).and_then(|p| lines.iter().find(|l| l["uuid"] == p).cloned());
    }
    assert_eq!(chain, ["a2", "u2", "3a06", "8750", "a1", "u1"]);
}

/// A parent cycle (`c`, `d`) under a line stamped before it (`x`): `c` hangs from the line before
/// it (`w`), which is below `x`, closing a cycle among written lines. It is cut where it starts.
#[rstest]
fn a_cycle_closed_by_clock_skew_is_cut(projects: TempDir) {
    let session = session("s-skew", projects.path(), vec![
        message("u1", None, Role::User, vec![Content::Text("hi".to_owned())]),
        message("x", Some("c"), Role::User, vec![Content::Text("skewed".to_owned())]),
        message("w", Some("x"), Role::Assistant, vec![Content::Text("reply".to_owned())]),
        message("c", Some("d"), Role::User, vec![Content::Text("cycle".to_owned())]),
        message("d", Some("c"), Role::Other("attachment".to_owned()), vec![]),
    ]);
    let path = rehydrate_into(projects.path(), &session).unwrap();
    let lines = lines(&path);
    let parent = |id: &str| lines.iter().find(|l| l["uuid"] == id).unwrap()["parentUuid"].clone();

    assert_eq!(parent("c"), "w");
    assert_eq!(parent("w"), "x");
    assert_eq!(parent("x"), "u1");
}

/// Past attachments, only a system line capture kept no text of is a boundary: one with text
/// (a hook's, say) is written as itself, and the summary hangs from it.
#[rstest]
fn a_system_line_with_text_above_attachments_is_no_boundary(projects: TempDir) {
    let session = session("s-hook", projects.path(), vec![
        message("u1", None, Role::User, vec![Content::Text("hi".to_owned())]),
        message("h1", Some("u1"), Role::System, vec![Content::Text("hook output".to_owned())]),
        message("x1", Some("h1"), Role::Other("attachment".to_owned()), vec![]),
        message("s1", Some("x1"), Role::System, vec![Content::Summary("we said hi".to_owned())]),
    ]);
    let path = rehydrate_into(projects.path(), &session).unwrap();
    let lines = lines(&path);
    let by_uuid = |id: &str| lines.iter().find(|l| l["uuid"] == id).unwrap().clone();

    assert!(lines.iter().all(|l| l["subtype"] != "compact_boundary"));
    assert_eq!(by_uuid("h1")["parentUuid"], "u1");
    assert_eq!(by_uuid("h1")["isMeta"], true);
    assert_eq!(by_uuid("s1")["parentUuid"], "h1");
}

/// Each line's `cwd` is where the session resumes, or the same subdirectory of it, never with a
/// trailing separator (a line recorded in the original directory itself once came out as
/// `…/dir/`, and so did a session resumed in a `$PWD` ending in one).
#[rstest]
#[case::same_directory("/elsewhere/proj", "")]
#[case::subdirectory("/elsewhere/proj/sub", "sub")]
#[case::elsewhere("/unrelated", "")]
fn line_directories_have_no_trailing_separator(
    projects: TempDir,
    #[case] recorded: &str,
    #[case] expected_sub: &str,
    #[values(false, true)] pwd_with_slash: bool,
) {
    std::fs::create_dir_all(projects.path().join("here/sub")).unwrap();
    let here = projects.path().join("here");
    let mut resumed_in = here.clone().into_os_string();
    if pwd_with_slash {
        resumed_in.push("/");
    }
    let mut m = message("u1", None, Role::User, vec![Content::Text("hi".to_owned())]);
    m.cwd = Some(PathBuf::from(recorded));
    let session = session("s-cwd", Path::new(&resumed_in), vec![m]);
    let path = rehydrate_into(projects.path(), &session).unwrap();
    let expected = if expected_sub.is_empty() {
        here
    } else {
        here.join(expected_sub)
    };
    assert_eq!(lines(&path)[0]["cwd"], expected.to_string_lossy().as_ref());
}

/// A transcript is never replaced, wherever Claude Code keeps it.
#[rstest]
fn never_overwrites_a_transcript(projects: TempDir) {
    let messages = vec![message("u1", None, Role::User, vec![Content::Text("hi".to_owned())])];
    let here = session("s-once", &projects.path().join("here"), messages.clone());
    let path = rehydrate_into(projects.path(), &here).unwrap();
    let before = std::fs::read(&path).unwrap();

    let err = rehydrate_into(projects.path(), &here).unwrap_err();
    assert!(matches!(&err, RehydrateError::AlreadyExists(p) if *p == path), "{err:?}");
    // Nor is a second copy written into another project, which `claude --resume` would refuse.
    let there = session("s-once", &projects.path().join("there"), messages);
    assert!(matches!(
        rehydrate_into(projects.path(), &there),
        Err(RehydrateError::AlreadyExists(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(projects.path()).unwrap().count(), 1);
}

#[rstest]
#[case::root("/work", "-work")]
#[case::punctuation("/home/u/my project.x/ü😀", "-home-u-my-project-x----")]
#[case::trailing_separator("/work/atuin/", "-work-atuin")]
fn project_directories_are_named_as_claude_code_names_them(
    #[case] cwd: &str,
    #[case] expected: &str,
) {
    assert_eq!(project_dir_name(Path::new(cwd)), expected);
}

/// A name past 200 characters is cut short and suffixed with a hash of the path, matching Claude
/// Code's (computed with its own function, in node). The hash is of the path as the platform
/// writes it: on Windows, with `\` between its components.
#[rstest]
fn long_project_directories_are_shortened_with_a_hash() {
    let cwd = format!("/home/u/{}proj", "deep/".repeat(45));
    let name = project_dir_name(Path::new(&cwd));
    assert!(name.starts_with("-home-u-deep-deep-"), "{name}");
    let (shortened, hash) = name.rsplit_once('-').unwrap();
    assert_eq!(shortened.len(), 200, "{name}");
    assert!(shortened.ends_with("ep-de"), "{name}");
    #[cfg(unix)]
    assert_eq!(hash, "szbxz9");
    #[cfg(windows)]
    assert_eq!(hash, base36(js_string_hash(&cwd.replace('/', "\\")).unsigned_abs()));
}

#[rstest]
#[case::bad_id("../escape")]
#[case::empty("")]
fn refuses_ids_that_are_not_file_names(projects: TempDir, #[case] id: &str) {
    let s = session(id, projects.path(), Vec::new());
    assert!(matches!(rehydrate_into(projects.path(), &s), Err(RehydrateError::Other(_))));
}

/// An edit's or write's result whose patch capture kept is written with the record Claude Code
/// shows its diff from, the call's own strings filled in; one without a patch keeps its text.
#[rstest]
#[case::edit(
    "Edit",
    json!({"file_path": "/w/a.rs", "old_string": "b", "new_string": "c"}),
    Change::Update,
    json!({"filePath": "/w/a.rs", "oldString": "b", "newString": "c", "originalFile": null,
        "structuredPatch": [{"oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
            "lines": [" a", "-b", "+c"]}],
        "userModified": false, "replaceAll": false})
)]
#[case::write(
    "Write",
    json!({"file_path": "/w/a.rs", "content": "a\nc\n"}),
    Change::Update,
    json!({"type": "update", "filePath": "/w/a.rs", "content": "a\nc\n",
        "structuredPatch": [{"oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
            "lines": [" a", "-b", "+c"]}],
        "originalFile": null})
)]
#[case::multi_edit(
    "MultiEdit",
    json!({"file_path": "/w/a.rs", "edits": [{"old_string": "b", "new_string": "c"}]}),
    Change::Update,
    json!({"filePath": "/w/a.rs", "edits": [{"old_string": "b", "new_string": "c"}],
        "originalFileContents": null,
        "structuredPatch": [{"oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
            "lines": [" a", "-b", "+c"]}],
        "userModified": false})
)]
#[case::created(
    "Write",
    json!({"file_path": "/w/a.rs", "content": "a\n"}),
    Change::Add,
    json!({"type": "create", "filePath": "/w/a.rs", "content": "a\n", "structuredPatch": [],
        "originalFile": null})
)]
fn an_edits_result_gets_its_record_back(
    projects: TempDir,
    #[case] tool: &str,
    #[case] input: serde_json::Value,
    #[case] change: Change,
    #[case] record: serde_json::Value,
) {
    let call = ToolCallId::from("toolu_1".to_owned());
    let hunks = if change == Change::Add {
        Vec::new()
    } else {
        vec![crate::harnesstools::session::Hunk {
            old_start: 1,
            old_lines: 2,
            new_start: 1,
            new_lines: 2,
            lines: vec![" a".into(), "-b".into(), "+c".into()],
        }]
    };
    let patch = Patch {
        call: call.clone(),
        files: vec![crate::harnesstools::session::FilePatch {
            path: "/w/a.rs".into(),
            change,
            moved_to: None,
            hunks,
        }],
    };
    let messages = vec![
        message("u1", None, Role::User, vec![Content::Text("edit it".into())]),
        message("a1", Some("u1"), Role::Assistant, vec![Content::ToolUse(ToolUse {
            id: call.clone(),
            name: tool.into(),
            input,
        })]),
        message("t1", Some("a1"), Role::Tool, vec![
            Content::ToolResult(ToolResult {
                call,
                output: json!("done"),
                error: false,
            }),
            Content::Patch(patch.clone()),
        ]),
    ];
    let session = session("5d1f0b1e-4a8e-4f1b-9d59-2c0f2f3f0a11", projects.path(), messages);
    let written = lines(&rehydrate_into(projects.path(), &session).unwrap());
    let result = written.iter().find(|l| l["uuid"] == "t1").unwrap();
    assert_eq!(result["toolUseResult"], record);
    // Read back, the line carries the same patch.
    let read: CcodeMessage = serde_json::from_value(result.clone()).unwrap();
    assert!(read.content().contains(&Content::Patch(patch)));
}

/// A result without a patch keeps its text as its record.
#[rstest]
fn a_result_without_a_patch_keeps_its_text(projects: TempDir) {
    let call = ToolCallId::from("toolu_1".to_owned());
    let messages = vec![
        message("u1", None, Role::User, vec![Content::Text("run it".into())]),
        message("a1", Some("u1"), Role::Assistant, vec![Content::ToolUse(ToolUse {
            id: call.clone(),
            name: "Bash".into(),
            input: json!({"command": "ls"}),
        })]),
        message("t1", Some("a1"), Role::Tool, vec![Content::ToolResult(ToolResult {
            call,
            output: json!("a.rs"),
            error: false,
        })]),
    ];
    let session = session("5d1f0b1e-4a8e-4f1b-9d59-2c0f2f3f0a11", projects.path(), messages);
    let written = lines(&rehydrate_into(projects.path(), &session).unwrap());
    let result = written.iter().find(|l| l["uuid"] == "t1").unwrap();
    assert_eq!(result["toolUseResult"], json!("a.rs"));
}
