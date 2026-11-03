//! Amp's threads as `amp threads export` gives them, for importing threads from before the
//! plugin, or turns it missed. The export's format is not documented: what is read of it is what
//! Amp 0.0.1792180867 writes. Its message ids (`protocolMessageID`) are the ones the plugin
//! pushes, so a thread imported after the plugin captured some of it stores only what's missing.

use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::process::Command;

use crate::harnesstools::session::Usage;
use crate::harnesstools::session::pushed::{Block, Known, PushedMessage, Source, Title, Version};

/// How many threads one `amp threads list` is asked for.
const PAGE: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("couldn't run amp")]
    Run(#[source] std::io::Error),
    #[error("amp failed: {0}")]
    Failed(String),
    #[error("couldn't read what amp printed")]
    Json(#[source] serde_json::Error),
}

/// Run `amp args`, its stdout as JSON.
async fn amp(args: &[&str]) -> Result<Value, ExportError> {
    let out = Command::new("amp").args(args).output().await.map_err(ExportError::Run)?;
    if !out.status.success() {
        return Err(ExportError::Failed(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
    }
    serde_json::from_slice(&out.stdout).map_err(ExportError::Json)
}

/// The ids of every thread of the user Amp is logged in as, archived ones included.
pub async fn threads() -> Result<Vec<String>, ExportError> {
    let mut ids = Vec::new();
    for offset in (0..).step_by(PAGE) {
        let (limit, offset) = (PAGE.to_string(), offset.to_string());
        let args = ["threads", "list", "--json", "--include-archived", "--limit", &limit];
        let page = amp(&[&args[..], &["--offset", &offset]].concat()).await?;
        let page = page.as_array().map(Vec::as_slice).unwrap_or_default();
        ids.extend(page.iter().filter_map(|t| t["id"].as_str()).map(str::to_owned));
        if page.len() < PAGE {
            break;
        }
    }
    Ok(ids)
}

/// Thread `id`'s messages.
pub async fn export(id: &str) -> Result<Vec<PushedMessage>, ExportError> {
    Ok(messages(&amp(&["threads", "export", id]).await?))
}

/// The messages of an exported `thread`, each following the one before it, then its title.
fn messages(thread: &Value) -> Vec<PushedMessage> {
    let cwd = thread["env"]["initial"]["trees"][0]["uri"]
        .as_str()
        .and_then(|uri| url::Url::parse(uri).ok()?.to_file_path().ok());
    let title = thread["title"].as_str();
    let raw = thread["messages"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut previous: Option<String> = None;
    let mut out = Vec::with_capacity(raw.len());
    for (n, m) in raw.iter().enumerate() {
        // Older threads' messages have no `protocolMessageID`, and the oldest no id at all.
        let id = m["protocolMessageID"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| m["messageId"].as_u64().map(|id| id.to_string()))
            .unwrap_or_else(|| format!("#{n}"));
        let usage = &m["usage"];
        let calls = usage.is_object();
        out.push(PushedMessage {
            v: Version,
            parent: previous.replace(id.clone()),
            role: match m["role"].as_str() {
                Some("info") => "system",
                Some(role) => role,
                None => "unknown",
            }
            .to_owned(),
            time: time(m),
            content: m["content"].as_array().into_iter().flatten().map(block).collect(),
            cwd: cwd.clone(),
            branch: None,
            model: usage["model"].as_str().map(str::to_owned),
            provider: None,
            turn: calls.then(|| id.clone()),
            usage: calls.then(|| Usage {
                input: usage["inputTokens"].as_u64(),
                output: usage["outputTokens"].as_u64(),
                cache_read: usage["cacheReadInputTokens"].as_u64(),
                cache_write: usage["cacheCreationInputTokens"].as_u64(),
                reasoning: None,
            }),
            stop: stop(&m["state"]),
            title: None,
            parent_session: None,
            parent_kind: None,
            id: Some(id),
        });
    }
    // A row of its own, without an id, so it is keyed on the title and when the thread last
    // changed: a message the plugin already stored would carry it as a duplicate, and drop it.
    if let Some(text) = title {
        let time = parsed(&thread["updatedAt"]).or_else(|| out.last().and_then(|m| m.time));
        out.push(PushedMessage {
            role: "system".to_owned(),
            time,
            title: Some(Title {
                text: Some(text.to_owned()),
                source: Source::Generated,
            }),
            v: Version,
            id: None,
            parent: None,
            content: Vec::new(),
            cwd: None,
            branch: None,
            model: None,
            provider: None,
            turn: None,
            usage: None,
            stop: None,
            parent_session: None,
            parent_kind: None,
        });
    }
    out
}

fn parsed(v: &Value) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(v.as_str()?, &Rfc3339).ok()
}

/// When `m` was written: its `createdAt`, else its model call's time, else when it was sent.
fn time(m: &Value) -> Option<OffsetDateTime> {
    parsed(&m["createdAt"]).or_else(|| parsed(&m["usage"]["timestamp"])).or_else(|| {
        let ms = m["meta"]["sentAt"].as_i64()?;
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()
    })
}

/// Why an assistant message ended, as `atuin threads push` names it.
fn stop(state: &Value) -> Option<String> {
    match state["type"].as_str()? {
        "cancelled" => Some("aborted".to_owned()),
        "error" => Some("error".to_owned()),
        _ => state["stopReason"].as_str().map(str::to_owned),
    }
}

fn block(b: &Value) -> Block {
    let text = |key: &str| b[key].as_str().unwrap_or_default().to_owned();
    Block::Known(match b["type"].as_str() {
        Some("text") => Known::Text { text: text("text") },
        Some("thinking") => Known::Reasoning {
            text: Some(text("thinking")),
            tokens: None,
        },
        // A compaction's summary, on an `info` message.
        Some("summary") => Known::Summary {
            text: b["summary"]["summary"]
                .as_str()
                .or(b["summary"].as_str())
                .unwrap_or_default()
                .to_owned(),
        },
        Some("redacted_thinking") => Known::Reasoning {
            text: None,
            tokens: None,
        },
        Some("tool_use") => Known::ToolUse {
            id: text("id"),
            name: text("name"),
            input: b["input"].clone(),
        },
        Some("tool_result") => {
            let run = &b["run"];
            let output = match &run["result"] {
                Value::Null => run["error"].clone(),
                result => result.clone(),
            };
            // A shell command that ran and failed is `done` too.
            let failed = run["status"].as_str().is_some_and(|s| s != "done")
                || output["exitCode"].as_i64().is_some_and(|code| code != 0);
            Known::ToolResult {
                id: text("toolUseID"),
                output,
                error: failed,
            }
        }
        _ => return Block::Other(b.clone()),
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::harnesstools::session::{Content, Message, Role, StopReason};

    /// A real thread's export (Amp 0.0.1793649628), trimmed to what's read.
    fn thread() -> Value {
        serde_json::from_str(include_str!("../../../tests/fixtures/amp/export.json")).unwrap()
    }

    #[rstest]
    fn reads_an_exported_thread() {
        let read = messages(&thread());
        let (title, read) = read.split_last().unwrap();
        assert_eq!(read.len(), 14);
        assert_eq!(read[0].id.as_deref(), Some("M-034qq9yNic5tdSISmJcXx8"));
        assert_eq!(read[0].parent, None);
        assert!(read.windows(2).all(|w| w[1].parent == w[0].id), "each follows the one before");
        assert_eq!(read[0].role(), Role::User);
        // The workspace's file URI, as a path on this platform.
        let tree = url::Url::parse("file:///home/ellie/.cache/amp-e2e/proj").unwrap();
        assert_eq!(read[0].cwd, tree.to_file_path().ok());

        let call = &read[1];
        assert_eq!(call.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(call.stop_reason(), Some(StopReason::ToolUse));
        assert_eq!(call.turn.as_deref(), call.id.as_deref());
        assert_eq!(call.usage.unwrap().cache_write, Some(28719));

        // `ls /nonexistent` exited 2, though Amp says the call is `done`.
        let errors = read
            .iter()
            .flat_map(Message::content)
            .filter(|c| matches!(c, Content::ToolResult(r) if r.error))
            .count();
        assert_eq!(errors, 1);

        // The title is a row of its own: on a message the plugin stored, it would be dropped.
        let text = title.title.as_ref().and_then(|t| t.text.as_deref());
        assert_eq!(text, Some("calc.py addition fix"));
        assert_eq!((title.id.as_deref(), title.parent.as_deref()), (None, None));
        assert!(read.iter().all(|m| m.title.is_none()));
    }

    /// Older threads' messages have no `protocolMessageID`, and the oldest no id at all: each is
    /// still read, under its `messageId` or else its place in the thread.
    #[rstest]
    fn messages_without_ids_are_kept() {
        let thread = serde_json::json!({"messages": [
            {"role": "user", "messageId": 7, "content": [{"type": "text", "text": "a"}]},
            {"role": "assistant", "content": [{"type": "text", "text": "b"}]},
        ]});
        let read = messages(&thread);
        let ids: Vec<_> = read.iter().map(|m| m.id.as_deref()).collect();
        assert_eq!(ids, [Some("7"), Some("#1")]);
        assert_eq!(read[1].parent.as_deref(), Some("7"));
    }

    /// A compaction's summary, as Amp writes one on an `info` message.
    #[rstest]
    fn a_summary_block_is_a_summary() {
        let raw = serde_json::json!({"type": "summary",
            "summary": {"type": "message", "summary": "## Active task"}});
        let line = serde_json::to_value(block(&raw)).unwrap();
        assert_eq!(line, serde_json::json!({"type": "summary", "text": "## Active task"}));
    }

    #[rstest]
    #[case::cancelled(serde_json::json!({"type": "cancelled"}), Some("aborted"))]
    #[case::errored(serde_json::json!({"type": "error"}), Some("error"))]
    #[case::complete(serde_json::json!({"type": "complete", "stopReason": "end_turn"}), Some("end_turn"))]
    #[case::none(Value::Null, None)]
    fn reads_why_a_message_stopped(#[case] state: Value, #[case] want: Option<&str>) {
        assert_eq!(stop(&state).as_deref(), want);
    }
}
