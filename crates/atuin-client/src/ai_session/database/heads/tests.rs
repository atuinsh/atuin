use atuin_common::harnesstools::session::{Content, Role};
use atuin_domain::record::{HostId, RecordId};
use proptest::prelude::*;
use rstest::rstest;
use time::OffsetDateTime;

use super::Analysis;
use crate::ai_session::{
    AiSessionDatabase, HarnessKind, HarnessSession, Message, NativeSessionId, SourceId,
};

/// What a row is, as far as branches go.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// A user prompt.
    Prompt,
    /// Assistant text.
    Reply,
    /// A tool call or its result: substantive neither.
    Tool,
    /// A Pi session header.
    Header,
}
use Kind::{Header, Prompt, Reply, Tool};

/// A row: its source id, parent, host, timestamp (ms) and kind.
type Row<'a> = (&'a str, Option<&'a str>, u8, i64, Kind);

fn host(n: u8) -> HostId {
    HostId(uuid::Uuid::from_u128(u128::from(n)))
}

fn handle(harness: HarnessKind) -> HarnessSession {
    HarnessSession {
        harness,
        session: NativeSessionId::from("s".to_owned()),
    }
}

fn message(harness: HarnessKind, (id, parent, on, at, kind): Row<'_>) -> Message {
    let (role, content) = match kind {
        Prompt => (Role::User, vec![Content::Text(format!("hi {id}"))]),
        Reply => (Role::Assistant, vec![Content::Text(format!("ok {id}"))]),
        Tool => (Role::Tool, vec![Content::Other(serde_json::json!(id))]),
        Header => (Role::Other("session".to_owned()), Vec::new()),
    };
    Message::builder()
        .id(RecordId(atuin_common::utils::uuid_v7()))
        .session(handle(harness))
        .source_id(SourceId::from(id.to_owned()))
        .parent_source_id(parent.map(|p| SourceId::from(p.to_owned())))
        .timestamp(OffsetDateTime::UNIX_EPOCH + time::Duration::milliseconds(at))
        .role(role)
        .content(content)
        .host(Some(host(on)))
        .build()
}

fn analyse(harness: HarnessKind, rows: &[Row<'_>]) -> Analysis {
    Analysis::new(harness, rows.iter().map(|&r| message(harness, r)).collect())
}

/// (tip, messages) of each head, newest first.
fn tips(analysis: &Analysis) -> Vec<(String, u64)> {
    analysis.heads().iter().map(|h| (h.source_id.to_string(), h.messages)).collect()
}

fn path(analysis: &Analysis, to: &str) -> Vec<String> {
    let to = SourceId::from(to.to_owned());
    analysis.path_to(&to).iter().map(|m| m.source_id.to_string()).collect()
}

const CC: HarnessKind = HarnessKind::ClaudeCode;
const CODEX: HarnessKind = HarnessKind::Codex;
const OPENCODE: HarnessKind = HarnessKind::Opencode;

/// A base every Claude Code case grows from: a prompt, a reply and a tool call, on host 1.
const BASE: &[Row<'static>] =
    &[("u1", None, 1, 0, Prompt), ("a1", Some("u1"), 1, 1, Reply), ("t1", Some("a1"), 1, 2, Tool)];

#[rstest]
#[case::linear(CC, &[], &[("t1", 2)], false)]
// Parallel tool calls fork the tree without making a branch: the side holding only a tool call
// stays on the line, and the newest leaf is the tip.
#[case::tool_calls_continued(
    CC,
    &[("t2", Some("a1"), 1, 3, Tool), ("r1", Some("t1"), 1, 4, Tool), ("a2", Some("r1"), 1, 5, Reply)],
    &[("a2", 3)],
    false
)]
#[case::tool_calls_ended(
    CC,
    &[("t2", Some("a1"), 1, 3, Tool), ("r1", Some("t1"), 1, 4, Tool)],
    &[("r1", 2)],
    false
)]
// A reply beside a tool call stays on the line too, but is its tip even when a result of the tool
// call came later: the head's path holds the reply.
#[case::reply_beside_a_newer_tool_result(
    CC,
    &[("a2", Some("a1"), 1, 3, Reply), ("r1", Some("t1"), 1, 4, Tool)],
    &[("a2", 3)],
    false
)]
// A rewind on one host leaves two heads, newest first; one host's heads are no divergence.
#[case::rewind(
    CC,
    &[("u2", Some("t1"), 1, 3, Prompt), ("u2b", Some("t1"), 1, 10, Prompt), ("a2b", Some("u2b"), 1, 11, Reply)],
    &[("a2b", 4), ("u2", 3)],
    false
)]
// Two hosts continuing the same row are diverged.
#[case::two_hosts(
    CC,
    &[("u2", Some("t1"), 1, 10, Prompt), ("v2", Some("t1"), 2, 20, Prompt), ("b2", Some("v2"), 2, 21, Reply)],
    &[("b2", 4), ("u2", 3)],
    true
)]
// One host carrying on a line another left is not, even past a rewind of the first host.
#[case::carried_on_past_a_rewind(
    CC,
    &[("u2", Some("t1"), 1, 10, Prompt), ("v2", Some("t1"), 1, 20, Prompt), ("v3", Some("v2"), 2, 30, Prompt)],
    &[("v3", 4), ("u2", 3)],
    false
)]
// A side's host is the one that made it substantive, not whichever host's tool row came first:
// host 2 prompting below host 1's tool call, while host 1 rewound past it, is diverged ...
#[case::second_host_below_a_tool_row(
    CC,
    &[("u2", Some("a1"), 1, 10, Prompt), ("v2", Some("t1"), 2, 20, Prompt)],
    &[("v2", 3), ("u2", 3)],
    true
)]
// ... while host 1 prompting there itself is a rewind ...
#[case::rewind_below_a_tool_row(
    CC,
    &[("u2", Some("a1"), 1, 10, Prompt), ("u3", Some("t1"), 1, 20, Prompt)],
    &[("u3", 3), ("u2", 3)],
    false
)]
// ... and host 2 carrying on from there is no divergence either.
#[case::carried_on_below_a_tool_row(
    CC,
    &[("u2", Some("a1"), 1, 10, Prompt), ("u3", Some("t1"), 1, 20, Prompt), ("v4", Some("u3"), 2, 30, Prompt)],
    &[("v4", 4), ("u2", 3)],
    false
)]
// Titles and other content-addressed rows are no node of a Claude Code tree.
#[case::content_addressed_off_the_tree(CC, &[("syn-title", None, 1, 50, Prompt)], &[("t1", 2)], false)]
// A row whose parent was never captured continues the line rather than starting a branch.
#[case::missing_parent(
    CC,
    &[("u2", Some("not-captured"), 1, 10, Prompt), ("a2", Some("u2"), 1, 11, Reply)],
    &[("a2", 4)],
    false
)]
// ... from its own host's row before it, not another host's that came between.
#[case::missing_parent_follows_its_host(
    CC,
    &[("v2", Some("t1"), 2, 10, Prompt), ("u2", Some("not-captured"), 1, 20, Prompt)],
    &[("u2", 3), ("v2", 3)],
    true
)]
// A host's first row follows the row before it at all: it carries on what it restored.
#[case::missing_parent_first_on_its_host(
    CC,
    &[("v2", Some("not-captured"), 2, 10, Prompt), ("b2", Some("v2"), 2, 11, Reply)],
    &[("b2", 4)],
    false
)]
fn claude_code_heads(
    #[case] harness: HarnessKind,
    #[case] more: &[Row<'static>],
    #[case] heads: &[(&str, u64)],
    #[case] diverged: bool,
) {
    let rows: Vec<Row<'_>> = BASE.iter().chain(more).copied().collect();
    let analysis = analyse(harness, &rows);
    let want: Vec<(String, u64)> = heads.iter().map(|&(id, n)| (id.to_owned(), n)).collect();
    assert_eq!(tips(&analysis), want);
    assert_eq!(analysis.diverged(), diverged);
}

/// Codex rows name the row captured before them. Two hosts continuing the same row (say both
/// restored the session and carried on) part there; one host carrying on another's line follows
/// it. Rows captured before parents were (none) follow on from their host's row before them (or
/// the row before them at all, for a host's first), and new rows chain onto them.
#[rstest]
#[case::one_line(
    &[("meta", None, 1, 0, Tool), ("p0", Some("meta"), 1, 1, Prompt), ("r0", Some("p0"), 1, 2, Reply),
      ("p1", Some("r0"), 2, 10, Prompt), ("r1", Some("p1"), 2, 11, Reply)],
    &[("r1", 4)],
    false
)]
#[case::two_hosts_continue_one_row(
    &[("meta", None, 1, 0, Tool), ("p0", Some("meta"), 1, 1, Prompt), ("r0", Some("p0"), 1, 2, Reply),
      ("p1", Some("r0"), 2, 10, Prompt), ("r1", Some("p1"), 2, 11, Reply),
      ("q1", Some("r0"), 1, 20, Prompt), ("usage", Some("q1"), 1, 21, Tool)],
    &[("usage", 3), ("r1", 4)],
    true
)]
#[case::old_rows_in_time_order(
    &[("meta", None, 1, 0, Tool), ("p0", None, 1, 1, Prompt), ("r0", None, 1, 2, Reply),
      ("p1", Some("r0"), 2, 10, Prompt), ("q1", None, 2, 20, Prompt)],
    &[("q1", 4)],
    false
)]
#[case::old_rows_follow_their_host(
    &[("meta", None, 1, 0, Tool), ("p0", None, 1, 1, Prompt), ("r0", None, 1, 2, Reply),
      ("p1", None, 2, 10, Prompt), ("q1", None, 1, 20, Prompt)],
    &[("q1", 3), ("p1", 3)],
    true
)]
#[case::old_rows_then_two_hosts(
    &[("meta", None, 1, 0, Tool), ("p0", None, 1, 1, Prompt), ("r0", None, 1, 2, Reply),
      ("p1", Some("r0"), 2, 10, Prompt), ("q1", Some("r0"), 1, 20, Prompt)],
    &[("q1", 3), ("p1", 3)],
    true
)]
fn codex_heads(
    #[case] rows: &[Row<'static>],
    #[case] heads: &[(&str, u64)],
    #[case] diverged: bool,
) {
    let analysis = analyse(CODEX, rows);
    let want: Vec<(String, u64)> = heads.iter().map(|&(id, n)| (id.to_owned(), n)).collect();
    assert_eq!(tips(&analysis), want);
    assert_eq!(analysis.diverged(), diverged);
}

/// opencode is linear: one host carrying on from another continues it, a host coming back after
/// another's rows is interleaving, parting where it left off. Rows are `r0`, `r1`, ... in time
/// order, given as (host, kind); the path checked is the newest head's.
#[rstest]
#[case::handed_on(&[(1, Prompt), (1, Prompt), (2, Prompt), (2, Prompt)], &["r3"], &["r0", "r1", "r2", "r3"], false)]
#[case::interleaved(
    &[(1, Prompt), (1, Prompt), (2, Prompt), (1, Prompt), (2, Prompt)],
    &["r4", "r3"],
    &["r0", "r1", "r2", "r4"],
    true
)]
// Another host's tool row between a host's rows is on none of its paths, and, substantive
// neither, makes no head or divergence of its own.
#[case::tool_row_between(
    &[(1, Prompt), (1, Reply), (2, Prompt), (1, Tool), (2, Reply)],
    &["r4"],
    &["r0", "r1", "r2", "r4"],
    false
)]
#[case::late_tool_row(&[(1, Prompt), (2, Prompt), (1, Tool)], &["r1"], &["r0", "r1"], false)]
// A host whose first row there was a tool row was active from then on: its later prompt parts from
// the other host's reply that came between.
#[case::tool_row_opens_a_side(
    &[(1, Prompt), (2, Tool), (1, Reply), (2, Prompt)],
    &["r3", "r2"],
    &["r0", "r1", "r3"],
    true
)]
// A third host carries on the second's line; the first coming back parts from both.
#[case::three_hosts(
    &[(1, Prompt), (2, Prompt), (3, Prompt), (1, Prompt)],
    &["r3", "r2"],
    &["r0", "r3"],
    true
)]
fn opencode_parts_where_hosts_interleave(
    #[case] rows: &[(u8, Kind)],
    #[case] heads: &[&str],
    #[case] newest_path: &[&str],
    #[case] diverged: bool,
) {
    let ids: Vec<String> = (0..rows.len()).map(|i| format!("r{i}")).collect();
    let rows: Vec<Row<'_>> = rows
        .iter()
        .zip(&ids)
        .zip(0..)
        .map(|((&(on, kind), id), at)| (id.as_str(), None, on, at, kind))
        .collect();
    let analysis = analyse(OPENCODE, &rows);
    let tips: Vec<String> = analysis.heads().iter().map(|h| h.source_id.to_string()).collect();
    assert_eq!(tips, heads);
    assert_eq!(path(&analysis, heads[0]), newest_path);
    assert_eq!(analysis.diverged(), diverged);
}

/// A Pi header is never a tip while anything else can be; a session of nothing but
/// content-addressed rows is linear; a parent cycle (a corrupt transcript) is cut rather than
/// looping.
#[rstest]
#[case::header(
    HarnessKind::Pi,
    &[("hdr", None, 1, -1, Header), ("u1", None, 1, 0, Prompt), ("a1", Some("u1"), 1, 1, Reply)],
    "a1"
)]
#[case::only_content_addressed(HarnessKind::Pi, &[("syn-1", None, 1, 0, Prompt), ("syn-2", None, 1, 1, Reply)], "syn-2")]
#[case::cycle(
    CC,
    &[("a", Some("c"), 1, 0, Prompt), ("b", Some("a"), 1, 1, Reply), ("c", Some("b"), 1, 2, Prompt)],
    "c"
)]
fn odd_sessions_keep_one_head(
    #[case] harness: HarnessKind,
    #[case] rows: &[Row<'static>],
    #[case] tip: &str,
) {
    let analysis = analyse(harness, rows);
    assert_eq!(analysis.heads().len(), 1, "{:?}", analysis.heads());
    assert_eq!(analysis.heads()[0].source_id.as_ref(), tip);
}

/// A branch's path runs from the root to its head, through the rows it shares with the others;
/// a row off the tree has none. Read from the database, as fork and fast-forward will.
#[rstest]
#[tokio::test]
async fn a_stored_sessions_branches_and_their_paths() {
    let rows = [
        ("u1", None, 1, 0, Prompt),
        ("a1", Some("u1"), 1, 1, Reply),
        ("u2", Some("a1"), 1, 2, Prompt),
        ("v2", Some("a1"), 2, 10, Prompt),
        ("b2", Some("v2"), 2, 11, Reply),
        ("syn-title", None, 1, 12, Prompt),
    ];
    let db = AiSessionDatabase::in_memory().await.unwrap();
    for row in rows {
        db.append(&message(CC, row)).await.unwrap();
    }
    let analysis = db.analyse(&handle(CC)).await.unwrap();
    assert!(analysis.diverged());
    assert_eq!(tips(&analysis), [("b2".to_owned(), 4), ("u2".to_owned(), 3)]);
    assert_eq!(analysis.heads()[0].host, Some(host(2)));
    assert_eq!(path(&analysis, "b2"), ["u1", "a1", "v2", "b2"]);
    assert_eq!(path(&analysis, "a1"), ["u1", "a1"]);
    assert!(path(&analysis, "syn-title").is_empty());
    assert!(path(&analysis, "nope").is_empty());

    let empty = db.analyse(&handle(CODEX)).await.unwrap();
    assert!(empty.heads().is_empty());
}

/// An arbitrary forest of rows: each row's parent is an earlier one (or none, or one not
/// stored), on one of three hosts, of any kind.
fn arb_rows() -> impl Strategy<Value = Vec<(String, Option<String>, u8, i64, Kind)>> {
    let kind = prop_oneof![Just(Prompt), Just(Reply), Just(Tool)];
    prop::collection::vec((0..8_usize, 1..4_u8, kind, 0..4_i64, 0..6_u8), 1..24).prop_map(|specs| {
        specs
            .into_iter()
            .enumerate()
            .map(|(i, (up, on, kind, jitter, link))| {
                let parent = match link {
                    0 => None,
                    1 => Some("missing".to_owned()),
                    _ => i.checked_sub(up + 1).map(|p| format!("n{p:02}")),
                };
                let at = i64::try_from(i).unwrap() * 4 + jitter;
                (format!("n{i:02}"), parent, on, at, kind)
            })
            .collect()
    })
}

fn arb_harness() -> impl Strategy<Value = HarnessKind> {
    prop_oneof![Just(CC), Just(CODEX), Just(OPENCODE), Just(HarnessKind::Copilot)]
}

fn messages(
    harness: HarnessKind,
    rows: &[(String, Option<String>, u8, i64, Kind)],
) -> Vec<Message> {
    rows.iter()
        .map(|(id, parent, on, at, kind)| {
            message(harness, (id.as_str(), parent.as_deref(), *on, *at, *kind))
        })
        .collect()
}

proptest! {
    /// Heads depend only on the rows, never on the order they are handed over in; each head's
    /// path ends at it and holds as many messages as it says, and every substantive row is on some
    /// head's path; a divergence leaves two heads.
    #[rstest]
    fn heads_hold_their_invariants(
        rows in arb_rows(),
        harness in arb_harness(),
        seed in any::<u64>(),
    ) {
        let rows = messages(harness, &rows);
        let analysis = Analysis::new(harness, rows.clone());
        let mut shuffled = rows;
        let mut seed = seed;
        for i in (1..shuffled.len()).rev() {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            shuffled.swap(i, usize::try_from((seed >> 33) % (i as u64 + 1)).unwrap());
        }
        let again = Analysis::new(harness, shuffled);
        prop_assert_eq!(analysis.heads(), again.heads());
        prop_assert_eq!(analysis.diverged(), again.diverged());

        prop_assert!(!analysis.heads().is_empty());
        for pair in analysis.heads().windows(2) {
            prop_assert!(
                (pair[0].last_at, pair[0].source_id.as_ref()) >= (pair[1].last_at, pair[1].source_id.as_ref())
            );
        }
        for head in analysis.heads() {
            let path = analysis.path_to(&head.source_id);
            prop_assert_eq!(path.last().map(|m| &m.source_id), Some(&head.source_id));
            let messages = path
                .iter()
                .filter(|m| atuin_common::harnesstools::session::is_substantive(&m.role, &m.content))
                .count();
            prop_assert_eq!(head.messages, messages as u64);
        }
        let on_a_head: std::collections::HashSet<&SourceId> = analysis
            .heads()
            .iter()
            .flat_map(|h| analysis.path_to(&h.source_id))
            .map(|m| &m.source_id)
            .collect();
        for row in &analysis.rows {
            let substantive = atuin_common::harnesstools::session::is_substantive(&row.role, &row.content);
            prop_assert!(!substantive || on_a_head.contains(&row.source_id), "{} is on no head", row.source_id);
        }
        if analysis.diverged() {
            prop_assert!(analysis.heads().len() > 1, "{:?}", analysis.heads());
        }
    }
}
