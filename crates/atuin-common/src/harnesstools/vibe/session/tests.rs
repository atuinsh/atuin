use std::num::NonZeroUsize;
use std::time::Duration;

use futures::{StreamExt, TryStreamExt};
use rstest::{fixture, rstest};
use serde_json::{Value, json};

use super::*;

pub const FIXTURE_ID: &str = "e22d9812-1df4-5164-b22f-20290a24ffa0";

fn pool() -> BlockingPool {
    BlockingPool::new(NonZeroUsize::MIN)
}

/// A session Vibe 3.1.3 wrote (`vibe -p` reading, editing and writing a file and running two
/// commands, the second failing), trimmed to the files the reader reads, its paths and user made
/// up, and its `git_branch` set (a headless run records none).
pub fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vibe/session")
}

async fn read_all(session: &VibeSession) -> Vec<VibeMessage> {
    session.read().try_collect().await.unwrap()
}

#[fixture]
fn root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A session of the store under `root`, written as Vibe writes one: generation `1` holding
/// `entries` and the public state `envelope`, its journal holding `journal` (each record's
/// projection ops), and `runtime` as its runtime state.
fn write_store(
    root: &Path,
    id: &str,
    entries: &[Value],
    journal: &[Value],
    runtime: &Value,
    envelope: &Value,
) -> PathBuf {
    let dir = root.join(UNIFIED_DIR).joined(id);
    let generation = dir.join("generations").joined("0000000000000001");
    std::fs::create_dir_all(&generation).unwrap();
    std::fs::create_dir_all(dir.join("chunks")).unwrap();
    std::fs::create_dir_all(dir.join(JOURNAL_DIR)).unwrap();
    std::fs::write(dir.join("chunks").joined("c1.json"), serde_json::to_vec(entries).unwrap())
        .unwrap();
    let manifest = json!({
        "projection_state": { "chunks": ["c1"], "path": "projection-state.json" },
        "recovery_journal_segment": { "first_sequence": 2, "path": "journal/0000000000000002.jsonl" },
    });
    std::fs::write(generation.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(
        generation.join("projection-state.json"),
        json!({ "snapshot": { "session": envelope } }).to_string(),
    )
    .unwrap();
    std::fs::write(generation.join("runtime-state.json"), runtime.to_string()).unwrap();
    std::fs::write(dir.join(CURRENT_FILE), json!({ "generation": "0000000000000001" }).to_string())
        .unwrap();
    write_journal(&dir, journal);
    dir
}

fn write_journal(dir: &Path, journal: &[Value]) {
    let lines: String = journal
        .iter()
        .map(|ops| {
            json!({ "type": "projection_delta", "payload": { "delta": ops } }).to_string() + "\n"
        })
        .collect();
    std::fs::write(dir.join(JOURNAL_DIR).joined("0000000000000002.jsonl"), lines).unwrap();
}

fn runtime(cwd: &str) -> Value {
    json!({
        "identity": { "kind": "root", "parent_session_id": null },
        "session_metadata": { "cwd": cwd, "active_model": "mistral-large-4-0", "parent_session_id": null },
        "import_provenance": null,
    })
}

fn user(id: &str, text: &str) -> Value {
    json!({ "id": id, "type": "message", "role": "user", "generationStatus": "completed", "createdAt": 1_795_000_000_000_i64, "content": [{ "type": "text", "text": text }] })
}

fn reply(id: &str, text: &str) -> Value {
    json!({ "id": id, "type": "message", "role": "assistant", "generationStatus": "completed", "createdAt": 1_795_000_000_000_i64, "outcome": { "type": "committed" }, "content": [{ "type": "text", "text": text }] })
}

fn running(id: &str, command: &str) -> Value {
    json!({ "id": id, "type": "effect", "generationStatus": "in_progress", "toolCallId": format!("{id}-call"), "detail": { "toolName": "file_system.bash", "kind": "tool", "input": { "command": command } }, "state": { "status": "running" } })
}

fn ran(id: &str, command: &str, stdout: &str) -> Value {
    let mut entry = running(id, command);
    entry["generationStatus"] = json!("completed");
    entry["state"] = json!({ "status": "completed", "output": { "type": "success", "structured_content": { "command": command, "returncode": 0, "stdout": stdout } } });
    entry
}

fn ids(messages: &[VibeMessage]) -> Vec<String> {
    messages.iter().map(|m| m.id().unwrap().to_string()).collect()
}

#[rstest]
#[tokio::test]
async fn reads_a_session_vibe_wrote() {
    let root = fixture_root();
    let session =
        VibeSession::open(&root, root.join(UNIFIED_DIR).joined(FIXTURE_ID), pool()).unwrap();
    assert_eq!(session.id().as_ref(), FIXTURE_ID);
    let messages = read_all(&session).await;

    let calls: Vec<(String, Value)> = messages
        .iter()
        .flat_map(Message::content)
        .filter_map(|c| match c {
            Content::ToolUse(call) => Some((call.name, call.input)),
            _ => None,
        })
        .collect();
    let names: Vec<&str> = calls.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["read_file", "edit", "write_file", "bash", "bash"]);
    assert_eq!(calls[3].1["command"], "ls -1");

    let results: Vec<ToolResult> = messages
        .iter()
        .flat_map(Message::content)
        .filter_map(|c| match c {
            Content::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 5);
    assert_eq!(results.iter().map(|r| r.error).collect::<Vec<_>>(), [
        false, false, false, false, true
    ]);
    assert_eq!(results[4].output["returncode"], 1);

    let patches: Vec<Patch> = messages
        .iter()
        .flat_map(Message::content)
        .filter_map(|c| match c {
            Content::Patch(patch) => Some(patch),
            _ => None,
        })
        .collect();
    assert_eq!(patches.len(), 2);
    assert_eq!(patches[0].files[0].change, Change::Update);
    assert!(
        patches[0].files[0].hunks[0]
            .lines
            .iter()
            .any(|l| l.starts_with('+') && l.contains("MAGPIE"))
    );
    assert_eq!(patches[1].files[0].change, Change::Add);
    assert!(patches[1].files[0].path.ends_with("notes.txt"));

    let first = &messages[0];
    assert_eq!(first.role(), Role::User);
    assert_eq!(first.cwd(), Some(PathBuf::from("/home/user/proj")));
    assert_eq!(first.git_branch().as_deref(), Some("fixture-branch"));
    assert_eq!(first.model(), None);
    assert!(messages.iter().any(|m| m.model().as_deref() == Some("mistral-large-4-0")));
    assert!(
        messages.iter().any(|m| m.content().iter().any(|c| matches!(c, Content::Reasoning(_))))
    );
    let last_text =
        messages.iter().rev().find(|m| m.role() == Role::Assistant && m.title().is_none()).unwrap();
    assert!(matches!(&last_text.content()[0], Content::Text(t) if t.starts_with("All five steps")));
}

/// A tool call written as it starts holds the read back until it settles: the rows before it
/// are read at once, the call and its result once the journal replaces it with its outcome.
#[rstest]
#[tokio::test]
async fn a_running_call_waits_for_its_result(root: tempfile::TempDir) {
    let dir = write_store(
        root.path(),
        "s1",
        &[user("u1", "list it")],
        &[json!([{ "op": "append_entry", "entry": running("e1", "ls") }])],
        &runtime("/w"),
        &json!({}),
    );
    let session = VibeSession::open(root.path(), dir.clone(), pool()).unwrap();
    assert_eq!(ids(&read_all(&session).await), ["u1"]);

    write_journal(&dir, &[
        json!([{ "op": "append_entry", "entry": running("e1", "ls") }]),
        json!([{ "op": "replace_entry", "id": "e1", "entry": ran("e1", "ls", "a\n") }]),
        json!([{ "op": "append_entry", "entry": reply("r1", "done") }]),
    ]);
    let messages = read_all(&session).await;
    assert_eq!(ids(&messages), ["u1", "e1:call", "e1:result", "r1"]);
    assert!(
        matches!(&messages[1].content()[0], Content::ToolUse(call) if call.name == "bash" && call.id.as_ref() == "e1-call")
    );
    assert_eq!(messages[1].stop_reason(), Some(StopReason::ToolUse));
}

/// A read resumes just past the entry its checkpoint names; an entry no longer there (a rewind)
/// starts the session over, as a later read.
#[rstest]
#[tokio::test]
async fn a_read_resumes_where_its_checkpoint_still_holds(root: tempfile::TempDir) {
    let entries = [user("u1", "one"), reply("r1", "two"), user("u2", "three")];
    let dir = write_store(root.path(), "s1", &entries, &[], &runtime("/w"), &json!({}));
    let session = VibeSession::open(root.path(), dir, pool()).unwrap();
    let all: Vec<(Checkpoint, VibeMessage)> =
        session.clone().messages_from(None).try_collect().await.unwrap();
    let after_reply = all[1].0;
    // Each row names the row before it, which keeps rows of one timestamp in order.
    let parents: Vec<_> = all.iter().map(|(_, m)| m.parent_id().map(|p| p.to_string())).collect();
    assert_eq!(parents, [None, Some("u1".to_owned()), Some("r1".to_owned())]);

    let resumed: Vec<_> =
        session.clone().messages_from(Some(after_reply)).try_collect().await.unwrap();
    assert_eq!(resumed.iter().map(|(_, m)| m.id().unwrap().to_string()).collect::<Vec<_>>(), [
        "u2"
    ]);
    assert_eq!(resumed[0].0.generation, 0);
    assert_eq!(resumed[0].1.parent_id().map(|p| p.to_string()).as_deref(), Some("r1"));

    let moved = Checkpoint::new(2, b"no such entry");
    let over: Vec<_> = session.messages_from(Some(moved)).try_collect().await.unwrap();
    assert_eq!(over.len(), 3);
    assert!(over.iter().all(|(at, _)| at.generation == 1));
}

/// A call's row resumes before its entry, so a read stopped between the call and its result reads
/// both again.
#[rstest]
#[tokio::test]
async fn only_an_entrys_last_row_moves_the_checkpoint_past_it(root: tempfile::TempDir) {
    let entries = [user("u1", "one"), ran("e1", "ls", "")];
    let dir = write_store(root.path(), "s1", &entries, &[], &runtime("/w"), &json!({}));
    let session = VibeSession::open(root.path(), dir, pool()).unwrap();
    let all: Vec<(Checkpoint, VibeMessage)> =
        session.messages_from(None).try_collect().await.unwrap();
    let checkpoints: Vec<u64> = all.iter().map(|(at, _)| at.at).collect();
    assert_eq!(checkpoints, [1, 1, 2]);
}

#[rstest]
#[tokio::test]
async fn the_title_is_a_row_of_its_own(root: tempfile::TempDir) {
    let envelope = json!({ "title": "Five step task list", "titleSource": "auto", "updatedAt": 1_795_000_000_000_i64 });
    let dir = write_store(root.path(), "s1", &[user("u1", "hi")], &[], &runtime("/w"), &envelope);
    let session = VibeSession::open(root.path(), dir.clone(), pool()).unwrap();
    let messages = read_all(&session).await;
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[1].title(),
        Some(TitleChange::new(TitleSource::Generated, "Five step task list"))
    );

    write_journal(&dir, &[
        json!([{ "op": "set_envelope", "state": { "session": { "title": "Renamed", "titleSource": "manual" } } }]),
    ]);
    let messages = read_all(&session).await;
    assert_eq!(messages[1].title(), Some(TitleChange::new(TitleSource::Named, "Renamed")));
}

#[rstest]
#[tokio::test]
async fn a_subagent_names_the_session_that_spawned_it(root: tempfile::TempDir) {
    let runtime = json!({
        "identity": { "kind": "subagent", "parent_session_id": "parent-1" },
        "session_metadata": { "cwd": "/w", "parent_session_id": "parent-1" },
    });
    let dir = write_store(
        root.path(),
        "child-abc",
        &[user("u1", "count files")],
        &[],
        &runtime,
        &json!({}),
    );
    let session = VibeSession::open(root.path(), dir, pool()).unwrap();
    assert_eq!(session.id().as_ref(), "child-abc");
    let messages = read_all(&session).await;
    assert_eq!(messages[0].parent_session().map(|s| s.to_string()).as_deref(), Some("parent-1"));
    assert_eq!(messages[0].parent_kind(), Some(ParentKind::Subagent));
}

/// A legacy folder atuin wrote and Vibe imported reads under the id atuin wrote it as, with only
/// what Vibe added since: the imported entries are rows atuin already holds.
#[rstest]
#[tokio::test]
async fn an_import_of_a_session_atuin_wrote_reads_as_that_session(
    root: tempfile::TempDir,
    #[values(true, false)] by_atuin: bool,
) {
    let source = "a7e0f00d-1111-4222-8333-944455556666";
    let legacy = root.path().join("session_20261118_120000_a7e0f00d");
    std::fs::create_dir_all(&legacy).unwrap();
    let mut meta = json!({ "session_id": source, "title": "Written title" });
    if by_atuin {
        meta[ATUIN_KEY] = json!({ "version": 1, "fork_of": "c4e8fddb" });
    }
    std::fs::write(legacy.join("meta.json"), meta.to_string()).unwrap();
    let written = |id: &str| {
        json!({ "id": id, "role": "User", "time": "2026-11-18T12:00:00Z", "content": [{ "Text": "old" }] }).to_string() + "\n"
    };
    std::fs::write(legacy.join(WRITTEN_FILE), written("w1") + &written("w2")).unwrap();
    // Vibe imported the folder before `w2` was appended to it.
    let revisions = "{\"revision\":\"r1\",\"rows\":1}\n{\"revision\":\"r2\",\"rows\":2}\n";
    std::fs::write(legacy.join(REVISIONS_FILE), revisions).unwrap();

    let mut runtime = runtime("/w");
    runtime["import_provenance"] = json!({ "imported_at": "2026-11-18T22:43:47Z", "source": { "backend": "legacy", "session_id": source, "store_revision": "r1" } });
    let entries = [user("imported-1", "old"), reply("imported-2", "old reply"), user("u1", "new")];
    let dir = write_store(
        root.path(),
        "3a287dd5-8631-b2d2-07ba-ade24d0e34de",
        &entries,
        &[],
        &runtime,
        &json!({}),
    );

    let session = VibeSession::open(root.path(), dir.clone(), pool()).unwrap();
    let messages = read_all(&session).await;
    if by_atuin {
        assert_eq!(session.id().as_ref(), source);
        // The rows atuin wrote that Vibe imported stand for the imported entries, then what Vibe
        // added.
        assert_eq!(ids(&messages[..2]), ["w1", "u1"]);
        assert_eq!(messages[0].content(), [Content::Text("old".into())]);
        assert_eq!(
            messages[1].parent_session().map(|p| p.to_string()).as_deref(),
            Some("c4e8fddb")
        );
        assert_eq!(messages[1].parent_kind(), Some(ParentKind::Fork));
        assert_eq!(messages[1].parent_id().map(|p| p.to_string()).as_deref(), Some("w1"));
        assert_eq!(messages[2].parent_id(), None);
        // Vibe's import drops the title atuin wrote, so the read gives it until Vibe has one.
        assert_eq!(
            messages[2].title(),
            Some(TitleChange::new(TitleSource::Generated, "Written title"))
        );
        assert_eq!(messages.len(), 3);
        assert_eq!(locate(root.path(), source), Some(dir.clone()));
        assert_eq!(store_dir(root.path(), source), Some(dir));
    }
    // A subagent names the session that spawned it as atuin captures it.
    let runtime = json!({
        "identity": { "kind": "subagent", "parent_session_id": "3a287dd5-8631-b2d2-07ba-ade24d0e34de" },
        "session_metadata": { "cwd": "/w" },
    });
    let child =
        write_store(root.path(), "child-abc", &[user("c1", "go")], &[], &runtime, &json!({}));
    let child = VibeSession::open(root.path(), child, pool()).unwrap();
    let parent = read_all(&child).await[0].parent_session().map(|s| s.to_string());
    assert_eq!(parent.as_deref(), Some(session.id().as_ref()));
    if !by_atuin {
        assert_eq!(session.id().as_ref(), "3a287dd5-8631-b2d2-07ba-ade24d0e34de");
        assert_eq!(ids(&messages), ["imported-1", "imported-2", "u1"]);
    }
}

#[rstest]
fn locate_finds_a_store_session_or_a_legacy_folder(root: tempfile::TempDir) {
    let dir = write_store(root.path(), "s1", &[], &[], &runtime("/w"), &json!({}));
    assert_eq!(locate(root.path(), "s1"), Some(dir));

    let legacy = root.path().join("session_20261118_120000_a7e0f00d");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("meta.json"), json!({ "session_id": "a7e0f00d-1111" }).to_string())
        .unwrap();
    assert_eq!(locate(root.path(), "a7e0f00d-1111"), Some(legacy));
    assert_eq!(locate(root.path(), "a7e0f00d-2222"), None);
    assert_eq!(locate(root.path(), "../s1"), None);
}

#[rstest]
fn a_partial_last_journal_line_waits_for_the_next_read() {
    let (mut entries, mut envelope) = (Vec::new(), Value::Null);
    let whole = json!({ "type": "projection_delta", "payload": { "delta": [{ "op": "append_entry", "entry": user("u1", "a") }] } }).to_string();
    let bytes = format!("{whole}\n{}", &whole[..whole.len() / 2]);
    replay(bytes.as_bytes(), &mut entries, &mut envelope, &mut Recorded::default());
    assert_eq!(entries.len(), 1);
}

#[rstest]
#[case::removed(json!([{ "op": "remove_entry", "id": "u1" }]), vec![])]
#[case::set_all(json!([{ "op": "set_history_entries", "entries": [user("x", "y")] }]), vec!["x"])]
#[case::unknown(json!([{ "op": "a_newer_op" }]), vec!["u1"])]
fn replay_folds_projection_ops(#[case] ops: Value, #[case] expected: Vec<&str>) {
    let (mut entries, mut envelope) = (vec![user("u1", "a")], Value::Null);
    let line =
        json!({ "type": "projection_delta", "payload": { "delta": ops } }).to_string() + "\n";
    replay(line.as_bytes(), &mut entries, &mut envelope, &mut Recorded::default());
    let got: Vec<&str> = entries.iter().filter_map(|e| e["id"].as_str()).collect();
    assert_eq!(got, expected);
}

/// The listener reports a session made after it started, and wakes its reader as the journal
/// grows.
#[rstest]
#[tokio::test]
async fn the_listener_follows_a_session_as_vibe_writes_it(root: tempfile::TempDir) {
    let sessions = VibeSessions::builder().root(root.path()).pool(pool()).build();
    let listener = sessions.listener().unwrap();
    let events = listener.events(std::time::SystemTime::UNIX_EPOCH, |_| async { None });
    futures::pin_mut!(events);

    let dir = write_store(root.path(), "s1", &[user("u1", "hi")], &[], &runtime("/w"), &json!({}));
    let wait = Duration::from_secs(20);
    let first = tokio::time::timeout(wait, events.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(first.session.as_ref(), "s1");
    assert_eq!(first.message.id().unwrap().as_ref(), "u1");

    write_journal(&dir, &[json!([{ "op": "append_entry", "entry": reply("r1", "hello") }])]);
    let second = tokio::time::timeout(wait, events.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(second.message.id().unwrap().as_ref(), "r1");
}

/// A model call's usage is on its first row, and a call gets the arguments its entry leaves out,
/// both from the journal: the segment going on from the generation, and one before it that Vibe
/// keeps.
#[rstest]
#[tokio::test]
async fn usage_and_left_out_arguments_come_from_the_journal(root: tempfile::TempDir) {
    const SEARCH: &str = "b2a2350b-e8a8-5077-9173-f04b241a6c47";
    const ANSWER: &str = "84a7ef7b-776b-5a1f-899f-ace36b5c15c9";
    let dir = write_store(root.path(), "s1", &[user("u1", "hi")], &[], &runtime("/w"), &json!({}));
    let completion = |action: &str, parts: Value, input: u64| {
        json!({ "type": "action_result", "payload": { "action_id": action, "result": {
            "type": "completion_succeeded", "result": { "parts": parts, "usage": {
                "input_tokens": input, "cached_input_tokens": 3584, "cache_write_tokens": 0,
                "output_tokens": 368, "reasoning_tokens": 346, "total_tokens": input + 368 } } } } })
    };
    let delta = |entries: Vec<Value>| {
        let ops: Vec<Value> =
            entries.into_iter().map(|e| json!({ "op": "append_entry", "entry": e })).collect();
        json!({ "type": "projection_delta", "payload": { "delta": ops } })
    };
    let search = json!({ "id": "effect-tool-discovery-c1", "type": "effect", "generationStatus": "completed",
        "detail": { "toolName": "search_tool_functions", "input": { "mode": "best_match" } },
        "state": { "status": "completed", "output": { "mode": "best_match", "toolCount": 3 } } });
    let thought =
        json!({ "id": format!("reasoning-{SEARCH}-0"), "type": "reasoning", "text": "look" });
    let args = json!({ "mode": "best_match", "query": "find files" }).to_string();
    let lines = [
        completion(
            SEARCH,
            json!([{ "type": "reasoning" }, { "type": "tool_call", "id": "c1",
            "name": "search_tool_functions", "arguments_json": args }]),
            9008,
        ),
        delta(vec![thought, search]),
        delta(vec![reply(&format!("assistant-{ANSWER}"), "found")]),
    ];
    let lines: String = lines.iter().map(|l| l.to_string() + "\n").collect();
    std::fs::write(dir.join(JOURNAL_DIR).joined("0000000000000002.jsonl"), lines).unwrap();
    let older = completion(ANSWER, json!([{ "type": "text" }]), 10000).to_string() + "\n";
    std::fs::write(dir.join(JOURNAL_DIR).joined("0000000000000001.jsonl"), older).unwrap();

    let session = VibeSession::open(root.path(), dir, pool()).unwrap();
    let messages = read_all(&session).await;
    let usage: Vec<Option<u64>> =
        messages.iter().map(|m| m.usage().and_then(|u| u.input)).collect();
    assert_eq!(usage, [None, Some(9008 - 3584), None, None, Some(10000 - 3584)]);
    assert_eq!(messages[1].usage().unwrap().reasoning, Some(346));
    let Content::ToolUse(call) = &messages[2].content()[0] else {
        panic!("not a call")
    };
    assert_eq!(call.id.as_ref(), "c1");
    assert_eq!(call.input["query"], "find files");
    let Content::ToolResult(found) = &messages[3].content()[0] else {
        panic!("not a result")
    };
    assert_eq!(found.output["toolCount"], 3);
}
