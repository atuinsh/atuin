//! Full-text search over captured output, end to end through the RPCs: register output the way a
//! shell does, then find it through `SearchCommandOutput` the way `atuin search-output` does.

#![cfg(unix)]

mod common;

use std::ops::Range;

use atuin_client::history::HistoryId;
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
    let matches = search.search_command_output("disk", 0).await.unwrap();
    assert_eq!(matches.len(), 1);
    let m = &matches[0];
    assert_eq!(HistoryId::try_from(m.history_id.clone().unwrap()).unwrap(), id);
    assert_eq!(m.output, "error: disk full\nnext line");
    let ranges: Vec<Range<usize>> =
        m.matches.iter().map(|r| Range::try_from(*r).unwrap()).collect();
    assert_eq!(ranges, vec![7..11]);
    assert_eq!(&m.output[ranges[0].clone()], "disk");

    // Deleting the entry drops it from search along with its output.
    assert_eq!(history.delete_history(vec![id]).await.unwrap().deleted, 1);
    assert!(search.search_command_output("disk", 0).await.unwrap().is_empty());
}
