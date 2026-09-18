//! Claude Code: `~/.claude/projects/<escaped cwd>/<session>.jsonl`, plus
//! `<session>/subagents/agent-<id>.jsonl` for sidechains. Every line repeats the session id,
//! cwd and branch, so lines parse independently and a file can be tailed from any offset.

use std::path::{Path, PathBuf};
use std::process::Command;

use eyre::Result;
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::ingest::{Stats, tail};
use crate::store::Store;
use crate::{
    Agent, CUSTOM_TITLE_PREFIX, FromNative, Message, Role, Session, StopReason, ToNative, Tokens,
    ToolCall, cap_output, handoff, lines_with_offsets, ts_rfc3339, turns, working_dir,
};

/// What Claude Code writes into a user turn for a slash command and whatever it printed.
const COMMAND_MARKUP: [&str; 5] = [
    "<command-name>",
    "<command-message>",
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<local-command-caveat>",
];

pub struct ClaudeCode;

impl FromNative for ClaudeCode {
    const AGENT: Agent = Agent::ClaudeCode;

    async fn ingest(store: &Store) -> Result<Stats> {
        let mut stats = Stats::default();
        for path in files(&root()) {
            tail(store, <Self as FromNative>::AGENT, &path, &mut stats, |_, body, base, out| {
                parse(body, base, out);
            })
            .await?;
        }
        Ok(stats)
    }
}

impl ToNative for ClaudeCode {
    const AGENT: Agent = Agent::ClaudeCode;

    async fn write(session: &Session, messages: &[Message]) -> Result<String> {
        write_to(&root(), session, messages)
    }

    fn resume(native_id: &str) -> Command {
        let mut c = Command::new("claude");
        c.args(["--resume", native_id]);
        c
    }
}

/// The API accepts only `[A-Za-z0-9_-]` in a tool id; other agents use more (pi joins two ids
/// with `|`). Applied to the call and its result alike, so they still pair.
fn tool_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Claude Code's project directory name for a cwd: every non-alphanumeric char becomes `-`.
fn project_dir_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Write `session` as `<root>/<project>/<id>.jsonl` unless Claude Code already has that file.
fn write_to(root: &Path, session: &Session, messages: &[Message]) -> Result<String> {
    let id = crate::native_uuid(session).to_string();
    let cwd = working_dir(session);
    let dir = root.join(project_dir_name(&cwd));
    let path = dir.join(format!("{id}.jsonl"));
    if !handoff::writable(&path) {
        return Ok(id); // Claude Code has its own copy, or has continued ours
    }
    let cwd = cwd.to_string_lossy();
    let mut lines = Vec::new();
    let mut parent: Option<String> = None;
    for turn in turns(messages) {
        let ts = turn.timestamp.format(&Rfc3339)?;
        let envelope = |uuid: &str, parent: &Option<String>| {
            json!({
                "parentUuid": parent,
                "isSidechain": false,
                "userType": "external",
                "cwd": cwd,
                "sessionId": id,
                "gitBranch": session.git_branch,
                "uuid": uuid,
                "timestamp": ts,
            })
        };
        let uuid = Uuid::new_v4().to_string();
        let mut line = envelope(&uuid, &parent);
        match turn.role {
            Role::Assistant => {
                let mut content = Vec::new();
                if !turn.text.trim().is_empty() {
                    content.push(json!({"type": "text", "text": turn.text}));
                }
                for call in &turn.calls {
                    let (name, input) = match call.shell_command() {
                        Some(command) => ("Bash".to_owned(), json!({"command": command})),
                        // The API requires an object, even for a call made elsewhere.
                        None if call.input.is_object() => (call.name.clone(), call.input.clone()),
                        None => (call.name.clone(), json!({"input": call.input})),
                    };
                    content.push(
                        json!({"type": "tool_use", "id": tool_id(&call.id), "name": name, "input": input}),
                    );
                }
                line["type"] = "assistant".into();
                line["message"] = json!({
                    "id": format!("msg_{}", Uuid::new_v4().simple()),
                    "type": "message",
                    "role": "assistant",
                    "model": session.model.as_deref().unwrap_or("unknown"),
                    "content": content,
                    "stop_reason": if turn.calls.is_empty() { "end_turn" } else { "tool_use" },
                    "stop_sequence": null,
                    "usage": {"input_tokens": 0, "output_tokens": 0},
                });
            }
            _ => {
                line["type"] = "user".into();
                line["message"] = json!({"role": "user", "content": turn.text});
            }
        }
        lines.push(line.to_string());
        parent = Some(uuid);

        // Results go in the very next user line: the API refuses a `tool_use` left unanswered.
        if !turn.calls.is_empty() {
            let uuid = Uuid::new_v4().to_string();
            let mut line = envelope(&uuid, &parent);
            let results: Vec<Value> = turn
                .calls
                .iter()
                .map(|c| {
                    let mut result =
                        json!({"type": "tool_result", "tool_use_id": tool_id(&c.id), "content": c.output});
                    if c.is_error {
                        result["is_error"] = true.into();
                    }
                    result
                })
                .collect();
            line["type"] = "user".into();
            line["message"] = json!({"role": "user", "content": results});
            lines.push(line.to_string());
            parent = Some(uuid);
        }
    }
    if let Some(title) = &session.title {
        lines.push(json!({"type": "ai-title", "aiTitle": title, "sessionId": id}).to_string());
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&path, handoff::stamp(lines, session)?)?;
    Ok(id)
}

pub fn root() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| atuin_common::utils::home_dir().join(".claude"))
        .join("projects")
}

/// Every transcript under `root`, main sessions and subagents alike.
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

/// Parse `body`, which starts `base` bytes into its file.
pub fn parse(body: &str, base: u64, out: &mut Vec<Message>) {
    // `ai-title` lines carry no timestamp; they take the previous line's.
    let mut last_ts = None;
    for (offset, line) in lines_with_offsets(body, base) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(session_id) = v["sessionId"].as_str() else {
            continue;
        };
        let kind = v["type"].as_str().unwrap_or_default();
        let Some(ts) = v["timestamp"].as_str().and_then(ts_rfc3339).or(last_ts) else {
            continue;
        };
        last_ts = Some(ts);
        let uuid = v["uuid"].as_str().unwrap_or_default();

        let new = |source_id: String, role| {
            let mut m = Message::new(Agent::ClaudeCode, session_id, source_id, ts, role);
            if v["isSidechain"].as_bool() == Some(true) {
                m.thread = v["agentId"].as_str().map(str::to_owned);
            }
            m.parent_source_id = v["parentUuid"].as_str().map(str::to_owned);
            m.cwd = v["cwd"].as_str().map(str::to_owned);
            m.git_branch = v["gitBranch"].as_str().map(str::to_owned);
            m
        };

        match kind {
            "ai-title" => {
                if let Some(title) = v["aiTitle"].as_str() {
                    let mut m = new(format!("title:{offset}"), Role::Title);
                    title.clone_into(&mut m.content);
                    out.push(m);
                }
            }
            // Set by hand with /rename: outranks anything generated.
            "custom-title" => {
                if let Some(title) = v["customTitle"].as_str() {
                    let mut m = new(format!("{CUSTOM_TITLE_PREFIX}{offset}"), Role::Title);
                    title.clone_into(&mut m.content);
                    out.push(m);
                }
            }
            "system" if v.get("compactMetadata").is_some() => {
                let mut m = new(uuid.to_owned(), Role::System);
                v["content"].as_str().unwrap_or_default().clone_into(&mut m.content);
                out.push(m);
            }
            "user" if v["isMeta"].as_bool() != Some(true) => {
                let mut text = String::new();
                let mut results = Vec::new();
                match &v["message"]["content"] {
                    Value::String(s) => text.push_str(s),
                    Value::Array(blocks) => {
                        for b in blocks {
                            match b["type"].as_str() {
                                Some("text") => {
                                    text.push_str(b["text"].as_str().unwrap_or_default());
                                }
                                Some("tool_result") => results.push(b),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                // An interrupt is recorded as a user line; it is the previous turn that it ends.
                if text.trim_start().starts_with("[Request interrupted by user")
                    && let Some(last) = out.iter_mut().rev().find(|m| {
                        m.role == Role::Assistant
                            && m.session_id == session_id
                            && m.thread.is_none()
                    })
                {
                    last.stop_reason = Some(StopReason::Aborted);
                }
                // Slash commands and their output are the harness talking, not the user.
                if COMMAND_MARKUP.iter().any(|tag| text.trim_start().starts_with(tag)) {
                    text.clear();
                }
                if !text.trim().is_empty() {
                    let role = if v["isCompactSummary"].as_bool() == Some(true) {
                        Role::System
                    } else {
                        Role::User
                    };
                    let mut m = new(uuid.to_owned(), role);
                    m.content = text;
                    out.push(m);
                }
                for (i, r) in results.iter().enumerate() {
                    let mut m = new(format!("{uuid}#{i}"), Role::Tool);
                    m.tool_use_id = r["tool_use_id"].as_str().map(str::to_owned);
                    m.is_error = r["is_error"].as_bool() == Some(true);
                    m.content = cap_output(block_text(&r["content"]));
                    out.push(m);
                }
            }
            "assistant" => {
                let msg = &v["message"];
                let mut m = new(uuid.to_owned(), Role::Assistant);
                m.model = msg["model"].as_str().map(str::to_owned);
                m.stop_reason = match msg["stop_reason"].as_str() {
                    Some("end_turn" | "stop_sequence") => Some(StopReason::EndTurn),
                    Some("tool_use") => Some(StopReason::ToolUse),
                    Some("max_tokens") => Some(StopReason::MaxTokens),
                    _ => None,
                };
                if let Some(u) = msg["usage"].as_object() {
                    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                    m.tokens = Some(Tokens {
                        input: n("input_tokens"),
                        output: n("output_tokens"),
                        cache_read: n("cache_read_input_tokens"),
                        cache_write: n("cache_creation_input_tokens"),
                    });
                }
                for b in msg["content"].as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => m.content.push_str(b["text"].as_str().unwrap_or_default()),
                        Some("tool_use") => m.tool_calls.push(ToolCall {
                            id: b["id"].as_str().unwrap_or_default().to_owned(),
                            name: b["name"].as_str().unwrap_or_default().to_owned(),
                            input: b["input"].to_string(),
                        }),
                        _ => {}
                    }
                }
                if !m.content.trim().is_empty() || !m.tool_calls.is_empty() {
                    out.push(m);
                }
            }
            _ => {}
        }
    }
}

/// A `tool_result` body is a string or a list of text blocks.
fn block_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => {
            blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n")
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        r#"{"type":"user","uuid":"u1","parentUuid":null,"sessionId":"s1","cwd":"/p","gitBranch":"main","isSidechain":false,"timestamp":"2026-09-01T10:00:00.000Z","message":{"role":"user","content":"fix the test"}}"#,
        "\n",
        r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"s1","cwd":"/p","isSidechain":false,"timestamp":"2026-09-01T10:00:01.000Z","message":{"model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":5},"content":[{"type":"text","text":"Running it."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}}"#,
        "\n",
        r#"{"type":"user","uuid":"u2","parentUuid":"a1","sessionId":"s1","cwd":"/p","isSidechain":false,"timestamp":"2026-09-01T10:00:02.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"ok"}]}]},"toolUseResult":{}}"#,
        "\n",
        r#"{"type":"ai-title","sessionId":"s1","aiTitle":"Fix flaky test"}"#,
        "\n",
        r#"{"type":"user","uuid":"u3","sessionId":"s1","isSidechain":true,"agentId":"ag1","timestamp":"2026-09-01T10:00:03.000Z","message":{"content":"sub"}}"#,
        "\n",
        r#"{"type":"file-history-snapshot","sessionId":"s1","snapshot":{}}"#,
        "\n",
        r#"{"type":"user","uuid":"m1","sessionId":"s1","isMeta":true,"timestamp":"2026-09-01T10:00:04.000Z","message":{"content":"caveat"}}"#,
        "\n",
        r#"{"type":"user","uuid":"c1","sessionId":"s1","timestamp":"2026-09-01T10:00:05.000Z","message":{"content":"<command-name>/model</command-name>"}}"#,
        "\n",
        r#"{"type":"custom-title","customTitle":"My name for it","sessionId":"s1"}"#,
        "\n",
        r#"{"type":"ai-title","sessionId":"s1","aiTitle":"A later generated title"}"#,
        "\n",
        r#"{"type":"user","uuid":"e1","sessionId":"s1","timestamp":"2026-09-01T10:00:06.000Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"boom"}]}}"#,
        "\n",
        r#"{"type":"user","uuid":"i1","sessionId":"s1","timestamp":"2026-09-01T10:00:07.000Z","message":{"content":"[Request interrupted by user]"}}"#,
        "\n",
    );

    #[test]
    fn parses_prompts_tool_calls_results_titles_and_sidechains() {
        let mut out = Vec::new();
        parse(FIXTURE, 0, &mut out);
        let roles: Vec<Role> = out.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            [
                Role::User,
                Role::Assistant,
                Role::Tool,
                Role::Title,
                Role::User,
                Role::Title, // set by hand; the /model command before it is dropped
                Role::Title,
                Role::Tool,
                Role::User,
            ]
        );
        assert!(out[5].source_id.starts_with(CUSTOM_TITLE_PREFIX));
        assert_eq!((out[7].is_error, out[2].is_error), (true, false));
        // The interrupt ends the assistant turn before it, and the hand-set title wins.
        assert_eq!(out[1].stop_reason, Some(StopReason::Aborted));
        let session = &crate::sessions(&out)[0];
        assert_eq!(session.title.as_deref(), Some("My name for it"));
        assert_eq!(session.last_stop, Some(StopReason::Aborted));

        assert_eq!(out[0].git_branch.as_deref(), Some("main"));
        assert_eq!(out[1].tool_calls[0].name, "Bash");
        assert_eq!(out[1].tool_calls[0].input, r#"{"command":"cargo test"}"#);
        assert_eq!(out[1].tokens.unwrap().cache_read, 5);
        assert_eq!(out[2].tool_use_id.as_deref(), Some("t1"));
        assert_eq!((out[2].content.as_str(), out[2].source_id.as_str()), ("ok", "u2#0"));
        assert_eq!(out[3].content, "Fix flaky test");
        assert_eq!(out[3].timestamp, out[2].timestamp); // no timestamp of its own
        assert_eq!(out[4].thread.as_deref(), Some("ag1"));
    }

    #[test]
    fn to_native_round_trips_through_from_native() {
        use time::macros::datetime;

        let t = datetime!(2026-09-01 10:00 UTC);
        let session = Session {
            agent: Agent::Codex,
            session_id: "th1".into(), // not a UUID: Claude gets a fresh one
            parent_session_id: None,
            title: Some("Listing".into()),
            cwd: Some("/".into()),
            git_branch: Some("main".into()),
            model: Some("gpt-5.5".into()),
            started_at: t,
            ended_at: t,
            messages: 3,
            tool_calls: 1,
            threads: 0,
            tokens: Tokens::default(),
            last_stop: None,
        };
        let mut user = Message::new(Agent::Codex, "th1", "1", t, Role::User);
        user.content = "ls please".into();
        let mut call = Message::new(Agent::Codex, "th1", "2", t, Role::Assistant);
        call.tool_calls.push(ToolCall {
            id: "c".into(),
            name: "exec_command".into(),
            input: r#"{"cmd":"ls"}"#.into(),
        });
        call.tool_calls.push(ToolCall {
            id: "p".into(),
            name: "apply_patch".into(),
            input: "*** Begin Patch".into(),
        });
        let mut result = Message::new(Agent::Codex, "th1", "3", t, Role::Tool);
        result.tool_use_id = Some("c".into());
        result.content = "a\nb".into();
        let messages = [user, call, result];

        let root = tempfile::tempdir().unwrap();
        let id = write_to(root.path(), &session, &messages).unwrap();
        let path = root.path().join("-").join(format!("{id}.jsonl"));
        assert!(path.is_file());
        // Writing again refreshes it in place: same id, same file.
        assert_eq!(write_to(root.path(), &session, &messages).unwrap(), id);

        let mut back = Vec::new();
        parse(&std::fs::read_to_string(&path).unwrap(), 0, &mut back);
        // The Codex shell call comes back as Claude's own Bash, paired with its output; the
        // bare-string patch is wrapped in an object; the unanswered call gets an empty result.
        let turns_back = turns(&back);
        assert_eq!(
            turns_back.iter().map(|t| t.role).collect::<Vec<_>>(),
            [Role::User, Role::Assistant]
        );
        assert_eq!(turns_back[0].text, "ls please");
        let calls = &turns_back[1].calls;
        assert_eq!(
            (calls[0].name.as_str(), &calls[0].input, calls[0].output.as_str()),
            ("Bash", &json!({"command": "ls"}), "a\nb")
        );
        assert_eq!(
            (calls[1].name.as_str(), &calls[1].input, calls[1].output.as_str()),
            ("apply_patch", &json!({"input": "*** Begin Patch"}), "")
        );
        // Every tool_use is answered in the line right after it.
        let raw: Vec<Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(raw[1]["message"]["stop_reason"], "tool_use");
        assert_eq!(raw[2]["message"]["content"][1]["tool_use_id"], "p");
        assert_eq!(raw[2]["parentUuid"], raw[1]["uuid"]);
        assert!(back.iter().all(|m| m.session_id == id));
        assert!(
            back.iter().filter(|m| m.role != Role::Title).all(|m| m.cwd.as_deref() == Some("/"))
        );
        assert_eq!(back.iter().find(|m| m.role == Role::Title).unwrap().content, "Listing");
        assert_eq!(project_dir_name(Path::new("/Users/ellie/.herdr/wt")), "-Users-ellie--herdr-wt");
        assert_eq!(tool_id("call_1|fc_2"), "call_1_fc_2");

        // Untouched, a handoff is refreshed from its source. Once the agent has added to it,
        // it holds turns that exist nowhere else and is left exactly as it is.
        let stamped = std::fs::read_to_string(&path).unwrap();
        assert_eq!(crate::handoff::Origin::of_file(&path).unwrap().bytes, stamped.len() as u64);
        let continued = format!("{stamped}{{\"the agent\":\"carried on\"}}\n");
        std::fs::write(&path, &continued).unwrap();
        assert_eq!(write_to(root.path(), &session, &[]).unwrap(), id);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), continued);
    }

    #[test]
    fn offsets_are_absolute() {
        let mut out = Vec::new();
        let second_line = FIXTURE.find('\n').unwrap() + 1;
        parse(&FIXTURE[second_line..], second_line as u64, &mut out);
        let title = out.iter().find(|m| m.role == Role::Title).unwrap();
        let title_at = FIXTURE.find(r#"{"type":"ai-title""#).unwrap();
        assert_eq!(title.source_id, format!("title:{title_at}"));
    }
}
