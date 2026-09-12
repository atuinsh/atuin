//! Full-text search over captured output, end to end through the RPCs: register output the way a
//! shell does, then find it through `SearchCommandOutput` the way `atuin search-output` does.

#![cfg(unix)]

mod common;

use atuin_client::history::HistoryId;
use atuin_common::string::highlighted::HighlightedString;
use common::TestEnv;
use easy_cast::Conv;
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
    let mut stream = search.search_command_output("disk", 0).await.unwrap();
    let mut matches = Vec::new();
    while let Some(m) = stream.message().await.unwrap() {
        matches.push(m);
    }
    assert_eq!(matches.len(), 1);
    let m = &matches[0];
    assert_eq!(HistoryId::try_from(m.history_id.clone().unwrap()).unwrap(), id);
    let highlighted: HighlightedString = m.output.clone().unwrap().try_into().unwrap();
    assert_eq!(highlighted.display_plain().to_string(), "error: disk full\nnext line");
    let marked = highlighted.as_ref();
    let got: Vec<&str> = highlighted.ranges().map(|r| &marked[r]).collect();
    assert_eq!(got, vec!["disk"]);

    // Deleting the entry drops it from search along with its output.
    assert_eq!(history.delete_history(vec![id]).await.unwrap().deleted, 1);
    let mut stream = search.search_command_output("disk", 0).await.unwrap();
    assert!(stream.message().await.unwrap().is_none());
}
