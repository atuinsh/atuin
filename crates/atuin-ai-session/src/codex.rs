//! Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<thread>.jsonl`, one thread per file.
//! Line 1 is `session_meta` (thread id, cwd, branch); later lines carry no ids, so the byte
//! offset of a line is its `source_id`. Titles live in `~/.codex/session_index.jsonl`.

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

pub struct Codex;

impl FromNative for Codex {
    const AGENT: Agent = Agent::Codex;

    async fn ingest(store: &Store) -> Result<Stats> {
        let mut stats = Stats::default();
        for path in files(&root()) {
            tail(
                store,
                <Self as FromNative>::AGENT,
                &path,
                &mut stats,
                |header, body, base, out| {
                    parse(&path, header, body, base, out);
                },
            )
            .await?;
        }
        Ok(stats)
    }
}

impl ToNative for Codex {
    const AGENT: Agent = Agent::Codex;

    async fn write(session: &Session, messages: &[Message]) -> Result<String> {
        let root = root();
        let (id, path) = write_to(&root, session, messages)?;
        if let Some(path) = path {
            index_thread(&root, &id, &path, session).await;
        }
        Ok(id)
    }

    fn resume(native_id: &str) -> Command {
        let mut c = Command::new("codex");
        c.args(["resume", native_id]);
        c
    }
}

/// Write `session` as a rollout under `<root>/sessions` unless Codex already has that thread.
/// Returns the thread id and the new file, if one was written.
fn write_to(
    root: &Path,
    session: &Session,
    messages: &[Message],
) -> Result<(String, Option<PathBuf>)> {
    let id = crate::native_uuid(session).to_string();
    let suffix = format!("-{id}.jsonl");
    let existing = files(root).into_iter().find(|p| p.to_string_lossy().ends_with(&suffix));
    if existing.as_deref().is_some_and(|p| !handoff::writable(p)) {
        return Ok((id, None)); // Codex has its own copy, or has continued ours
    }
    let cwd = working_dir(session).to_string_lossy().into_owned();
    let started = session.started_at;
    let dir = root.join("sessions").join(format!(
        "{:04}/{:02}/{:02}",
        started.year(),
        u8::from(started.month()),
        started.day()
    ));
    let stamp = started.format(&time::macros::format_description!(
        "[year]-[month]-[day]T[hour]-[minute]-[second]"
    ))?;
    let path = dir.join(format!("rollout-{stamp}-{id}.jsonl"));

    let mut lines = vec![
        json!({
            "timestamp": started.format(&Rfc3339)?,
            "type": "session_meta",
            "payload": {
                "id": id,
                "timestamp": started.format(&Rfc3339)?,
                "cwd": cwd,
                "originator": "atuin",
                "cli_version": "0.0.0",
                "source": "cli",
                "model_provider": "openai",
                "git": {"branch": session.git_branch},
            },
        })
        .to_string(),
    ];
    // `response_item` is what the model replays; the `event_msg` twin is what Codex's UI draws.
    let mut push = |ts: &str, kind: &str, payload: Value| {
        lines.push(json!({"timestamp": ts, "type": kind, "payload": payload}).to_string());
    };
    for turn in turns(messages) {
        let ts = turn.timestamp.format(&Rfc3339)?;
        if !turn.text.trim().is_empty() {
            let (role, kind, event) = match turn.role {
                Role::Assistant => (
                    "assistant",
                    "output_text",
                    json!({"type": "agent_message", "message": turn.text}),
                ),
                _ => (
                    "user",
                    "input_text",
                    json!({"type": "user_message", "message": turn.text, "kind": "plain"}),
                ),
            };
            push(
                &ts,
                "response_item",
                json!({"type": "message", "role": role, "content": [{"type": kind, "text": turn.text}]}),
            );
            push(&ts, "event_msg", event);
        }
        // Every call is followed by its output: the API refuses a call left unanswered.
        for call in &turn.calls {
            let (name, arguments) = match call.shell_command() {
                Some(command) => ("exec_command".to_owned(), json!({"cmd": command}).to_string()),
                None => (
                    tool_name(&call.name),
                    call.input.as_str().map_or_else(|| call.input.to_string(), str::to_owned),
                ),
            };
            push(
                &ts,
                "response_item",
                json!({"type": "function_call", "name": name, "arguments": arguments, "call_id": call.id}),
            );
            push(
                &ts,
                "response_item",
                json!({"type": "function_call_output", "call_id": call.id, "output": call.output}),
            );
        }
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&path, handoff::stamp(lines, session)?)?;
    // A refresh of a file we wrote earlier already has its title line.
    if let Some(title) = session.title.as_ref().filter(|_| existing.is_none()) {
        let entry = json!({"id": id, "thread_name": title, "updated_at": session.ended_at.format(&Rfc3339)?});
        let mut index =
            std::fs::OpenOptions::new().append(true).create(true).open(root.join(INDEX_FILE))?;
        std::io::Write::write_all(&mut index, (entry.to_string() + "\n").as_bytes())?;
    }
    Ok((id, Some(path)))
}

/// The shell command inside a code-mode script, when the script is one `tools.exec_command`
/// call: `text(await tools.exec_command({cmd:"atuin sync", …}))`. Keys may be bare or quoted.
fn code_mode_command(script: &str) -> Option<String> {
    if script.matches("tools.").count() != 1 {
        return None;
    }
    let args = &script[script.find("tools.exec_command(")?..];
    let after_key = &args[args.find("cmd")? + "cmd".len()..];
    let value = after_key.trim_start_matches('"').trim_start().strip_prefix(':')?.trim_start();
    // The value is a double-quoted literal, which JSON reads the same way JavaScript does.
    serde_json::Deserializer::from_str(value).into_iter::<String>().next()?.ok()
}

/// A tool output as text, with the exit code when the output carries one.
///
/// Older rollouts store a string. Newer ones store text blocks: a `Script completed…` banner,
/// then either plain text or a JSON envelope holding `output` and `exit_code`.
fn output_text(output: &Value) -> (String, Option<i64>) {
    let Some(blocks) = output.as_array() else {
        return (output.as_str().map_or_else(|| output.to_string(), str::to_owned), None);
    };
    let mut exit_code = None;
    let mut parts = Vec::new();
    for text in blocks.iter().filter_map(|b| b["text"].as_str()) {
        if blocks.len() > 1 && text.starts_with("Script completed") {
            continue;
        }
        match serde_json::from_str::<Value>(text) {
            Ok(envelope) if envelope["output"].is_string() => {
                exit_code = envelope["exit_code"].as_i64().or(exit_code);
                parts.push(envelope["output"].as_str().unwrap_or_default().to_owned());
            }
            _ => parts.push(text.to_owned()),
        }
    }
    (parts.join("\n"), exit_code)
}

/// Whether a command output reports a non-zero exit, in either shape Codex has used: the
/// `Process exited with code N` header, or an `"exit_code":N` field in a JSON envelope.
fn exit_failed(output: &str) -> bool {
    ["Process exited with code ", "\"exit_code\":"].iter().any(|marker| {
        output.find(marker).is_some_and(|at| {
            let code: String =
                output[at + marker.len()..].chars().take_while(char::is_ascii_digit).collect();
            code.parse::<i64>().is_ok_and(|c| c != 0)
        })
    })
}

/// The API validates replayed function names against `[A-Za-z0-9_-]+`; other agents allow more.
fn tool_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "tool".to_owned()
    } else {
        safe
    }
}

/// Register a new rollout in Codex's thread index so `codex resume` finds it. Best effort: the
/// index is Codex's own schema and a miss only costs the picker entry.
async fn index_thread(root: &Path, id: &str, path: &Path, session: &Session) {
    let db = root.join("state_5.sqlite");
    if !db.is_file() {
        return;
    }
    let result: Result<()> = async {
        use sqlx::{Connection, Row};
        let mut conn = sqlx::SqliteConnection::connect_with(&sqlx::sqlite::SqliteConnectOptions::new().filename(&db))
            .await?;
        // Copy the policy columns from the newest thread; they are required and opaque to us.
        let template = atuin_common::db::query(
            "select sandbox_policy, approval_mode from threads order by updated_at desc limit 1",
        )
        .fetch_optional(&mut conn)
        .await?;
        let (sandbox, approval): (String, String) = template
            .map(|r| (r.get(0), r.get(1)))
            .unwrap_or_else(|| ("{}".to_owned(), "on-request".to_owned()));
        atuin_common::db::query(
            "insert or ignore into threads (id, rollout_path, created_at, updated_at, source, \
             model_provider, cwd, title, sandbox_policy, approval_mode, git_branch, first_user_message) \
             values (?1, ?2, ?3, ?4, 'cli', 'openai', ?5, ?6, ?7, ?8, ?9, ?6)",
        )
        .bind(id)
        .bind(path.to_string_lossy().as_ref())
        .bind(session.started_at.unix_timestamp())
        .bind(session.ended_at.unix_timestamp())
        .bind(working_dir(session).to_string_lossy().as_ref())
        .bind(session.title.as_deref().unwrap_or_default())
        .bind(sandbox)
        .bind(approval)
        .bind(&session.git_branch)
        .execute(&mut conn)
        .await?;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        tracing::warn!("could not register thread {id} in codex index: {e}");
    }
}

pub const INDEX_FILE: &str = "session_index.jsonl";

/// Text Codex injects into user turns that is not the user's prompt.
const INJECTED: [&str; 6] = [
    "<turn_aborted>",
    "<environment_context>",
    "# AGENTS.md",
    "<permissions",
    "<collaboration_mode>",
    "<skills_instructions>",
];

pub fn root() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| atuin_common::utils::home_dir().join(".codex"))
}

/// Every rollout under `root/sessions`, plus the title index if present.
pub fn files(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = WalkDir::new(root.join("sessions"))
        .into_iter()
        .filter_map(Result::ok)
        .map(walkdir::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    let index = root.join(INDEX_FILE);
    if index.is_file() {
        files.push(index);
    }
    files
}

/// Parse `body`, which starts `base` bytes into `path`. `header` is the file's first line,
/// needed for the thread id and cwd when `body` starts past it.
pub fn parse(path: &Path, header: &str, body: &str, base: u64, out: &mut Vec<Message>) {
    if path.file_name().is_some_and(|f| f == INDEX_FILE) {
        return parse_index(body, base, out);
    }
    let Ok(meta) = serde_json::from_str::<Value>(header) else {
        return;
    };
    if meta["type"] != "session_meta" {
        return;
    }
    let p = &meta["payload"];
    let Some(session_id) = p["id"].as_str() else {
        return;
    };
    let git_branch = p["git"]["branch"].as_str().map(str::to_owned);
    let mut cwd = p["cwd"].as_str().map(str::to_owned);
    let mut model = None;

    for (offset, line) in lines_with_offsets(body, base) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let p = &v["payload"];
        match v["type"].as_str() {
            Some("turn_context") => {
                cwd = p["cwd"].as_str().map(str::to_owned).or(cwd);
                model = p["model"].as_str().map(str::to_owned).or(model);
                continue;
            }
            Some("event_msg") if p["type"] == "token_count" => {
                let u = &p["info"]["last_token_usage"];
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                if let Some(m) = out
                    .iter_mut()
                    .rev()
                    .find(|m| m.role == Role::Assistant && m.session_id == session_id)
                    .filter(|m| m.tokens.is_none())
                {
                    m.tokens = Some(Tokens {
                        input: n("input_tokens"),
                        output: n("output_tokens"),
                        cache_read: n("cached_input_tokens"),
                        cache_write: 0,
                    });
                }
                continue;
            }
            Some("event_msg") if p["type"] == "turn_aborted" => {
                if let Some(m) = out
                    .iter_mut()
                    .rev()
                    .find(|m| m.role == Role::Assistant && m.session_id == session_id)
                {
                    m.stop_reason = Some(StopReason::Aborted);
                }
                continue;
            }
            Some("response_item") => {}
            _ => continue,
        }
        let Some(ts) = v["timestamp"].as_str().and_then(ts_rfc3339) else {
            continue;
        };
        let new = |role| {
            let mut m = Message::new(Agent::Codex, session_id, offset.to_string(), ts, role);
            m.cwd.clone_from(&cwd);
            m.git_branch.clone_from(&git_branch);
            m.model.clone_from(&model);
            m
        };
        match p["type"].as_str() {
            Some("message") => {
                let role = match p["role"].as_str() {
                    Some("user") => Role::User,
                    Some("assistant") => Role::Assistant,
                    _ => continue,
                };
                let text: String = p["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|b| b["text"].as_str())
                    .filter(|t| !INJECTED.iter().any(|i| t.starts_with(i)))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.trim().is_empty() {
                    let mut m = new(role);
                    m.content = text;
                    out.push(m);
                }
            }
            Some("function_call" | "custom_tool_call") => {
                let mut m = new(Role::Assistant);
                let input = p.get("arguments").or_else(|| p.get("input"));
                let mut name = p["name"].as_str().unwrap_or_default().to_owned();
                let mut input = input
                    .and_then(Value::as_str)
                    .map_or_else(|| input.map(Value::to_string).unwrap_or_default(), str::to_owned);
                // Code mode wraps the real call in a script; record the call it made.
                if name == "exec"
                    && let Some(cmd) = code_mode_command(&input)
                {
                    "exec_command".clone_into(&mut name);
                    input = json!({"cmd": cmd}).to_string();
                }
                m.tool_calls.push(ToolCall {
                    id: p["call_id"].as_str().unwrap_or_default().to_owned(),
                    name,
                    input,
                });
                out.push(m);
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                let mut m = new(Role::Tool);
                m.tool_use_id = p["call_id"].as_str().map(str::to_owned);
                let (text, exit_code) = output_text(&p["output"]);
                m.is_error = exit_code.is_some_and(|c| c != 0) || exit_failed(&text);
                m.content = cap_output(text);
                out.push(m);
            }
            _ => {}
        }
    }
}

fn parse_index(body: &str, base: u64, out: &mut Vec<Message>) {
    for (offset, line) in lines_with_offsets(body, base) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let (Some(id), Some(name), Some(ts)) = (
            v["id"].as_str(),
            v["thread_name"].as_str(),
            v["updated_at"].as_str().and_then(ts_rfc3339),
        ) else {
            continue;
        };
        let mut m = Message::new(Agent::Codex, id, format!("title:{offset}"), ts, Role::Title);
        name.clone_into(&mut m.content);
        out.push(m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = r#"{"timestamp":"2026-03-03T21:05:36.051Z","type":"session_meta","payload":{"id":"th1","cwd":"/w","git":{"branch":"dev"},"source":"cli"}}"#;
    const BODY: &str = concat!(
        r#"{"timestamp":"2026-03-03T21:05:37.000Z","type":"turn_context","payload":{"cwd":"/w","model":"gpt-5.5"}}"#,
        "\n",
        r#"{"timestamp":"2026-03-03T21:05:38.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"},{"type":"input_text","text":"refactor sync"}]}}"#,
        "\n",
        r#"{"timestamp":"2026-03-03T21:05:39.000Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}"#,
        "\n",
        r#"{"timestamp":"2026-03-03T21:05:40.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"c1","arguments":"{\"cmd\":\"ls\"}"}}"#,
        "\n",
        r#"{"timestamp":"2026-03-03T21:05:41.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"a\nb"}}"#,
        "\n",
        r#"{"timestamp":"2026-03-03T21:05:42.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":7,"output_tokens":3,"cached_input_tokens":1}}}}"#,
        "\n",
        r#"{"timestamp":"2026-03-03T21:05:43.000Z","type":"response_item","payload":{"type":"reasoning","encrypted_content":"..."}}"#,
        "\n",
    );

    #[test]
    fn parses_thread_with_header_state() {
        let mut out = Vec::new();
        parse(Path::new("rollout-x.jsonl"), HEADER, BODY, 0, &mut out);
        let roles: Vec<Role> = out.iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::Tool]);
        assert_eq!(out[0].content, "refactor sync"); // injected block dropped
        assert_eq!(out[0].model.as_deref(), Some("gpt-5.5"));
        assert_eq!(out[0].git_branch.as_deref(), Some("dev"));
        assert_eq!(out[1].tool_calls[0].input, r#"{"cmd":"ls"}"#);
        assert_eq!(out[1].tokens.unwrap().input, 7);
        assert_eq!(out[2].tool_use_id.as_deref(), Some("c1"));
        assert!(!out[2].is_error);
        assert!(exit_failed("Chunk ID: 1\nProcess exited with code 101\nOutput:"));
        assert!(exit_failed(r#"{"output":"x","metadata":{"exit_code":2}}"#));
        assert!(!exit_failed("Process exited with code 0") && !exit_failed("no marker"));

        let mut aborted = Vec::new();
        let body = format!(
            "{BODY}{}\n",
            r#"{"timestamp":"2026-03-03T21:05:44.000Z","type":"event_msg","payload":{"type":"turn_aborted"}}"#
        );
        parse(Path::new("rollout-x.jsonl"), HEADER, &body, 0, &mut aborted);
        assert_eq!(aborted[1].stop_reason, Some(StopReason::Aborted));
        let expected: Vec<String> = [1usize, 3, 4]
            .iter()
            .map(|&n| BODY.lines().take(n).map(|l| l.len() + 1).sum::<usize>().to_string())
            .collect();
        assert_eq!(out.iter().map(|m| m.source_id.clone()).collect::<Vec<_>>(), expected);
    }

    #[test]
    fn to_native_round_trips_through_from_native() {
        use time::macros::datetime;

        let t = datetime!(2026-09-01 10:00:05 UTC);
        let session = Session {
            agent: Agent::ClaudeCode,
            session_id: "0192a7d2-8b6a-7c1e-9d3f-1a2b3c4d5e6f".into(), // a UUID, so it is kept
            parent_session_id: None,
            title: Some("Listing".into()),
            cwd: Some("/".into()),
            git_branch: Some("main".into()),
            model: None,
            started_at: t,
            ended_at: t,
            messages: 2,
            tool_calls: 0,
            threads: 0,
            tokens: Tokens::default(),
            last_stop: None,
        };
        let mut user = Message::new(Agent::ClaudeCode, &session.session_id, "u1", t, Role::User);
        user.content = "ls please".into();
        let mut reply =
            Message::new(Agent::ClaudeCode, &session.session_id, "a1", t, Role::Assistant);
        reply.content = "Done.".into();
        reply.tool_calls.push(ToolCall {
            id: "t1".into(),
            name: "Bash".into(),
            input: r#"{"command":"ls"}"#.into(),
        });
        reply.tool_calls.push(ToolCall {
            id: "t2".into(),
            name: "mcp__atuin__history".into(),
            input: r#"{"q":"x"}"#.into(),
        });
        let mut result =
            Message::new(Agent::ClaudeCode, &session.session_id, "u2#0", t, Role::Tool);
        result.tool_use_id = Some("t1".into());
        result.content = "a\nb".into();

        let root = tempfile::tempdir().unwrap();
        let (id, path) = write_to(root.path(), &session, &[user, reply, result]).unwrap();
        assert_eq!(id, session.session_id);
        let path = path.unwrap();
        assert!(
            path.ends_with(format!("sessions/2026/09/01/rollout-2026-09-01T10-00-05-{id}.jsonl"))
        );

        let text = std::fs::read_to_string(&path).unwrap();
        let (header, body) = text.split_once('\n').unwrap();
        let mut back = Vec::new();
        parse(&path, header, body, header.len() as u64 + 1, &mut back);
        let mut index = Vec::new();
        parse(
            Path::new(INDEX_FILE),
            "",
            &std::fs::read_to_string(root.path().join(INDEX_FILE)).unwrap(),
            0,
            &mut index,
        );
        // Claude's Bash comes back as Codex's own exec_command, each call followed by its output.
        let turns_back = turns(&back);
        assert_eq!(
            turns_back.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["ls please", "Done.", "", ""]
        );
        let calls: Vec<&crate::Call> = turns_back.iter().flat_map(|t| &t.calls).collect();
        assert_eq!(
            (calls[0].name.as_str(), &calls[0].input, calls[0].output.as_str()),
            ("exec_command", &json!({"cmd": "ls"}), "a\nb")
        );
        assert_eq!((calls[1].name.as_str(), calls[1].output.as_str()), ("mcp__atuin__history", ""));
        assert_eq!(tool_name("mcp.atuin/history"), "mcp_atuin_history");
        // The UI twins are written, and ignored on the way back in.
        assert_eq!(text.matches(r#""type":"event_msg""#).count(), 2);
        assert!(back.iter().all(|m| m.session_id == id && m.git_branch.as_deref() == Some("main")));
        assert_eq!((index[0].role, index[0].content.as_str()), (Role::Title, "Listing"));
        assert_eq!(index.len(), 1);

        // Untouched, a handoff is refreshed from its source. Once the agent has added to it,
        // it holds turns that exist nowhere else and is left exactly as it is.
        let stamped = std::fs::read_to_string(&path).unwrap();
        assert_eq!(crate::handoff::Origin::of_file(&path).unwrap().bytes, stamped.len() as u64);
        let continued = format!("{stamped}{{\"the agent\":\"carried on\"}}\n");
        std::fs::write(&path, &continued).unwrap();
        assert_eq!(write_to(root.path(), &session, &[]).unwrap(), (id, None));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), continued);
    }

    #[test]
    fn code_mode_calls_are_unwrapped() {
        let bare = r#"text(await tools.exec_command({cmd:"atuin kv list -n \"chat\"","max_output_tokens":1000}));"#;
        assert_eq!(code_mode_command(bare).as_deref(), Some(r#"atuin kv list -n "chat""#));
        let quoted = r#"const r = await tools.exec_command({"cmd":"jj status"});"#;
        assert_eq!(code_mode_command(quoted).as_deref(), Some("jj status"));
        // More than one tool call, or none, stays a script.
        assert_eq!(
            code_mode_command("await tools.a(); await tools.exec_command({cmd:\"x\"})"),
            None
        );
        assert_eq!(code_mode_command("console.log(1)"), None);

        let blocks = json!([
            {"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
            {"type": "input_text", "text": r#"{"chunk_id":"a","exit_code":3,"output":"boom\n"}"#},
        ]);
        assert_eq!(output_text(&blocks), ("boom\n".to_owned(), Some(3)));
        let plain = json!([{"type": "input_text", "text": "Script completed"}, {"type": "input_text", "text": "hello"}]);
        assert_eq!(output_text(&plain), ("hello".to_owned(), None));
        assert_eq!(output_text(&json!("just text")), ("just text".to_owned(), None));

        let mut out = Vec::new();
        let body = format!(
            "{}\n{}\n",
            json!({"timestamp": "2026-03-03T21:05:40.000Z", "type": "response_item", "payload": {"type": "custom_tool_call", "name": "exec", "call_id": "c9", "input": bare}}),
            json!({"timestamp": "2026-03-03T21:05:41.000Z", "type": "response_item", "payload": {"type": "custom_tool_call_output", "call_id": "c9", "output": blocks}}),
        );
        parse(Path::new("rollout-x.jsonl"), HEADER, &body, 0, &mut out);
        assert_eq!(out[0].tool_calls[0].name, "exec_command");
        assert_eq!(
            turns(&out)[0].calls[0].shell_command().as_deref(),
            Some(r#"atuin kv list -n "chat""#)
        );
        assert_eq!((out[1].content.as_str(), out[1].is_error), ("boom\n", true));
    }

    #[test]
    fn parses_title_index() {
        let mut out = Vec::new();
        let index =
            r#"{"id":"th1","thread_name":"Review sync","updated_at":"2026-09-04T21:38:11.730Z"}"#;
        parse(Path::new(INDEX_FILE), "", index, 0, &mut out);
        assert_eq!(
            (out[0].role, out[0].content.as_str(), out[0].session_id.as_str()),
            (Role::Title, "Review sync", "th1")
        );
    }
}
