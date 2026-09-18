//! pi: `~/.pi/agent/sessions/--<cwd dashed>--/<timestamp>_<session>.jsonl`, one session per file.
//! Line 1 is a `session` header (id, cwd); every later line has an `id`, a `parentId` and a
//! timestamp, and turns live in `message` lines whose `message.role` says what they are.

use std::path::{Path, PathBuf};
use std::process::Command;

use eyre::Result;
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use walkdir::WalkDir;

use crate::ingest::{Stats, tail};
use crate::store::Store;
use crate::{
    Agent, FromNative, Message, Role, Session, StopReason, ToNative, Tokens, ToolCall, cap_output,
    handoff, lines_with_offsets, ts_rfc3339, turns, working_dir,
};

pub struct Pi;

impl FromNative for Pi {
    const AGENT: Agent = Agent::Pi;

    async fn ingest(store: &Store) -> Result<Stats> {
        let mut stats = Stats::default();
        for path in files(&root()) {
            tail(store, <Self as FromNative>::AGENT, &path, &mut stats, parse).await?;
        }
        Ok(stats)
    }
}

impl ToNative for Pi {
    const AGENT: Agent = Agent::Pi;

    async fn write(session: &Session, messages: &[Message]) -> Result<String> {
        write_to(&root(), session, messages)
    }

    fn resume(native_id: &str) -> Command {
        let mut c = Command::new("pi");
        c.args(["--session", native_id]);
        c
    }
}

/// `PI_CODING_AGENT_SESSION_DIR`, else `<PI_CODING_AGENT_DIR>/sessions`, else the default.
#[must_use]
pub fn root() -> PathBuf {
    let env = |name: &str| atuin_common::utils::env_nonempty(name).map(PathBuf::from);
    env("PI_CODING_AGENT_SESSION_DIR")
        .or_else(|| env("PI_CODING_AGENT_DIR").map(|d| d.join("sessions")))
        .unwrap_or_else(|| atuin_common::utils::home_dir().join(".pi/agent/sessions"))
}

#[must_use]
pub fn files(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .map(walkdir::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    files
}

/// Parse `body`, which starts `base` bytes into its file. `header` is the file's first line,
/// which names the session and its directory.
pub fn parse(header: &str, body: &str, _base: u64, out: &mut Vec<Message>) {
    let Ok(head) = serde_json::from_str::<Value>(header) else {
        return;
    };
    let (Some("session"), Some(session_id)) = (head["type"].as_str(), head["id"].as_str()) else {
        return;
    };
    let cwd = head["cwd"].as_str().map(str::to_owned);
    let parent_session = head["parentSession"].as_str().map(str::to_owned);

    for (_, line) in lines_with_offsets(body, 0) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let (Some(id), Some(ts)) = (v["id"].as_str(), v["timestamp"].as_str().and_then(ts_rfc3339))
        else {
            continue;
        };
        let new = |source_id: String, role| {
            let mut m = Message::new(Agent::Pi, session_id, source_id, ts, role);
            m.parent_source_id = v["parentId"].as_str().map(str::to_owned);
            m.parent_session_id.clone_from(&parent_session);
            m.cwd.clone_from(&cwd);
            m
        };
        let msg = &v["message"];
        match (v["type"].as_str(), msg["role"].as_str()) {
            // A name the user gave the session.
            (Some("session_info"), _) => {
                if let Some(name) = v["name"].as_str() {
                    let mut m = new(format!("{}{id}", crate::CUSTOM_TITLE_PREFIX), Role::Title);
                    name.clone_into(&mut m.content);
                    out.push(m);
                }
            }
            (Some("message"), Some("user")) => {
                let mut m = new(id.to_owned(), Role::User);
                m.content = text_of(&msg["content"]);
                out.push(m);
            }
            (Some("custom_message"), _) => {
                let mut m = new(id.to_owned(), Role::System);
                m.content = text_of(&v["content"]);
                out.push(m);
            }
            (Some("message"), Some("assistant")) => {
                let mut m = new(id.to_owned(), Role::Assistant);
                m.content = text_of(&msg["content"]);
                m.model = msg["model"].as_str().map(str::to_owned);
                m.stop_reason = match msg["stopReason"].as_str() {
                    Some("stop") => Some(StopReason::EndTurn),
                    Some("toolUse") => Some(StopReason::ToolUse),
                    Some("length") => Some(StopReason::MaxTokens),
                    Some("aborted") => Some(StopReason::Aborted),
                    Some("error") => Some(StopReason::Error),
                    _ => None,
                };
                let u = &msg["usage"];
                if u.is_object() {
                    let n = |k: &str| u[k].as_u64().unwrap_or(0);
                    m.tokens = Some(Tokens {
                        input: n("input"),
                        output: n("output"),
                        cache_read: n("cacheRead"),
                        cache_write: n("cacheWrite"),
                    });
                }
                for b in msg["content"].as_array().into_iter().flatten() {
                    if b["type"] == "toolCall" {
                        m.tool_calls.push(ToolCall {
                            id: b["id"].as_str().unwrap_or_default().to_owned(),
                            name: b["name"].as_str().unwrap_or_default().to_owned(),
                            input: b["arguments"].to_string(),
                        });
                    }
                }
                out.push(m);
            }
            (Some("message"), Some("toolResult")) => {
                let mut m = new(id.to_owned(), Role::Tool);
                m.tool_use_id = msg["toolCallId"].as_str().map(str::to_owned);
                m.is_error = msg["isError"].as_bool() == Some(true);
                m.content = cap_output(text_of(&msg["content"]));
                out.push(m);
            }
            // A `!command` the user ran in pi's own shell, with what it printed.
            (Some("message"), Some("bashExecution")) => {
                let mut m = new(id.to_owned(), Role::User);
                m.content = format!("!{}", msg["command"].as_str().unwrap_or_default());
                out.push(m);
                let mut r = new(format!("{id}#out"), Role::Tool);
                r.is_error = msg["exitCode"].as_i64().is_some_and(|c| c != 0);
                r.content = cap_output(msg["output"].as_str().unwrap_or_default().to_owned());
                out.push(r);
            }
            _ => {}
        }
    }
    out.retain(|m| !m.content.trim().is_empty() || !m.tool_calls.is_empty());
}

/// Content is a string or a list of blocks; only `text` blocks are prose.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// pi's directory name for a cwd: drop the leading separator, turn `/`, `\` and `:` into `-`,
/// wrap in `--`.
fn project_dir_name(cwd: &Path) -> String {
    let cwd = cwd.to_string_lossy();
    let inner: String = cwd
        .trim_start_matches(['/', '\\'])
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("--{inner}--")
}

/// pi records which API produced a turn. Guess from the model name; pi converts history to
/// whatever model the user continues with.
fn provider_api(model: &str) -> (&'static str, &'static str) {
    if model.contains("claude") {
        ("anthropic", "anthropic-messages")
    } else {
        ("openai", "openai-responses")
    }
}

/// Write `session` under `<root>/<project>/` unless pi already has a file for that id.
fn write_to(root: &Path, session: &Session, messages: &[Message]) -> Result<String> {
    let id = crate::native_uuid(session).to_string();
    let suffix = format!("_{id}.jsonl");
    if files(root).iter().any(|p| p.to_string_lossy().ends_with(&suffix) && !handoff::writable(p)) {
        return Ok(id); // pi has its own copy, or has continued ours
    }
    let cwd = working_dir(session);
    let dir = root.join(project_dir_name(&cwd));
    let started = session.started_at.format(&Rfc3339)?;
    let path = dir.join(format!("{}{suffix}", started.replace([':', '.'], "-")));
    let model = session.model.clone().unwrap_or_default();
    let (provider, api) = provider_api(&model);

    let mut lines = vec![
        json!({"type": "session", "version": 3, "id": id, "timestamp": started, "cwd": cwd})
            .to_string(),
    ];
    let mut parent: Option<String> = None;
    let mut push = |ts: &str, mut line: Value| {
        let entry = format!("{:08x}", lines.len());
        line["id"] = entry.clone().into();
        line["parentId"] = parent.clone().into();
        line["timestamp"] = ts.into();
        lines.push(line.to_string());
        parent = Some(entry);
    };
    if let Some(title) = &session.title {
        push(&started, json!({"type": "session_info", "name": title}));
    }
    for turn in turns(messages) {
        let ts = turn.timestamp.format(&Rfc3339)?;
        let ms = i64::try_from(turn.timestamp.unix_timestamp_nanos() / 1_000_000).unwrap_or(0);
        let mut content = Vec::new();
        if !turn.text.trim().is_empty() {
            content.push(json!({"type": "text", "text": turn.text}));
        }
        if turn.role != Role::Assistant {
            push(
                &ts,
                json!({"type": "message", "message": {"role": "user", "content": content, "timestamp": ms}}),
            );
            continue;
        }
        let calls: Vec<(String, Value)> = turn
            .calls
            .iter()
            .map(|c| match c.shell_command() {
                Some(command) => ("bash".to_owned(), json!({"command": command})),
                None => (c.name.clone(), c.input.clone()),
            })
            .collect();
        for (c, (name, arguments)) in turn.calls.iter().zip(&calls) {
            content.push(
                json!({"type": "toolCall", "id": c.id, "name": name, "arguments": arguments}),
            );
        }
        push(
            &ts,
            json!({"type": "message", "message": {
                "role": "assistant", "content": content, "api": api, "provider": provider, "model": model,
                "usage": {
                    "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                    "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0},
                },
                "stopReason": if turn.calls.is_empty() { "stop" } else { "toolUse" },
                "timestamp": ms,
            }}),
        );
        for (c, (name, _)) in turn.calls.iter().zip(&calls) {
            push(
                &ts,
                json!({"type": "message", "message": {
                    "role": "toolResult", "toolCallId": c.id, "toolName": name,
                    "content": [{"type": "text", "text": c.output}], "isError": c.is_error, "timestamp": ms,
                }}),
            );
        }
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&path, handoff::stamp(lines, session)?)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    const HEADER: &str = r#"{"type":"session","version":3,"id":"p1","timestamp":"2026-04-13T19:17:07.541Z","cwd":"/w"}"#;
    const BODY: &str = concat!(
        r#"{"type":"model_change","id":"m0","parentId":null,"timestamp":"2026-04-13T19:17:07.581Z","modelId":"gpt-5.4"}"#,
        "\n",
        r#"{"type":"session_info","id":"n0","parentId":"m0","timestamp":"2026-04-13T19:17:08.000Z","name":"Installer work"}"#,
        "\n",
        r#"{"type":"message","id":"u1","parentId":"n0","timestamp":"2026-04-13T19:17:09.000Z","message":{"role":"user","content":[{"type":"text","text":"update the installer"}],"timestamp":1}}"#,
        "\n",
        r#"{"type":"message","id":"a1","parentId":"u1","timestamp":"2026-04-13T19:17:10.000Z","message":{"role":"assistant","model":"gpt-5.4","stopReason":"toolUse","usage":{"input":10,"output":2,"cacheRead":3,"cacheWrite":0},"content":[{"type":"thinking"},{"type":"text","text":"Looking."},{"type":"toolCall","id":"call_1|fc_2","name":"bash","arguments":{"command":"rg hook"}}]}}"#,
        "\n",
        r#"{"type":"message","id":"r1","parentId":"a1","timestamp":"2026-04-13T19:17:11.000Z","message":{"role":"toolResult","toolCallId":"call_1|fc_2","toolName":"bash","isError":true,"content":[{"type":"text","text":"rg: not found"}]}}"#,
        "\n",
        r#"{"type":"message","id":"b1","parentId":"r1","timestamp":"2026-04-13T19:17:12.000Z","message":{"role":"bashExecution","command":"ls","output":"a","exitCode":0}}"#,
        "\n",
        r#"{"type":"message","id":"a2","parentId":"b1","timestamp":"2026-04-13T19:17:13.000Z","message":{"role":"assistant","stopReason":"aborted","content":[{"type":"text","text":"Sto"}]}}"#,
        "\n",
    );

    #[test]
    fn parses_turns_tools_titles_and_stop_reasons() {
        let mut out = Vec::new();
        parse(HEADER, BODY, 0, &mut out);
        let roles: Vec<Role> = out.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            [
                Role::Title,
                Role::User,
                Role::Assistant,
                Role::Tool,
                Role::User,
                Role::Tool,
                Role::Assistant
            ]
        );
        assert!(out.iter().all(|m| m.session_id == "p1" && m.cwd.as_deref() == Some("/w")));
        assert_eq!(
            (out[2].content.as_str(), out[2].model.as_deref()),
            ("Looking.", Some("gpt-5.4"))
        );
        assert_eq!(out[2].tool_calls[0].input, r#"{"command":"rg hook"}"#);
        assert_eq!(out[2].tokens.unwrap().cache_read, 3);
        assert_eq!((out[3].is_error, out[3].tool_use_id.as_deref()), (true, Some("call_1|fc_2")));
        assert_eq!((out[4].content.as_str(), out[5].content.as_str()), ("!ls", "a"));

        let session = &crate::sessions(&out)[0];
        assert_eq!(session.title.as_deref(), Some("Installer work")); // named by hand
        assert_eq!(session.last_stop, Some(StopReason::Aborted));
    }

    #[test]
    fn to_native_round_trips_through_from_native() {
        let t = datetime!(2026-09-01 10:00:05 UTC);
        let session = Session {
            agent: Agent::ClaudeCode,
            session_id: "0192a7d2-8b6a-7c1e-9d3f-1a2b3c4d5e6f".into(),
            parent_session_id: None,
            title: Some("Listing".into()),
            cwd: Some("/".into()),
            git_branch: None,
            model: Some("claude-opus-5".into()),
            started_at: t,
            ended_at: t,
            messages: 3,
            tool_calls: 1,
            threads: 0,
            tokens: Tokens::default(),
            last_stop: None,
        };
        let mut user = Message::new(Agent::ClaudeCode, &session.session_id, "u1", t, Role::User);
        user.content = "ls please".into();
        let mut reply =
            Message::new(Agent::ClaudeCode, &session.session_id, "a1", t, Role::Assistant);
        reply.content = "Sure.".into();
        reply.tool_calls.push(ToolCall {
            id: "t1".into(),
            name: "Bash".into(),
            input: r#"{"command":"ls"}"#.into(),
        });
        let mut result =
            Message::new(Agent::ClaudeCode, &session.session_id, "u2#0", t, Role::Tool);
        result.tool_use_id = Some("t1".into());
        result.is_error = true;
        result.content = "denied".into();

        let root = tempfile::tempdir().unwrap();
        let id = write_to(root.path(), &session, &[user, reply, result]).unwrap();
        let path = root.path().join("----").join(format!("2026-09-01T10-00-05Z_{id}.jsonl"));
        assert!(path.is_file(), "{:?}", files(root.path()));

        let text = std::fs::read_to_string(&path).unwrap();
        let (header, body) = text.split_once('\n').unwrap();
        let mut back = Vec::new();
        parse(header, body, 0, &mut back);
        let turns_back = turns(&back);
        assert_eq!(
            turns_back.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["ls please", "Sure."]
        );
        let call = &turns_back[1].calls[0];
        // Claude's Bash becomes pi's own bash, still failed, still paired.
        assert_eq!(
            (call.name.as_str(), &call.input, call.output.as_str(), call.is_error),
            ("bash", &json!({"command": "ls"}), "denied", true)
        );
        assert_eq!(back.iter().find(|m| m.role == Role::Title).unwrap().content, "Listing");
        assert_eq!(
            project_dir_name(Path::new("/Users/alice/src/myproj")),
            "--Users-alice-src-myproj--"
        );

        // Untouched, a handoff is refreshed from its source. Once the agent has added to it,
        // it holds turns that exist nowhere else and is left exactly as it is.
        let stamped = std::fs::read_to_string(&path).unwrap();
        assert_eq!(crate::handoff::Origin::of_file(&path).unwrap().bytes, stamped.len() as u64);
        let continued = format!("{stamped}{{\"the agent\":\"carried on\"}}\n");
        std::fs::write(&path, &continued).unwrap();
        assert_eq!(write_to(root.path(), &session, &[]).unwrap(), id);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), continued);
    }
}
