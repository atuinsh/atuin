//! Tails live opencode sessions, the way a consumer of this module would.
//!
//! Run with `--all` to replay the whole event log first.

use std::collections::BTreeMap;

use atuin_common::db::sqlite::observe::Replay;
use atuin_common::harnesstools::opencode::session::OpencodeSessions;
use atuin_common::harnesstools::session::SessionEvent;
use atuin_common::harnesstools::session::model::Content;
use atuin_common::harnesstools::session::prelude::*;
use futures::StreamExt;

fn summarize(content: &[Content]) -> String {
    content
        .iter()
        .map(|part| match part {
            Content::Text(text) => format!("text {:?}", elide(text)),
            Content::Reasoning(text) => format!("reasoning {:?}", elide(text)),
            Content::ToolUse(use_) => format!("tool_use {} {}", use_.name, use_.id.as_ref()),
            Content::ToolResult(result) => format!(
                "tool_result {} {}",
                result.call.as_ref(),
                if result.error {
                    "error"
                } else {
                    "ok"
                }
            ),
            Content::ReasoningSummary { tokens } => {
                format!("reasoning_summary {tokens:?}")
            }
            Content::Other(value) => {
                format!("other {}", value.get("type").and_then(|t| t.as_str()).unwrap_or("?"))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn elide(text: &str) -> String {
    let flat = text.replace('\n', "⏎");
    if flat.chars().count() <= 60 {
        return flat;
    }
    format!("{}…", flat.chars().take(60).collect::<String>())
}

#[tokio::main]
async fn main() {
    let all = std::env::args().any(|arg| arg == "--all");

    let sessions = OpencodeSessions::builder()
        .replay(if all {
            Replay::All
        } else {
            Replay::FromNow
        })
        .build();
    let listener = match sessions.listener() {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("cannot watch opencode: {err}");
            std::process::exit(1);
        }
    };
    println!(
        "tailing {} -- run opencode now (ctrl-c to stop)",
        if all {
            "everything"
        } else {
            "new rows"
        }
    );

    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut events = Box::pin(listener.events(|_| async { 0 }, |_, _| async { true }));
    while let Some(event) = events.next().await {
        match event {
            Ok(SessionEvent {
                session, message, ..
            }) => {
                let n = seen.entry(session.to_string()).or_default();
                *n += 1;
                if *n == 1 {
                    println!("\n== session {session}");
                }
                println!(
                    "{session} #{n} id={} rev={} role={:?} at={} :: {}",
                    message.id().map_or_else(|| "-".to_owned(), |id| id.to_string()),
                    message.revision().map_or_else(|| "-".to_owned(), |r| r.to_string()),
                    message.role(),
                    message.timestamp().map_or_else(|| "-".to_owned(), |t| t.time().to_string()),
                    summarize(&message.content()),
                );
            }
            Err(err) => eprintln!("!! {err}"),
        }
    }
    println!("stream ended");
}
