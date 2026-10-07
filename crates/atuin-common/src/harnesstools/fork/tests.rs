use std::collections::HashSet;
use std::path::Path;

use futures::TryStreamExt;
use rstest::rstest;

use super::*;
use crate::harnesstools::ccode::session::CcodeSession;
use crate::harnesstools::codex::session::CodexSession;
use crate::harnesstools::continuation::tests::{
    CLAUDE, CODEX, OPENCODE, PI, pool, recorded, rows_of,
};
use crate::harnesstools::continuation::{fork_marker_text, linked_from_message};
use crate::harnesstools::opencode::rehydrate::tests::{captured, database, opencode_import};
use crate::harnesstools::pi::session::PiSession;
use crate::harnesstools::session::{Content, Message, ParentKind, Role, Session, SessionId};
use crate::harnesstools::{ccode, codex, opencode, pi};

/// The original's atuin id, as the picker passes it.
const ATUIN_ID: &str = "01a0d147e7457ae280000123456789ab";

/// A captured row, as a line capture reads.
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

/// Each line's link to another session, as capture reads it: the harness's own field, else (for
/// opencode, whose rows are read from its database) a marker.
fn links<M: Message>(own: &str, lines: &[M]) -> Vec<(String, ParentKind)> {
    lines
        .iter()
        .filter_map(|m| Some((m.parent_session()?.to_string(), m.parent_kind()?)))
        .filter(|(parent, _)| parent != own)
        .collect()
}

/// `session` written by `harness`'s writer under `root` and read back by its reader (the one
/// capture uses): its rows, and the links its lines make. opencode's database holds whatever was
/// written into it before.
async fn written(
    harness: AnyHarness,
    root: &Path,
    session: &RehydrateSession,
) -> (Vec<RehydrateMessage>, Vec<(String, ParentKind)>) {
    let id = SessionId::from(session.id.clone());
    match harness {
        AnyHarness::ClaudeCode(_) => {
            let path = ccode::rehydrate::rehydrate_into(root, session).unwrap();
            let lines: Vec<_> =
                CcodeSession::open(id, path, pool()).read().try_collect().await.unwrap();
            (rows_of(&lines), links(&session.id, &lines))
        }
        AnyHarness::Codex(_) => {
            let path = codex::rehydrate::write(root, session).unwrap();
            let lines: Vec<_> =
                CodexSession::open(id, path, pool()).read().try_collect().await.unwrap();
            (rows_of(&lines), links(&session.id, &lines))
        }
        AnyHarness::Pi(_) => {
            let path =
                pi::rehydrate::rehydrate_into(root, &root.join("sessions"), session).unwrap();
            let lines: Vec<_> =
                PiSession::open(id, path, pool()).read().try_collect().await.unwrap();
            (rows_of(&lines), links(&session.id, &lines))
        }
        AnyHarness::Opencode(_) => {
            let db = root.join("opencode.db");
            let mut conn = if db.exists() {
                let opts = sqlx::sqlite::SqliteConnectOptions::new().filename(&db);
                <sqlx::SqliteConnection as sqlx::Connection>::connect_with(&opts).await.unwrap()
            } else {
                database(&db).await
            };
            let export = opencode::rehydrate::export(session);
            opencode_import(&mut conn, &export, "/here").await;
            sqlx::Connection::close(conn).await.unwrap();
            let ids: HashSet<&str> = export["messages"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|m| m["parts"].as_array().unwrap())
                .filter_map(|p| p["id"].as_str())
                .collect();
            let rows: Vec<_> = captured(&db)
                .await
                .into_iter()
                .filter(|r| ids.contains(r.source_id.as_str()))
                .collect();
            let links = rows
                .iter()
                .filter_map(|r| linked_from_message(&Row(r.clone())))
                .map(|(_, id, kind)| (id, kind))
                .collect();
            (rows, links)
        }
    }
}

/// What `rows` say.
fn said(rows: &[RehydrateMessage]) -> Vec<(Role, Vec<Content>)> {
    rows.iter().map(|r| (r.role.clone(), r.content.clone())).collect()
}

/// A fork, written by its harness's writer next to the original and read back by its reader, is
/// a new session holding what the original's restore holds, and each of its lines that names a
/// session names the original as the one it is a fork of. Rows keep their ids where ids are the
/// transcript's own (capture keys rows per session), and opencode's, keys of its whole database,
/// are fresh: under the original's, the fork would import empty.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_reads_back_as_the_original_linked_as_its_fork(
    #[values(CLAUDE, CODEX, OPENCODE, PI)] harness: AnyHarness,
) {
    let dir = tempfile::tempdir().unwrap();
    let original = recorded(harness).await;
    let (restored, none) = written(harness, dir.path(), &original).await;
    assert!(none.is_empty(), "{none:?}");
    // pi names the parent by its file: the original's, written above.
    let path = matches!(harness, AnyHarness::Pi(_))
        .then(|| pi::session::locate(dir.path(), &original.id).unwrap());
    let of = ForkOf {
        id: original.id.clone(),
        atuin_id: Some(ATUIN_ID.to_owned()),
        path,
    };
    let forked = fork(harness, &original, of, None).unwrap();
    assert_ne!(forked.id, original.id);
    let (rows, links) = written(harness, dir.path(), &forked).await;

    assert!(!links.is_empty(), "the fork names its parent");
    assert!(links.iter().all(|l| *l == (original.id.clone(), ParentKind::Fork)), "{links:?}");
    let ids = |rows: &[RehydrateMessage]| -> HashSet<String> {
        rows.iter().map(|r| r.source_id.clone()).collect()
    };
    let mut said_rows = rows.clone();
    if matches!(harness, AnyHarness::Opencode(_)) {
        // The marker leads the first prompt, kept from the model; the rest is the original's,
        // under fresh ids.
        let text = fork_marker_text(harness, &original.id, Some(ATUIN_ID));
        let marker = vec![Content::Text(text)];
        let at = rows.iter().position(|r| r.content == marker).expect("the marker");
        assert_eq!(rows[at].role, Role::System);
        assert_eq!(rows[at + 1].role, Role::User, "then the prompt");
        said_rows.remove(at);
        assert!(ids(&rows).is_disjoint(&ids(&restored)));
    } else {
        // Bar the rows keyed on the session itself (its header, its title).
        let (fork, restored) = (ids(&rows), ids(&restored));
        let session =
            |id: &&String| *id == &forked.id || *id == &original.id || id.starts_with("atuin-");
        let differ: Vec<_> =
            fork.symmetric_difference(&restored).filter(|id| !session(id)).collect();
        assert!(differ.is_empty(), "{differ:?}");
    }
    pretty_assertions::assert_eq!(said(&said_rows), said(&restored));
}

/// A fork from a tip keeps the rows up to it, it included; a tip that is no row forks nothing.
#[rstest]
fn a_fork_from_a_tip_keeps_the_rows_up_to_it() {
    let original = {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(recorded(CLAUDE))
    };
    let tip = original.messages[2].source_id.clone();
    let of = || ForkOf {
        id: original.id.clone(),
        atuin_id: None,
        path: None,
    };
    let forked = fork(CLAUDE, &original, of(), Some(&tip)).unwrap();
    let kept: Vec<_> = forked.messages.iter().map(|m| m.source_id.as_str()).collect();
    let wanted: Vec<_> = original.messages[..3].iter().map(|m| m.source_id.as_str()).collect();
    assert_eq!(kept, wanted);
    assert!(fork(CLAUDE, &original, of(), Some("no such row")).is_none());
}

/// A Codex segment: the rollout a thread was reverted into, as capture names it after both.
const SEGMENT_ROLLOUT: &str = "01a0d160-0000-7000-8000-000000000001";

/// A fork of a segment of a Codex thread (a rollout it was reverted into, which holds only what
/// came after the revert) continues the history the segment's rollout does: its `history_base`,
/// numbered on from where that ends, when the rollout it names is here; without it, the fork
/// holds the segment's rows, as a restore would. Either way, Codex's own link names the thread,
/// and capture links the fork to the segment, not to the thread's first rollout.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_of_a_codex_segment_continues_its_history(#[values(true, false)] base_here: bool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let recorded = recorded(CODEX).await;
    let thread = recorded.id.clone();
    let segment = RehydrateSession {
        id: format!("{thread}_{SEGMENT_ROLLOUT}"),
        ..recorded
    };
    // The segment as Codex wrote it: its own rollout, naming the thread's first as its base.
    let path = codex::rehydrate::write(root, &segment).unwrap();
    let base = serde_json::json!({"thread_id": thread, "end_ordinal_exclusive": 12,
        "end_byte_offset": 4096});
    let text = std::fs::read_to_string(&path).unwrap();
    let (first, rest) = text.split_once('\n').unwrap();
    let mut meta: serde_json::Value = serde_json::from_str(first).unwrap();
    meta["payload"]["history_base"] = base.clone();
    std::fs::write(&path, format!("{meta}\n{rest}")).unwrap();
    if base_here {
        let first = root.join(format!("2026/09/24/rollout-2026-09-24T00-00-00-{thread}.jsonl"));
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::write(&first, "{}\n").unwrap();
    }

    let of = ForkOf {
        id: segment.id.clone(),
        atuin_id: Some(ATUIN_ID.to_owned()),
        path: Some(path),
    };
    let forked = fork(CODEX, &segment, of, None).unwrap();
    let written = codex::rehydrate::write(root, &forked).unwrap();
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&written)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let meta = &lines[0]["payload"];
    assert_eq!(meta["forked_from_id"], thread.as_str(), "Codex's link names the thread");
    let ordinals: Vec<u64> = lines.iter().map(|l| l["ordinal"].as_u64().unwrap()).collect();
    let first = if base_here {
        assert_eq!(meta["history_base"], base);
        12
    } else {
        assert!(meta["history_base"].is_null(), "{meta}");
        0
    };
    assert_eq!(ordinals, (first..).take(lines.len()).collect::<Vec<u64>>());

    let id = SessionId::from(forked.id.clone());
    let read: Vec<_> = CodexSession::open(id, written, pool()).read().try_collect().await.unwrap();
    let links = links(&forked.id, &read);
    assert!(!links.is_empty(), "the fork names its parent");
    assert!(links.iter().all(|l| *l == (segment.id.clone(), ParentKind::Fork)), "{links:?}");
}

/// An opencode session of only the model's replies (no prompt was captured) forks with its
/// marker in a user message of its own ahead of them, which opencode sends the model nothing of
/// (its only part is `ignored`; an assistant's text is sent, `ignored` or not), and capture still
/// reads the fork's link from it.
#[rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_opencode_fork_of_only_replies_names_its_original() {
    let dir = tempfile::tempdir().unwrap();
    let mut original = recorded(OPENCODE).await;
    original.messages.retain(|m| m.role == Role::Assistant);
    continuation::conversational(&original.messages).unwrap();
    let of = ForkOf {
        id: original.id.clone(),
        atuin_id: Some(ATUIN_ID.to_owned()),
        path: None,
    };
    let forked = fork(OPENCODE, &original, of, None).unwrap();

    let export = opencode::rehydrate::export(&forked);
    let messages = export["messages"].as_array().unwrap();
    let marker = fork_marker_text(OPENCODE, &original.id, Some(ATUIN_ID));
    let first = &messages[0];
    assert_eq!(first["info"]["role"], "user");
    let parts = first["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1, "{parts:?}");
    assert_eq!(parts[0]["text"], marker.as_str());
    assert_eq!(parts[0]["ignored"], true);
    assert!(messages[1..].iter().all(|m| m["info"]["role"] == "assistant"));
    assert_eq!(messages[1]["info"]["parentID"], first["info"]["id"], "the reply answers it");

    let (rows, links) = written(OPENCODE, dir.path(), &forked).await;
    assert_eq!(rows[0].content, vec![Content::Text(marker)]);
    assert_eq!(links, vec![(original.id.clone(), ParentKind::Fork)]);
}
