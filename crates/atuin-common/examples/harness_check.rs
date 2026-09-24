//! Reads every session a harness left under the current `HOME` (the usual default locations)
//! and fails unless the parser saw a whole conversation without errors.
//!
//! Used by `.github/workflows/harness-check.yml` against sessions a new harness release wrote
//! against a mock model: `cargo run -p atuin-common --features ai --example harness_check -- codex`

use std::collections::BTreeMap;
use std::process::ExitCode;

use atuin_common::harnesstools::session::model::{Content, Role};
use atuin_common::harnesstools::session::prelude::*;
use atuin_common::harnesstools::{AnyHarness, Harness};
use atuin_common::sync::BlockingPool;
use futures::StreamExt;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let name = std::env::args().nth(1).unwrap_or_default();
    let harness = match AnyHarness::from_name(&name) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let pool = BlockingPool::new(std::num::NonZeroUsize::MIN);
    let Some(sessions) = harness.sessions(&pool) else {
        eprintln!("{} has no session reader", harness.name());
        return ExitCode::FAILURE;
    };
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    let mut errors = Vec::new();
    let mut listed = match sessions.existing() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("listing sessions failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    while let Some(session) = listed.next().await {
        let session = match session {
            Ok(s) => s,
            Err(e) => {
                errors.push(format!("session: {e}"));
                continue;
            }
        };
        *seen.entry("session").or_default() += 1;
        let id = session.id();
        let mut messages = session.read();
        while let Some(message) = messages.next().await {
            let m = match message {
                Ok(m) => m,
                Err(e) => {
                    errors.push(format!("{id}: {e}"));
                    continue;
                }
            };
            if m.usage().is_some() {
                *seen.entry("usage").or_default() += 1;
            }
            for c in m.content() {
                let key = match (&m.role(), c) {
                    (Role::User, Content::Text(_)) => "user text",
                    (Role::Assistant, Content::Text(_)) => "assistant text",
                    (_, Content::ToolUse(_)) => "tool use",
                    (_, Content::ToolResult(_)) => "tool result",
                    _ => continue,
                };
                *seen.entry(key).or_default() += 1;
            }
        }
    }
    println!("{}: {seen:?}", harness.name());
    let required = ["session", "user text", "assistant text", "tool use", "tool result", "usage"];
    let missing: Vec<_> = required.iter().filter(|k| !seen.contains_key(*k)).collect();
    for e in &errors {
        eprintln!("error: {e}");
    }
    if !missing.is_empty() {
        eprintln!("parser found none of: {missing:?}");
    }
    if errors.is_empty() && missing.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
