//! Full-text search over captured output, end to end through the RPCs: register output the way a
//! shell does, then find it through `SearchCommandOutput` the way `atuin search-output` does.

#![cfg(unix)]

mod common;

use common::TestEnv;
use easy_cast::Conv;
use futures::TryStreamExt;
use rstest::*;

#[fixture]
async fn env() -> TestEnv {
    TestEnv::builder().build().await
}

#[rstest]
#[tokio::test]
async fn search_returns_the_visible_output_and_where_it_matched(#[future(awt)] env: TestEnv) {
    let mut history = env.history_client().await;
    let id = env.record(&mut history, "cargo build").await;
    // Colourised, as a terminal capture is: the escapes must not reach the client, and the ranges
    // must land on the visible text.
    let output = "\x1b[31merror\x1b[0m: disk full\nnext line";
    history
        .register_command_output(id, output, None, u64::conv(output.len()), 80, 24)
        .await
        .unwrap();

    let mut search = env.search_client().await;
    let matches: Vec<_> = search
        .search_command_output("disk".to_string(), None, Some(1))
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(matches.len(), 1);
    let m = &matches[0];
    assert_eq!(m.history_id, id);
    let lines: Vec<(i64, String)> =
        m.lines.iter().map(|l| (l.line, l.content.display_plain().to_string())).collect();
    assert_eq!(lines, vec![(0, "error: disk full".to_owned()), (1, "next line".to_owned())]);
    let plain = m.lines[0].content.to_plain();
    assert_eq!(&plain.text[plain.ranges[0].clone()], "disk");

    // Deleting the entry drops it from search along with its output.
    assert_eq!(history.delete_history(vec![id]).await.unwrap().deleted, 1);
    let remaining: Vec<_> = search
        .search_command_output("disk".to_string(), None, Some(1))
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(remaining.is_empty());
}
