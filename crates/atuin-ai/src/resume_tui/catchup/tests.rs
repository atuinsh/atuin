use atuin_client::ai_session::Message;
use atuin_common::harnesstools::session::{Content, Role};
use atuin_domain::record::HostId;
use rstest::rstest;
use time::Duration;

use super::*;
use crate::resume_tui::fake::{
    self, FakeSource, local_tip as tip, synced_handle as handle, synced_row, synced_rows as rows,
};

async fn analysis(rows: Vec<Message>) -> Analysis {
    let source = FakeSource::from_rows(Vec::new()).with_synced(&handle(), rows);
    source.analyse(&handle()).await.unwrap().unwrap()
}

/// A step, as the rows it appends (`ff:c,d@b`, from where) or its kind.
fn summary(step: &Step) -> String {
    match step {
        Step::AsIs => "as is".to_owned(),
        Step::Ahead => "ahead".to_owned(),
        Step::FastForward { rows, base } => {
            let ids: Vec<&str> = rows.iter().map(|r| r.source_id.as_str()).collect();
            format!("ff:{}@{}", ids.join(","), base.tip_source_id.as_deref().unwrap_or("-"))
        }
        Step::Choice(why) => format!("{why:?}"),
    }
}

#[rstest]
#[case::at_the_head(false, &["a", "b", "c", "d"], "d", "as is")]
#[case::behind(false, &["a", "b"], "b", "ff:c,d@b")]
#[case::past_the_head_synced(false, &["a", "b", "c", "d", "e"], "e", "ahead")]
#[case::past_the_head_unsynced(false, &["a", "b", "c", "d", "z"], "z", "Unsynced")]
#[case::rewound_here_past_the_head(false, &["a", "b", "c", "d", "r"], "r", "Diverged")]
#[case::a_line_of_its_own_unsynced(false, &["a", "b", "z"], "z", "Unsynced")]
#[case::on_another_branch(true, &["a", "b", "x", "y"], "y", "Diverged")]
#[case::behind_on_the_other_branch(true, &["a", "b", "x"], "x", "Diverged")]
#[tokio::test]
async fn a_copy_is_classified_against_the_head(
    #[case] diverged: bool,
    #[case] known: &[&str],
    #[case] at: &str,
    #[case] want: &str,
) {
    let mut rows = rows(diverged);
    // Synced rows of this host's: one past the head, and a prompt rewound to before it.
    rows.push(synced_row(&handle(), ("e", Some("d"), 1, 6, false)));
    rows.push(synced_row(&handle(), ("r", Some("b"), 1, 7, false)));
    let analysis = analysis(rows).await;
    let head = SourceId::from("d".to_owned());
    let step = classify(&analysis, HarnessKind::ClaudeCode, &head, &tip(known, Some(at)));
    assert_eq!(summary(&step), want);
}

/// A copy that stopped on a tool result off the head's line goes on from where it leaves it, when
/// the rows appended hang from the row they follow; one that stopped on a compaction's summary
/// would lose it, and is a branch of its own. A harness that reads its transcript in order would
/// keep the twig: a choice too.
#[rstest]
#[case::a_tool_result(HarnessKind::ClaudeCode, Role::Tool, None, "ff:c,d@b")]
#[case::a_tool_result_in_pi(HarnessKind::Pi, Role::Tool, None, "ff:c,d@b")]
#[case::a_summary(HarnessKind::Pi, Role::System, Some("what went before"), "Diverged")]
#[case::in_order(HarnessKind::Codex, Role::Tool, None, "Diverged")]
#[tokio::test]
async fn a_twig_off_the_heads_line_fast_forwards(
    #[case] harness: HarnessKind,
    #[case] role: Role,
    #[case] summary_text: Option<&str>,
    #[case] want: &str,
) {
    let mut rows = rows(false);
    let mut twig = synced_row(&handle(), ("t", Some("b"), 1, 1, false));
    twig.role = role;
    twig.content = summary_text.map(|t| Content::Summary(t.to_owned())).into_iter().collect();
    rows.push(twig);
    let analysis = analysis(rows).await;
    let head = SourceId::from("d".to_owned());
    let step = classify(&analysis, harness, &head, &tip(&["a", "b", "t"], Some("t")));
    assert_eq!(summary(&step), want);
}

/// Each head is offered with what this copy lacks of it, and when it was last written.
#[rstest]
#[tokio::test]
async fn branches_say_what_they_add_and_when() {
    let analysis = analysis(rows(true)).await;
    let copy = tip(&["a", "b", "x", "y"], Some("y"));
    let branches = branches(&analysis, Some(&copy), fake::THIS_HOST_ID);
    let lines: Vec<String> = branches.iter().map(|b| b.line(fake::now())).collect();
    assert_eq!(lines, [
        "fork this machine's · 55m ago",
        "fork @00000002's · +2 since they split · 57m ago",
    ]);
    assert_eq!((branches[0].selector.as_str(), branches[1].selector.as_str()), ("y", "d"));
}

#[rstest]
#[case::live(Why::Live, "Claude Code is running this session here")]
#[case::unsynced(Why::Unsynced, "this copy has messages sync hasn't got")]
#[case::diverged(Why::Diverged, "this copy went another way than @00000002's")]
#[case::refused(Why::Refused("it changed".into()), "couldn't catch up: it changed")]
#[tokio::test]
async fn a_choice_says_why(#[case] why: Why, #[case] want: &str) {
    let analysis = analysis(rows(false)).await;
    let held = Held {
        why,
        harness: HarnessKind::ClaudeCode,
        plan: ResumePlan {
            program: "claude".to_owned(),
            args: Vec::new(),
            cwd: None,
            cwd_requirement: atuin_common::harnesstools::resume::CwdRequirement::Preferred,
            native_path: None,
        },
        branches: branches(&analysis, None, fake::THIS_HOST_ID),
        chosen: 0,
    };
    assert_eq!(held.status(), want);
    assert_eq!(caught_up(1, "@00000002"), "caught up: 1 message from @00000002");
    assert_eq!(caught_up(3, "@00000002"), "caught up: 3 messages from @00000002");
}

/// Forking from a head starts from the rows root to it; without one, a diverged session forks
/// from its newest head, and one that went one way from every row synced.
#[rstest]
#[case::a_head(true, Some("d"), Some(&["a", "b", "c", "d"][..]))]
#[case::the_newest(true, None, Some(&["a", "b", "x", "y"][..]))]
#[case::one_way(false, None, None)]
#[tokio::test]
async fn forks_start_from_a_heads_rows(
    #[case] diverged: bool,
    #[case] head: Option<&str>,
    #[case] want: Option<&[&str]>,
) {
    let source = FakeSource::from_rows(Vec::new()).with_synced(&handle(), rows(diverged));
    let head = head.map(|h| SourceId::from(h.to_owned()));
    let from = fork_from(&source, &handle(), head.as_ref()).await.unwrap();
    let ids: Option<Vec<&str>> =
        from.rows.as_ref().map(|rows| rows.iter().map(|r| r.source_id.as_str()).collect());
    assert_eq!(ids.as_deref(), want);
}

/// A fork from a head brings the rows off the tree that go with its branch: a prompt and reply
/// from before the session had ids (a Pi session from before them), and another host's reply on
/// its own branch only.
#[rstest]
#[case::a_head(Some("d"), &["syn-p", "syn-r", "a", "b", "c", "d", "syn-d"])]
#[case::the_newest(None, &["syn-p", "syn-r", "a", "b", "x", "y"])]
#[tokio::test]
async fn forks_bring_the_rows_off_the_tree_on_their_branch(
    #[case] head: Option<&str>,
    #[case] want: &[&str],
) {
    let mut synced = rows(true);
    synced.extend([
        synced_row(&handle(), ("syn-p", None, 1, -2, false)),
        synced_row(&handle(), ("syn-r", None, 1, -1, true)),
        synced_row(&handle(), ("syn-d", None, 2, 4, true)),
    ]);
    let source = FakeSource::from_rows(Vec::new()).with_synced(&handle(), synced);
    let head = head.map(|h| SourceId::from(h.to_owned()));
    let from = fork_from(&source, &handle(), head.as_ref()).await.unwrap();
    let ids: Vec<&str> = from.rows.as_ref().unwrap().iter().map(|r| r.source_id.as_str()).collect();
    assert_eq!(ids, want);
}

fn head(id: &str, host: u128, minutes_ago: i64, messages: u64) -> Head {
    Head {
        source_id: id.to_owned().into(),
        host: Some(HostId(uuid::Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 | host))),
        last_at: fake::now() - Duration::minutes(minutes_ago),
        messages,
    }
}

/// A branch's selector is the shortest start of its id that no other branch shares, eight
/// characters at least, or the whole id; and it picks that branch back.
#[rstest]
#[case::uuid(&["7aaabc31-1631-4756", "0c1d2e3f-8a9b-4c5d"], "7aaabc31")]
#[case::sharing_a_long_start(&["prt_fcf1e6fc6001aa", "prt_fcf1e6fc9002bb"], "prt_fcf1e6fc6")]
#[case::a_start_of_another(&["b4", "b40"], "b4")]
fn a_selector_is_the_shortest_start_no_other_branch_shares(
    #[case] ids: &[&str],
    #[case] want: &str,
) {
    let heads: Vec<Head> = ids.iter().map(|id| head(id, 1, 0, 1)).collect();
    let selector = branch_selector(&heads, &heads[0]);
    assert_eq!(selector, want);
    let picked = pick_branch(&heads, &selector, fake::now(), fake::THIS_HOST_ID).unwrap();
    assert_eq!(picked, &heads[0]);
}

/// `--branch` takes `this`, `@host` (its short or full id) or the start of a head's id; anything
/// naming no single head is an error listing them.
#[rstest]
#[case::this("this", Ok("h1"))]
#[case::host("@00000002", Ok("h2"))]
#[case::full_host_id("@01900000-0000-7000-8000-000000000002", Ok("h2"))]
#[case::id("h3", Ok("h3"))]
#[case::unknown_host("@buildbox", Err("no branch is \"@buildbox\"; its branches:"))]
#[case::ambiguous_id("h", Err("\"h\" names 3 branches; name one by id:"))]
fn branches_are_picked_by_host_or_id(#[case] selector: &str, #[case] want: Result<&str, &str>) {
    let heads = [head("h2", 2, 60, 40), head("h1", 1, 120, 24), head("h3", 3, 180, 5)];
    let picked = pick_branch(&heads, selector, fake::now(), fake::THIS_HOST_ID);
    match want {
        Ok(id) => assert_eq!(picked.unwrap().source_id.as_ref(), id),
        Err(start) => {
            let why = picked.unwrap_err();
            assert!(why.starts_with(start), "{why}");
            let listing = "\n  h2  @00000002 · 1h · 40 msgs (latest)\n  h1  this machine · 2h · \
                           24 msgs\n  h3  @00000003 · 3h · 5 msgs";
            assert!(why.ends_with(listing), "{why}");
        }
    }
    assert!(pick_branch(&[], "this", fake::now(), fake::THIS_HOST_ID).is_err());
}
