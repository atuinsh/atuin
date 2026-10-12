use rstest::rstest;
use serde_json::{Value, json};

use super::*;
use crate::harnesstools::session::{ToolResult, ToolUse};

fn row(id: &str, role: Role, content: Vec<Content>) -> RehydrateMessage {
    RehydrateMessage {
        source_id: id.to_owned(),
        parent_source_id: None,
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

fn text(t: &str) -> Content {
    Content::Text(t.to_owned())
}

fn call(id: &str, name: &str, input: Value) -> Content {
    Content::ToolUse(ToolUse {
        id: ToolCallId::from(id.to_owned()),
        name: name.to_owned(),
        input,
    })
}

fn result(id: &str) -> Content {
    Content::ToolResult(ToolResult {
        call: ToolCallId::from(id.to_owned()),
        output: Value::Null,
        error: false,
    })
}

fn contents(rows: &[RehydrateMessage]) -> Vec<(&str, Vec<Content>)> {
    rows.iter().map(|m| (m.source_id.as_str(), m.content.clone())).collect()
}

/// A Claude Code turn as capture syncs it: its text, calls without their input, each result on a
/// line of its own, and the answer after them.
fn turn() -> Vec<RehydrateMessage> {
    let mut rows = vec![
        row("u1", Role::User, vec![text("fix the tests")]),
        row("a1", Role::Assistant, vec![text("Looking.")]),
        row("a2", Role::Assistant, vec![call("c1", "Bash", Value::Null)]),
        row("t1", Role::Tool, vec![result("c1")]),
        row("a3", Role::Assistant, vec![call("c2", "Bash", Value::Null)]),
        row("t2", Role::Tool, vec![result("c2")]),
        row("a4", Role::Assistant, vec![Content::ReasoningSummary { tokens: None }]),
        row("a5", Role::Assistant, vec![call("c3", "Edit", Value::Null)]),
        row("t3", Role::Tool, vec![result("c3")]),
        row("a6", Role::Assistant, vec![text("Fixed.")]),
        row("u2", Role::User, vec![text("thanks")]),
    ];
    for id in ["a2", "a3", "a5"] {
        rows.iter_mut().find(|m| m.source_id == id).unwrap().stop_reason =
            Some(StopReason::ToolUse);
    }
    rows
}

/// Merged by runs, a turn is one assistant row, the first, holding the text and the notes in
/// order (repeats counted), and the rows merged into it and the results are left empty.
#[rstest]
fn runs_merge_into_their_first_row() {
    let rows = flatten_uncaptured_calls(&turn(), &Flatten::Runs);
    assert_eq!(contents(&rows), vec![
        ("u1", vec![text("fix the tests")]),
        ("a1", vec![text("Looking.\n\n[ran a shell command] ×2\n[edited a file]\n\nFixed.")]),
        ("a2", vec![]),
        ("t1", vec![]),
        ("a3", vec![]),
        ("t2", vec![]),
        ("a4", vec![]),
        ("a5", vec![]),
        ("t3", vec![]),
        ("a6", vec![]),
        ("u2", vec![text("thanks")]),
    ]);
    // The turn no longer ends on a call.
    assert_eq!(rows[1].stop_reason, None);
}

/// A run ending on a call it no longer makes ends the turn instead.
#[rstest]
fn a_run_ending_on_a_flattened_call_ends_the_turn() {
    let mut rows = turn();
    rows.truncate(5);
    let rows = flatten_uncaptured_calls(&rows, &Flatten::Runs);
    assert_eq!(rows[1].content, vec![text("Looking.\n\n[ran a shell command] ×2")]);
    assert_eq!(rows[1].stop_reason, Some(StopReason::EndTurn));
}

/// Joined as notes, each note goes to the text before it in the turn, and the rows after it stay
/// as they were.
#[rstest]
fn notes_join_the_text_before_them() {
    let rows = flatten_uncaptured_calls(&turn(), &Flatten::Notes {
        host: &|_, _| true,
        own: &|_| true,
    });
    assert_eq!(contents(&rows), vec![
        ("u1", vec![text("fix the tests")]),
        ("a1", vec![text("Looking.\n\n[ran a shell command] ×2\n[edited a file]")]),
        ("a2", vec![]),
        ("t1", vec![]),
        ("a3", vec![]),
        ("t2", vec![]),
        ("a4", vec![Content::ReasoningSummary { tokens: None }]),
        ("a5", vec![]),
        ("t3", vec![]),
        ("a6", vec![text("Fixed.")]),
        ("u2", vec![text("thanks")]),
    ]);
}

/// With no text to join, a note takes its call's row, and the notes after it join it; a row that
/// can't change is left without its call.
#[rstest]
#[case::own_row(true, vec![
    ("u1", vec![text("go")]),
    ("a1", vec![text("[ran a shell command]\n[read a file]")]),
    ("t1", vec![]),
    ("a2", vec![]),
    ("t2", vec![]),
])]
#[case::no_row_to_change(false, vec![
    ("u1", vec![text("go")]),
    ("a1", vec![]),
    ("t1", vec![]),
    ("a2", vec![]),
    ("t2", vec![]),
])]
fn a_note_without_text_before_it_takes_its_calls_row(
    #[case] own: bool,
    #[case] expected: Vec<(&str, Vec<Content>)>,
) {
    let rows = vec![
        row("u1", Role::User, vec![text("go")]),
        row("a1", Role::Assistant, vec![call("c1", "shell", Value::Null)]),
        row("t1", Role::Tool, vec![result("c1")]),
        row("a2", Role::Assistant, vec![call("c2", "read", Value::Null)]),
        row("t2", Role::Tool, vec![result("c2")]),
    ];
    let rows = flatten_uncaptured_calls(&rows, &Flatten::Notes {
        host: &|_, _| true,
        own: &|_| own,
    });
    assert_eq!(contents(&rows), expected);
}

/// A note never joins a row `host` refuses, nor text across a prompt or a call kept whole.
#[rstest]
#[case::refused(vec![
    row("a1", Role::Assistant, vec![text("hi")]),
    row("a2", Role::Assistant, vec![call("c1", "bash", Value::Null)]),
], "a1", vec![text("hi")])]
#[case::across_a_prompt(vec![
    row("a1", Role::Assistant, vec![text("hi")]),
    row("u1", Role::User, vec![text("go on")]),
    row("a2", Role::Assistant, vec![call("c1", "bash", Value::Null)]),
], "-", vec![text("hi")])]
#[case::across_a_kept_call(vec![
    row("a1", Role::Assistant, vec![text("hi")]),
    row("a2", Role::Assistant, vec![call("c0", "bash", json!({"command": "ls"}))]),
    row("t0", Role::Tool, vec![result("c0")]),
    row("a3", Role::Assistant, vec![call("c1", "bash", Value::Null)]),
], "-", vec![text("hi")])]
fn notes_join_only_text_they_follow(
    #[case] rows: Vec<RehydrateMessage>,
    #[case] refused: &str,
    #[case] first: Vec<Content>,
) {
    let rows = flatten_uncaptured_calls(&rows, &Flatten::Notes {
        host: &|host, _| host.source_id != refused,
        own: &|_| true,
    });
    assert_eq!(rows[0].content, first);
    assert_eq!(rows.last().unwrap().content, vec![text("[ran a shell command]")]);
}

/// A call kept with its input stays a call, with its result; only the uncaptured one is a note.
#[rstest]
#[case::runs(Flatten::Runs)]
#[case::notes(Flatten::Notes { host: &|_, _| true, own: &|_| true })]
fn calls_kept_with_their_input_stay_calls(#[case] how: Flatten<'_>) {
    let kept = call("c0", "Bash", json!({"command": "ls"}));
    let rows = vec![
        row("u1", Role::User, vec![text("go")]),
        row("a1", Role::Assistant, vec![text("ok"), kept.clone(), call("c1", "Bash", Value::Null)]),
        row("t0", Role::Tool, vec![result("c0")]),
        row("t1", Role::Tool, vec![result("c1")]),
    ];
    let rows = flatten_uncaptured_calls(&rows, &how);
    assert_eq!(contents(&rows)[1..], [
        ("a1", vec![text("ok"), kept, text("[ran a shell command]")]),
        ("t0", vec![result("c0")]),
        ("t1", vec![]),
    ]);
}

/// A failed call's row is flattened where it is, never merged: its error would fail the turn.
#[rstest]
fn a_failed_row_is_not_merged() {
    let rows = vec![
        row("a1", Role::Assistant, vec![text("trying")]),
        row("a2", Role::Assistant, vec![
            call("c1", "Bash", Value::Null),
            Content::Error("overloaded".to_owned()),
        ]),
        row("a3", Role::Assistant, vec![call("c2", "Read", Value::Null)]),
    ];
    let rows = flatten_uncaptured_calls(&rows, &Flatten::Runs);
    assert_eq!(contents(&rows), vec![
        ("a1", vec![text("trying\n\n[read a file]")]),
        ("a2", vec![text("[ran a shell command]"), Content::Error("overloaded".to_owned())]),
        ("a3", vec![]),
    ]);
}

/// Without a call to flatten, nothing changes, whatever the writer.
#[rstest]
fn nothing_to_flatten_changes_nothing() {
    let rows = vec![
        row("a1", Role::Assistant, vec![text("a")]),
        row("a2", Role::Assistant, vec![call("c0", "Bash", json!({"command": "ls"}))]),
        row("t0", Role::Tool, vec![result("c0")]),
        row("a3", Role::Assistant, vec![text("b")]),
    ];
    let flattened = flatten_uncaptured_calls(&rows, &Flatten::Runs);
    assert_eq!(contents(&flattened), contents(&rows));
}
