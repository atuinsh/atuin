use std::num::NonZeroUsize;
use std::path::PathBuf;

use futures::TryStreamExt;
use rstest::rstest;
use serde_json::{Value, json};

use super::*;
use crate::harnesstools::ccode::Ccode;
use crate::harnesstools::ccode::session::{CcodeMessage, CcodeSession};
use crate::harnesstools::codex::Codex;
use crate::harnesstools::codex::session::CodexSession;
use crate::harnesstools::opencode::Opencode;
use crate::harnesstools::pi::Pi;
use crate::harnesstools::pi::session::PiSession;
use crate::harnesstools::session::{Session, SessionId, ToolCallId, ToolResult, ToolUse};
use crate::harnesstools::{ccode, codex, opencode, pi};
use crate::sync::BlockingPool;

const CLAUDE: AnyHarness = AnyHarness::ClaudeCode(Ccode);
const CODEX: AnyHarness = AnyHarness::Codex(Codex);
const OPENCODE: AnyHarness = AnyHarness::Opencode(Opencode);
const PI: AnyHarness = AnyHarness::Pi(Pi);

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(path)
}

fn pool() -> BlockingPool {
    BlockingPool::new(NonZeroUsize::MIN)
}

/// A captured row of `m`, keyed on its own id (or its place, for a line without one).
fn row_of<M: Message>(n: usize, m: &M) -> RehydrateMessage {
    RehydrateMessage {
        source_id: m.id().map_or_else(|| format!("line-{n}"), String::from),
        parent_source_id: m.parent_id().map(String::from),
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
    }
}

fn rows_of<M: Message>(messages: &[M]) -> Vec<RehydrateMessage> {
    messages.iter().enumerate().map(|(n, m)| row_of(n, m)).collect()
}

fn session_of(id: &str, messages: Vec<RehydrateMessage>) -> RehydrateSession {
    RehydrateSession {
        id: id.to_owned(),
        title: Some("fix the flaky test".to_owned()),
        cwd: std::env::temp_dir(),
        original_cwd: Some(PathBuf::from("/work/proj")),
        git_branch: Some("main".to_owned()),
        model: Some("some-model".to_owned()),
        started_at: messages
            .iter()
            .map(|m| m.timestamp)
            .min()
            .unwrap_or(OffsetDateTime::UNIX_EPOCH),
        messages,
    }
}

/// A real session recorded by `source`, as capture holds it.
async fn recorded(source: AnyHarness) -> RehydrateSession {
    match source {
        AnyHarness::ClaudeCode(_) => {
            let text = std::fs::read_to_string(fixture("ccode/session1.jsonl")).unwrap();
            let lines: Vec<CcodeMessage> =
                text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
            session_of("4bd6b5bd-7d34-4a39-a2d6-2e6f6a6c0b1d", rows_of(&lines))
        }
        AnyHarness::Codex(_) => {
            let id = "019a0d14-f276-77d3-b955-89d5b0151306";
            let lines: Vec<_> = CodexSession::open(
                SessionId::from(id.to_owned()),
                fixture("codex/session1.jsonl"),
                pool(),
            )
            .read()
            .try_collect()
            .await
            .unwrap();
            session_of(id, rows_of(&lines))
        }
        AnyHarness::Opencode(_) => {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("opencode.db");
            let id = opencode::rehydrate::tests::load_projection(&db).await;
            let mut rows = opencode::rehydrate::tests::captured(&db).await;
            // The fixture calls no tools: add calls as capture reads opencode's tool parts, a
            // call with its result, into its first answer.
            let at = rows.iter().position(|r| r.role == Role::Assistant).unwrap();
            for (n, (tool, input)) in [
                ("bash", json!({"command": "bun test", "description": "Run the tests"})),
                ("read", json!({"filePath": "/work/proj/src/index.ts"})),
            ]
            .into_iter()
            .enumerate()
            {
                let mut call = rows[at].clone();
                call.source_id = format!("prt_tool{n}");
                call.content = vec![
                    Content::ToolUse(ToolUse {
                        id: ToolCallId::from(format!("toolu_{n}")),
                        name: tool.to_owned(),
                        input,
                    }),
                    Content::ToolResult(ToolResult {
                        call: ToolCallId::from(format!("toolu_{n}")),
                        output: json!("12 pass"),
                        error: false,
                    }),
                ];
                rows.insert(at + 1 + n, call);
            }
            session_of(&id, rows)
        }
        AnyHarness::Pi(_) => {
            let id = "372bddb6-5d16-1553-fe79-3ffc84a47b20";
            let lines: Vec<_> = PiSession::open(
                SessionId::from(id.to_owned()),
                fixture("pi/session1.jsonl"),
                pool(),
            )
            .read()
            .try_collect()
            .await
            .unwrap();
            session_of(id, rows_of(&lines))
        }
    }
}

/// The session id, and every row id, is one the target's writer takes, in the target's format.
fn assert_ids(target: AnyHarness, session: &RehydrateSession) {
    let id = &session.id;
    let uuid = |s: &str| Uuid::parse_str(s).is_ok();
    match target {
        AnyHarness::ClaudeCode(_) => assert!(uuid(id), "{id}"),
        AnyHarness::Codex(_) => assert_eq!(Uuid::parse_str(id).unwrap().get_version_num(), 7),
        AnyHarness::Pi(_) => assert_eq!(Uuid::parse_str(id).unwrap().get_version_num(), 7),
        AnyHarness::Opencode(_) => {
            let rest = id.strip_prefix("ses_").unwrap();
            assert_eq!(rest.len(), 26, "{id}");
            assert!(rest[..12].chars().all(|c| c.is_ascii_hexdigit()));
            assert!(rest[12..].chars().all(|c| c.is_ascii_alphanumeric()));
        }
    }
    let mut seen = HashSet::new();
    for m in &session.messages {
        let id = m.source_id.as_str();
        assert!(seen.insert(id), "{id} is used twice");
        match target {
            AnyHarness::ClaudeCode(_) => assert!(uuid(id), "{id}"),
            AnyHarness::Codex(_) => assert!(id.starts_with("syn-"), "{id}"),
            AnyHarness::Opencode(_) => {
                assert!(id.starts_with("prt_") && id.len() == 30, "{id}");
            }
            AnyHarness::Pi(_) => {
                assert!(id.len() == 8 && id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
            }
        }
    }
    // Rows sort by id in time order where the target orders by id.
    if matches!(target, AnyHarness::Opencode(_)) {
        let ids: Vec<&str> = session.messages.iter().map(|m| m.source_id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }
}

/// The marker first, then user and assistant turns in turn, user first and assistant last,
/// each one text; nothing a tool call or result, nothing reasoning.
fn assert_shape(target: AnyHarness, source: AnyHarness, original: &str, s: &RehydrateSession) {
    // A line of its own (the harness's), or pi's first prompt's first block.
    let (marker, turns) = match target {
        AnyHarness::Pi(_) => (&s.messages[0], s.messages.as_slice()),
        _ => s.messages.split_first().expect("a marker"),
    };
    let [Content::Text(text), ..] = marker.content.as_slice() else {
        panic!("the marker is text: {marker:?}");
    };
    let (harness, id) = continued_from(text).unwrap();
    assert_eq!((harness.name(), id), (source.name(), original));
    let linked = continued_from_row(marker);
    assert_eq!(linked, Some((source.name(), original.to_owned())));
    if !matches!(target, AnyHarness::Pi(_)) {
        assert_eq!(marker.role, Role::System);
    }

    assert!(!turns.is_empty());
    assert_eq!(turns.len() % 2, 0, "user first, assistant last");
    for (n, m) in turns.iter().enumerate() {
        let want = if n % 2 == 0 {
            Role::User
        } else {
            Role::Assistant
        };
        assert_eq!(m.role, want, "turn {n}");
        let content = match (n, target) {
            (0, AnyHarness::Pi(_)) => &m.content[1..],
            _ => &m.content[..],
        };
        let [Content::Text(text)] = content else {
            panic!("turn {n} is one text: {m:?}");
        };
        assert!(!text.trim().is_empty());
        assert!(m.usage.is_none() && m.model.is_none() && m.turn_id.is_none());
    }
    let times: Vec<_> = s.messages.iter().map(|m| m.timestamp).collect();
    assert!(times.windows(2).all(|w| w[0] < w[1]), "every row after the one before");
    assert_eq!(s.model, None);
}

/// Every source into every other target: fresh ids in the target's format, only text left, and
/// a conversation of the shape every API takes.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_pair_flattens_to_alternating_text(
    #[values(CLAUDE, CODEX, OPENCODE, PI)] source: AnyHarness,
    #[values(CLAUDE, CODEX, OPENCODE, PI)] target: AnyHarness,
) {
    if source.name() == target.name() {
        return;
    }
    let original = recorded(source).await;
    let calls = original
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|c| matches!(c, Content::ToolUse(_)))
        .count();
    assert!(calls > 0, "the {} fixture calls tools", source.name());

    let continued = continue_in(source, &original, target);
    assert_ids(target, &continued.session);
    assert_shape(target, source, &original.id, &continued.session);
    assert_ne!(continued.session.id, original.id);
    assert_eq!(continued.flattened.tool_calls, calls);
    let notes: usize = continued
        .session
        .messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .flat_map(|m| &m.content)
        .map(|c| match c {
            // A note repeated is written once, counted (` ×3`).
            Content::Text(t) => t
                .lines()
                .filter(|l| l.starts_with('['))
                .map(|l| l.rsplit_once(" ×").map_or(1, |(_, n)| n.parse::<usize>().unwrap()))
                .sum(),
            _ => 0,
        })
        .sum();
    assert!(notes >= calls, "each call is a note ({notes} < {calls})");
}

/// The rows a continuation's transcript reads back as, through the target's own reader (the
/// one capture uses), and the session id it is filed under.
async fn written(target: AnyHarness, session: &RehydrateSession) -> Vec<RehydrateMessage> {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let id = SessionId::from(session.id.clone());
    match target {
        AnyHarness::ClaudeCode(_) => {
            let path = ccode::rehydrate::rehydrate_into(root, session).unwrap();
            assert_eq!(ccode::session::locate(root, &session.id), Some(path.clone()));
            let lines: Vec<_> =
                CcodeSession::open(id, path, pool()).read().try_collect().await.unwrap();
            rows_of(&lines)
        }
        AnyHarness::Codex(_) => {
            let path = codex::rehydrate::write(root, session).unwrap();
            assert_eq!(codex::session::locate(root, &session.id), Some(path.clone()));
            let lines: Vec<_> =
                CodexSession::open(id, path, pool()).read().try_collect().await.unwrap();
            rows_of(&lines)
        }
        AnyHarness::Pi(_) => {
            let path =
                pi::rehydrate::rehydrate_into(root, &root.join("sessions"), session).unwrap();
            assert_eq!(pi::session::locate(root, &session.id), Some(path.clone()));
            let lines: Vec<_> =
                PiSession::open(id, path, pool()).read().try_collect().await.unwrap();
            rows_of(&lines)
        }
        AnyHarness::Opencode(_) => {
            use opencode::rehydrate::tests::{captured, database, opencode_import};
            let db = root.join("opencode.db");
            let mut conn = database(&db).await;
            let export = opencode::rehydrate::export(session);
            opencode_import(&mut conn, &export, "/here").await;
            sqlx::Connection::close(conn).await.unwrap();
            let rows = captured(&db).await;
            assert!(
                rows.iter().any(|r| r.source_id.starts_with(&format!("{}:", session.id))),
                "the session is opencode's under its id"
            );
            rows
        }
    }
}

/// What a transcript says in the conversation, in order: role and text, merged where one role
/// speaks twice in a row (the marker is the user's turn, as every target sends it).
fn conversation(rows: &[RehydrateMessage]) -> Vec<(Role, String)> {
    let mut out: Vec<(Role, String)> = Vec::new();
    for r in rows {
        let role = match r.role {
            Role::Assistant => Role::Assistant,
            Role::User | Role::System | Role::Other(_) => Role::User,
            Role::Tool => panic!("a tool row was written: {r:?}"),
        };
        for c in &r.content {
            match c {
                Content::Text(t) if !t.trim().is_empty() => match out.last_mut() {
                    Some((last, text)) if *last == role => {
                        text.push('\n');
                        text.push_str(t);
                    }
                    _ => out.push((role.clone(), t.clone())),
                },
                Content::Text(_) => {}
                other => panic!("only text is written: {other:?}"),
            }
        }
    }
    out
}

/// A continuation, written by the target's writer and read back by the target's reader (the
/// one live capture uses), is the new session, holds the same conversation, and its marker is
/// what capture links to the session it continues.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_target_reads_back_the_conversation_and_its_parent(
    #[values(CLAUDE, CODEX, OPENCODE, PI)] source: AnyHarness,
    #[values(CLAUDE, CODEX, OPENCODE, PI)] target: AnyHarness,
) {
    if source.name() == target.name() {
        return;
    }
    let original = recorded(source).await;
    let continued = continue_in(source, &original, target).session;
    let rows = written(target, &continued).await;

    let expected = conversation(&continued.messages);
    let again = conversation(&rows);
    pretty_assertions::assert_eq!(again, expected);

    let parents: Vec<_> = rows.iter().filter_map(|r| continued_from_row(r)).collect();
    assert_eq!(parents, vec![(source.name(), original.id.clone())]);
}

/// [`continued_from_message`] over a read-back row.
fn continued_from_row(r: &RehydrateMessage) -> Option<(&'static str, String)> {
    struct Row(RehydrateMessage);
    impl Message for Row {
        fn id(&self) -> Option<crate::harnesstools::session::MessageId> {
            None
        }
        fn role(&self) -> Role {
            self.0.role.clone()
        }
        fn timestamp(&self) -> Option<OffsetDateTime> {
            None
        }
        fn content(&self) -> Vec<Content> {
            self.0.content.clone()
        }
    }
    continued_from_message(&Row(r.clone())).map(|(h, id)| (h.name(), id))
}

fn msg(role: Role, content: Vec<Content>, at: i64) -> RehydrateMessage {
    RehydrateMessage {
        source_id: format!("r{at}"),
        parent_source_id: None,
        timestamp: OffsetDateTime::from_unix_timestamp(at).unwrap(),
        role,
        content,
        model: Some("claude-opus-5".to_owned()),
        usage: None,
        stop_reason: None,
        turn_id: None,
        cwd: None,
        git_branch: None,
        seq: None,
    }
}

fn call(name: &str, input: Value) -> Content {
    Content::ToolUse(ToolUse {
        id: ToolCallId::from(format!("call-{name}")),
        name: name.to_owned(),
        input,
    })
}

fn result(output: &str) -> Content {
    Content::ToolResult(ToolResult {
        call: ToolCallId::from("c".to_owned()),
        output: json!(output),
        error: false,
    })
}

fn text(t: &str) -> Content {
    Content::Text(t.to_owned())
}

/// Tool rows go, calls become notes of their input and never their output, reasoning goes, and
/// what is left of one role in a row is one turn.
#[rstest]
fn calls_become_notes_and_turns_merge() {
    let original = session_of("orig", vec![
        msg(Role::System, vec![text("<environment_context>cwd</environment_context>")], 1),
        msg(Role::User, vec![text("fix the flaky test")], 2),
        msg(Role::Assistant, vec![Content::ReasoningSummary { tokens: Some(40) }], 3),
        msg(
            Role::Assistant,
            vec![
                text("Looking."),
                call(
                    "Bash",
                    json!({"command": "cargo test -p atuin-client", "description": "run"}),
                ),
            ],
            4,
        ),
        msg(Role::Tool, vec![result("SECRET OUTPUT: 3 passed")], 5),
        msg(
            Role::Assistant,
            vec![call(
                "Edit",
                json!({"file_path": "src/store.rs", "old_string": "a", "new_string": "b"}),
            )],
            6,
        ),
        msg(Role::Tool, vec![result("SECRET OUTPUT: edited")], 7),
        msg(Role::User, vec![result("SECRET OUTPUT: rejected"), text("no, the other one")], 8),
        msg(Role::User, vec![text("and hurry")], 9),
        msg(Role::Assistant, vec![Content::Error("overloaded_error: Overloaded".into())], 10),
        msg(Role::System, vec![Content::Summary("We fixed the store.".into())], 11),
    ]);
    let c = continue_at(CLAUDE, &original, CODEX, OffsetDateTime::UNIX_EPOCH);
    assert_eq!(c.flattened, Flattened {
        tool_calls: 2,
        tool_results: 3,
        reasoning: 1
    });
    assert_eq!(c.flattened.summary(), "2 tool calls flattened to notes, reasoning dropped");
    let turns: Vec<(Role, String)> = c.session.messages[1..]
        .iter()
        .map(|m| match m.content.as_slice() {
            [Content::Text(t)] => (m.role.clone(), t.clone()),
            other => panic!("{other:?}"),
        })
        .collect();
    pretty_assertions::assert_eq!(turns, vec![
        (Role::User, "fix the flaky test".to_owned()),
        (
            Role::Assistant,
            "Looking.\n\n[ran `cargo test -p atuin-client`]\n[edited `src/store.rs`]".to_owned()
        ),
        (Role::User, "no, the other one\n\nand hurry".to_owned()),
        (Role::Assistant, "[error: overloaded_error: Overloaded]".to_owned()),
        (Role::User, "[Summary of the earlier conversation]\nWe fixed the store.".to_owned()),
        (Role::Assistant, "[the original session ended before a reply to this]".to_owned()),
    ]);
    let everything = format!("{:?}", c.session);
    assert!(!everything.contains("SECRET OUTPUT"), "no tool output is carried over");
    assert!(!everything.contains("environment_context"), "nor the source's own context");
}

/// Calls captured without their input (all of them, now) say only what they did, and the same
/// note in a row is written once, counted.
#[rstest]
fn repeated_notes_are_counted() {
    let original = session_of("orig", vec![
        msg(Role::User, vec![text("why is the wal so large?")], 1),
        msg(Role::Assistant, vec![text("Looking."), call("exec_command", Value::Null)], 2),
        msg(Role::Assistant, vec![call("exec_command", Value::Null)], 3),
        msg(Role::Assistant, vec![call("exec_command", Value::Null)], 4),
        msg(Role::Assistant, vec![call("apply_patch", Value::Null)], 5),
        msg(Role::Assistant, vec![call("exec_command", Value::Null), text("It's sparse.")], 6),
    ]);
    let c = continue_in(CODEX, &original, CLAUDE);
    let [Content::Text(reply)] = c.session.messages[2].content.as_slice() else {
        panic!("{:?}", c.session.messages[2]);
    };
    assert_eq!(
        reply,
        "Looking.\n\n[ran a shell command] ×3\n[applied a patch]\n[ran a shell command]\n\nIt's \
         sparse."
    );
    assert_eq!(c.flattened.tool_calls, 5);
}

#[rstest]
#[case::claude(CLAUDE, "4bd6b5bd-7d34-4a39-a2d6-2e6f6a6c0b1d")]
#[case::opencode(OPENCODE, "ses_f2b627a90ffe3Kkao0Ukcp43QI")]
fn markers_read_back(#[case] source: AnyHarness, #[case] id: &str) {
    let text = marker_text(source, id);
    let (harness, got) = continued_from(&text).unwrap();
    assert_eq!((harness.name(), got), (source.name(), id));
}

#[rstest]
#[case::typed_prompt("please continue from claude-code session abc via atuin.")]
#[case::unknown_harness("Continued from cursor session abc via atuin.")]
#[case::path_id("Continued from pi session ../x via atuin.")]
#[case::not_first_line("hello\nContinued from pi session abc via atuin.")]
fn other_text_is_no_marker(#[case] text: &str) {
    assert!(continued_from(text).is_none());
}

/// A marker the user typed is no link: only a line the harness put in the turn itself is.
#[rstest]
fn a_typed_marker_is_no_link() {
    let marker = marker_text(CODEX, "019a0d14-f276-77d3-b955-89d5b0151306");
    let typed = msg(Role::User, vec![text(&marker)], 1);
    assert_eq!(continued_from_row(&typed), None);
    let later = msg(Role::User, vec![text("hi"), text(&marker)], 1);
    assert_eq!(continued_from_row(&later), None, "only a prompt's first block");
    let injected = msg(Role::System, vec![text(&marker)], 1);
    assert!(continued_from_row(&injected).is_some());
    // pi's: the first block of a prompt with more after it.
    let first_block = msg(Role::User, vec![text(&marker), text("fix it")], 1);
    assert!(continued_from_row(&first_block).is_some());
}

#[rstest]
fn a_conversation_opening_with_a_reply_gets_a_user_turn_first() {
    let original = session_of("orig", vec![msg(Role::Assistant, vec![text("hello")], 1)]);
    let c = continue_in(PI, &original, CLAUDE);
    let roles: Vec<Role> = c.session.messages.iter().map(|m| m.role.clone()).collect();
    assert_eq!(roles, vec![Role::System, Role::User, Role::Assistant]);
}

/// Where the target keeps a tree, each row hangs from the one before; opencode finds a reply's
/// question itself.
#[rstest]
#[case(CLAUDE, true)]
#[case(PI, true)]
#[case(CODEX, false)]
#[case(OPENCODE, false)]
fn rows_chain_where_the_target_keeps_a_tree(#[case] target: AnyHarness, #[case] tree: bool) {
    let original = session_of("orig", vec![
        msg(Role::User, vec![text("a")], 1),
        msg(Role::Assistant, vec![text("b")], 2),
    ]);
    let s = continue_in(CLAUDE, &original, target).session;
    let parents: Vec<Option<&str>> =
        s.messages.iter().map(|m| m.parent_source_id.as_deref()).collect();
    if tree {
        let mut chain = vec![None];
        chain.extend(s.messages.iter().map(|m| Some(m.source_id.as_str())));
        chain.pop();
        assert_eq!(parents, chain);
        // The marker and the two turns; pi's marker is in its first prompt.
        let rows = if matches!(target, AnyHarness::Pi(_)) {
            2
        } else {
            3
        };
        assert_eq!(s.messages.len(), rows);
    } else {
        assert!(parents.iter().all(Option::is_none));
    }
}
