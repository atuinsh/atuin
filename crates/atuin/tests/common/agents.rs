//! AI coding agents as the daemon sees them: their transcripts, written the way each agent writes
//! its own, and stand-in executables that only say how they were run.
//!
//! Every turn written here is a prompt, some reasoning, one shell command with its output, and a
//! reply, each holding a word no other turn has, so a test can tell exactly which of them a
//! session, a search or a restored transcript carries.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// A coding agent atuin captures sessions from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Opencode,
    Pi,
}

impl Agent {
    pub const ALL: [Self; 4] = [Self::Claude, Self::Codex, Self::Opencode, Self::Pi];

    /// The `harness` atuin's JSON output names it by.
    pub fn harness(self) -> &'static str {
        match self {
            Self::Claude => "claude-code",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
        }
    }

    /// Its executable, and the name `atuin ai resume --in` takes.
    pub fn program(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
        }
    }

    /// The name it gives the shell tool every turn here calls.
    pub fn tool(self) -> &'static str {
        match self {
            Self::Claude => "Bash",
            Self::Codex => "exec_command",
            Self::Opencode | Self::Pi => "bash",
        }
    }

    /// The name atuin's status lines give it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Opencode => "opencode",
            Self::Pi => "Pi",
        }
    }

    /// A fresh session id of the agent's own shape.
    pub fn new_id(self) -> String {
        let uuid = atuin_common::utils::uuid_v7();
        match self {
            Self::Opencode => format!("ses_{}", uuid.as_simple()),
            _ => uuid.as_hyphenated().to_string(),
        }
    }

    /// Where, under `home`, the agent keeps its sessions: created up front, as a daemon only looks
    /// for a missing one again after a backoff.
    pub fn root(self, home: &Path) -> PathBuf {
        match self {
            Self::Claude => home.join(".claude/projects"),
            Self::Codex => home.join(".codex/sessions"),
            Self::Opencode => home.join(".local/share/opencode/opencode.db"),
            Self::Pi => home.join(".pi/agent/sessions"),
        }
    }

    /// Lay out the agent's data under `home` as the agent itself would before its first session.
    pub fn install(self, home: &Path) {
        match self {
            Self::Opencode => opencode_db(home),
            // Codex keeps a lock per open thread here; with the directory present, atuin asks the
            // locks whether a session is open, rather than every `codex` process on the machine.
            Self::Codex => {
                fs::create_dir_all(self.root(home)).unwrap();
                fs::create_dir_all(home.join(".codex/thread-writer-locks")).unwrap();
            }
            _ => fs::create_dir_all(self.root(home)).unwrap(),
        }
    }
}

/// A word found nowhere else, for searches and for telling turns apart.
pub fn word() -> String {
    format!("w{}", atuin_common::utils::uuid_v7().as_simple())
}

/// One exchange: a prompt, reasoning, a shell command and its output, and the reply.
#[derive(Clone, Debug)]
pub struct Turn {
    pub prompt: String,
    pub thinking: String,
    pub command: String,
    pub output: String,
    pub reply: String,
}

impl Turn {
    pub fn new() -> Self {
        Self {
            prompt: format!("please look into {}", word()),
            thinking: format!("thinking about {}", word()),
            command: format!("echo {}", word()),
            output: format!("output {}", word()),
            reply: format!("all done with {}", word()),
        }
    }

    /// What this turn says in conversation, as a session's messages carry it: its prompt and its
    /// reply.
    pub fn said(&self) -> [&str; 2] {
        [&self.prompt, &self.reply]
    }

    /// The turn as a transcript holds it, reasoning aside: its prompt, the command it ran, what
    /// that printed, and its reply.
    pub fn whole(&self) -> [&str; 4] {
        [&self.prompt, &self.command, &self.output, &self.reply]
    }
}

/// Everything `turns` hold, reasoning aside, in order (see [`Turn::whole`]).
pub fn whole(turns: &[Turn]) -> Vec<String> {
    turns.iter().flat_map(|t| t.whole().map(str::to_owned)).collect()
}

/// Everything `turns` say in conversation, in order.
pub fn said(turns: &[Turn]) -> Vec<String> {
    turns.iter().flat_map(|t| t.said().map(str::to_owned)).collect()
}

/// A session's transcript on one machine.
#[derive(Clone, Debug)]
pub struct Transcript {
    pub agent: Agent,
    pub id: String,
    /// The transcript file, or for opencode its database.
    pub path: PathBuf,
}

/// Each line a millisecond after the last, so every agent orders them as written.
fn tick() -> OffsetDateTime {
    static LAST: AtomicI64 = AtomicI64::new(0);
    let now = i64::try_from(OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap();
    let last =
        LAST.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last| Some(now.max(last + 1)));
    let ms = now.max(last.unwrap() + 1);
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).unwrap()
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap()
}

fn millis(at: OffsetDateTime) -> i64 {
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap()
}

fn short() -> String {
    atuin_common::utils::uuid_v7().as_simple().to_string()
}

impl Transcript {
    /// Write a new session of `agent`, working in `cwd`, under `home`.
    pub fn create(home: &Path, agent: Agent, cwd: &Path, turns: &[Turn]) -> Self {
        let id = agent.new_id();
        let started = tick();
        let path = match agent {
            Agent::Claude => agent.root(home).join(claude_project(cwd)).join(format!("{id}.jsonl")),
            Agent::Codex => {
                let day = started.format(time::macros::format_description!("[year]/[month]/[day]"));
                let stamp = started.format(time::macros::format_description!(
                    "[year]-[month]-[day]T[hour]-[minute]-[second]"
                ));
                agent
                    .root(home)
                    .join(day.unwrap())
                    .join(format!("rollout-{}-{id}.jsonl", stamp.unwrap()))
            }
            Agent::Pi => {
                let dir = format!(
                    "--{}--",
                    cwd.display().to_string().trim_matches('/').replace('/', "-")
                );
                let stamp = rfc3339(started).replace([':', '.'], "-");
                agent.root(home).join(dir).join(format!("{stamp}_{id}.jsonl"))
            }
            Agent::Opencode => agent.root(home),
        };
        let transcript = Self { agent, id, path };
        match agent {
            Agent::Claude | Agent::Pi | Agent::Codex => {
                fs::create_dir_all(transcript.path.parent().unwrap()).unwrap();
                let header = match agent {
                    Agent::Codex => vec![json!({
                        "timestamp": rfc3339(started),
                        "type": "session_meta",
                        "payload": {
                            "id": transcript.id,
                            "timestamp": rfc3339(started),
                            "cwd": cwd,
                            "originator": "codex_cli_rs",
                            "cli_version": "0.50.0",
                            "instructions": null,
                            "git": {"branch": "main"},
                        },
                    })],
                    Agent::Pi => vec![json!({
                        "type": "session",
                        "version": 3,
                        "id": transcript.id,
                        "timestamp": rfc3339(started),
                        "cwd": cwd,
                    })],
                    _ => Vec::new(),
                };
                write_lines(&transcript.path, &header);
            }
            Agent::Opencode => {
                let title = format!("opencode session {}", word());
                opencode_events(home, &[(
                    "session.created.1",
                    json!({
                        "sessionID": transcript.id,
                        "info": {
                            "id": transcript.id,
                            "title": title,
                            "directory": cwd,
                            "time": {"created": millis(started), "updated": millis(started)},
                        },
                    }),
                )]);
            }
        }
        transcript.append(cwd, turns);
        transcript
    }

    /// A Claude Code subagent `parent` spawned, working in `cwd`: its own transcript beside the
    /// parent's, whose lines name the parent's session.
    pub fn claude_subagent(parent: &Self, cwd: &Path, turns: &[Turn]) -> Self {
        assert_eq!(parent.agent, Agent::Claude);
        let id = format!("agent-{}", &short()[16..]);
        let dir = parent.path.with_extension("").join("subagents");
        fs::create_dir_all(&dir).unwrap();
        let subagent = Self {
            agent: Agent::Claude,
            path: dir.join(format!("{id}.jsonl")),
            id,
        };
        subagent.append(cwd, turns);
        subagent
    }

    /// Where this agent keeps the transcript of session `id` under `home`, when it has one.
    pub fn find(home: &Path, agent: Agent, id: &str) -> Option<Self> {
        assert_ne!(agent, Agent::Opencode, "opencode keeps every session in one database");
        let mut dirs = vec![agent.root(home)];
        while let Some(dir) = dirs.pop() {
            // A directory gone or unreadable holds nothing to find: the rest may.
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.file_stem().unwrap().to_string_lossy().ends_with(id) {
                    let id = id.to_owned();
                    return Some(Self { agent, id, path });
                }
            }
        }
        None
    }

    /// The transcript as it stands.
    pub fn read(&self) -> String {
        fs::read_to_string(&self.path).unwrap()
    }

    /// Carry the session on with `turns`, as the agent would: after whatever its transcript ends
    /// with, whoever wrote it.
    pub fn append(&self, cwd: &Path, turns: &[Turn]) {
        match self.agent {
            Agent::Claude => self.append_claude(cwd, turns),
            Agent::Codex => self.append_codex(turns),
            Agent::Pi => self.append_pi(turns),
            Agent::Opencode => self.append_opencode(cwd, turns),
        }
    }

    /// The last line of the transcript that has `key`, as the agent finds where to go on from.
    fn last(&self, key: &str) -> Option<String> {
        fs::read_to_string(&self.path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|line| line["type"] != "session")
            .filter_map(|line| line[key].as_str().map(str::to_owned))
            .next_back()
    }

    fn append_claude(&self, cwd: &Path, turns: &[Turn]) {
        let mut parent = self.last("uuid");
        // A subagent's lines name the session it works for.
        let session = match self.id.strip_prefix("agent-") {
            Some(_) => self
                .path
                .ancestors()
                .nth(2)
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            None => self.id.clone(),
        };
        let mut lines = Vec::new();
        let mut line = |kind: &str, message: Value| {
            let uuid = uuid::Uuid::new_v4().to_string();
            lines.push(json!({
                "parentUuid": parent,
                "isSidechain": session != self.id,
                "userType": "external",
                "cwd": cwd,
                "sessionId": session,
                "version": "2.1.0",
                "gitBranch": "main",
                "type": kind,
                "message": message,
                "uuid": uuid,
                "timestamp": rfc3339(tick()),
            }));
            parent = Some(uuid);
        };
        for turn in turns {
            let call = format!("toolu_{}", short());
            let usage = json!({
                "input_tokens": 10,
                "output_tokens": 5,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            });
            line("user", json!({"role": "user", "content": turn.prompt}));
            line(
                "assistant",
                json!({
                    "id": format!("msg_{}", short()),
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-opus-5-5",
                    "content": [
                        {"type": "thinking", "thinking": turn.thinking, "signature": "sig"},
                        {"type": "tool_use", "id": call, "name": self.agent.tool(), "input": {"command": turn.command}},
                    ],
                    "stop_reason": "tool_use",
                    "usage": usage,
                }),
            );
            line(
                "user",
                json!({
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": call, "content": turn.output, "is_error": false}],
                }),
            );
            line(
                "assistant",
                json!({
                    "id": format!("msg_{}", short()),
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-opus-5-5",
                    "content": [{"type": "text", "text": turn.reply}],
                    "stop_reason": "end_turn",
                    "usage": usage,
                }),
            );
        }
        write_lines(&self.path, &lines);
    }

    fn append_codex(&self, turns: &[Turn]) {
        let mut lines = Vec::new();
        let mut item = |payload: Value| {
            lines.push(
                json!({"timestamp": rfc3339(tick()), "type": "response_item", "payload": payload}),
            );
        };
        for turn in turns {
            let call = format!("call_{}", short());
            item(
                json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": turn.prompt}]}),
            );
            item(json!({
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": turn.thinking}],
                "encrypted_content": null,
            }));
            item(json!({
                "type": "function_call",
                "name": self.agent.tool(),
                "arguments": json!({"cmd": turn.command}).to_string(),
                "call_id": call,
            }));
            item(json!({
                "type": "function_call_output",
                "call_id": call,
                "output": json!({"output": turn.output, "metadata": {"exit_code": 0}}).to_string(),
            }));
            item(
                json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": turn.reply}]}),
            );
        }
        write_lines(&self.path, &lines);
    }

    fn append_pi(&self, turns: &[Turn]) {
        let mut parent = self.last("id");
        let mut lines = Vec::new();
        let mut entry = |message: Value| {
            let id = short()[24..].to_owned();
            let at = tick();
            let mut message = message;
            message["timestamp"] = json!(millis(at));
            lines.push(json!({
                "type": "message",
                "id": id,
                "parentId": parent,
                "timestamp": rfc3339(at),
                "message": message,
            }));
            parent = Some(id);
        };
        let usage = json!({
            "input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 15,
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0},
        });
        for turn in turns {
            let call = format!("toolu_{}", short());
            entry(json!({"role": "user", "content": [{"type": "text", "text": turn.prompt}]}));
            entry(json!({
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": turn.thinking},
                    {"type": "toolCall", "id": call, "name": self.agent.tool(), "arguments": {"command": turn.command}},
                ],
                "api": "anthropic-messages",
                "provider": "anthropic",
                "model": "claude-opus-5-5",
                "usage": usage,
                "stopReason": "toolUse",
            }));
            entry(json!({
                "role": "toolResult",
                "toolCallId": call,
                "toolName": self.agent.tool(),
                "content": [{"type": "text", "text": turn.output}],
                "isError": false,
            }));
            entry(json!({
                "role": "assistant",
                "content": [{"type": "text", "text": turn.reply}],
                "api": "anthropic-messages",
                "provider": "anthropic",
                "model": "claude-opus-5-5",
                "usage": usage,
                "stopReason": "stop",
            }));
        }
        write_lines(&self.path, &lines);
    }

    fn append_opencode(&self, cwd: &Path, turns: &[Turn]) {
        let home = self.path.ancestors().nth(4).unwrap();
        let mut events = Vec::new();
        for turn in turns {
            let (asked, answered) = (tick(), tick());
            let (user, assistant) = (format!("msg_{}", short()), format!("msg_{}", short()));
            // A part's own clock, else the time it was written, orders it.
            let part = |message: &str, part: Value, at: OffsetDateTime| {
                let mut part = part;
                part["id"] = json!(format!("prt_{}", short()));
                part["messageID"] = json!(message);
                part["sessionID"] = json!(self.id);
                (
                    "message.part.updated.1",
                    json!({"sessionID": self.id, "part": part, "time": millis(at)}),
                )
            };
            let done = |at: OffsetDateTime| json!({"start": millis(at), "end": millis(tick())});
            events.push(("message.updated.1", json!({
                "sessionID": self.id,
                "info": {"id": user, "sessionID": self.id, "role": "user", "time": {"created": millis(asked)}},
            })));
            events.push(part(&user, json!({"type": "text", "text": turn.prompt}), asked));
            events.push((
                "message.updated.1",
                json!({
                    "sessionID": self.id,
                    "info": {
                        "id": assistant,
                        "sessionID": self.id,
                        "role": "assistant",
                        "parentID": user,
                        "modelID": "claude-opus-5-5",
                        "providerID": "anthropic",
                        "path": {"cwd": cwd, "root": cwd},
                        "time": {"created": millis(answered)},
                    },
                }),
            ));
            events.push(part(
                &assistant,
                json!({"type": "reasoning", "text": turn.thinking, "time": done(answered)}),
                answered,
            ));
            events.push(part(
                &assistant,
                json!({
                    "type": "tool",
                    "callID": format!("call_{}", short()),
                    "tool": self.agent.tool(),
                    "state": {
                        "status": "completed",
                        "input": {"command": turn.command},
                        "output": turn.output,
                        "time": done(answered),
                    },
                }),
                answered,
            ));
            events.push(part(
                &assistant,
                json!({"type": "text", "text": turn.reply, "time": done(answered)}),
                answered,
            ));
        }
        opencode_events(home, &events);
    }
}

fn write_lines(path: &Path, lines: &[Value]) {
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    let mut text = String::new();
    for line in lines {
        text.push_str(&line.to_string());
        text.push('\n');
    }
    // One write, so the daemon never reads half a turn.
    file.write_all(text.as_bytes()).unwrap();
}

/// Claude Code's directory for the project at `cwd`: every character but a letter or digit a dash.
fn claude_project(cwd: &Path) -> String {
    cwd.display()
        .to_string()
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

/// opencode's database: the event log it records sessions in, and the tables it projects them
/// into.
fn opencode_db(home: &Path) {
    let path = Agent::Opencode.root(home);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE event (id TEXT PRIMARY KEY, aggregate_id TEXT NOT NULL, seq INTEGER NOT \
         NULL, type TEXT NOT NULL, data TEXT NOT NULL);
         CREATE UNIQUE INDEX event_aggregate_seq_idx ON event (aggregate_id, seq);
         CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL, title \
         TEXT NOT NULL, revert TEXT, time_created INTEGER NOT NULL, time_updated INTEGER NOT \
         NULL);
         CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER \
         NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
         CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT \
         NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);",
    )
    .unwrap();
}

/// Append `events` to opencode's event log, each continuing its session's sequence, and project
/// them, in one transaction as opencode commits them.
fn opencode_events(home: &Path, events: &[(&str, Value)]) {
    let mut db = rusqlite::Connection::open(Agent::Opencode.root(home)).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    let tx = db.transaction().unwrap();
    for (kind, data) in events {
        let session = data["sessionID"].as_str().unwrap();
        tx.execute(
            "INSERT INTO event (id, aggregate_id, seq, type, data) VALUES (?1, ?2, (SELECT \
             coalesce(max(seq), -1) + 1 FROM event WHERE aggregate_id = ?2), ?3, ?4)",
            rusqlite::params![format!("evt_{}", short()), session, kind, data.to_string()],
        )
        .unwrap();
        // The projections keep a row's info without the ids their columns hold.
        let without = |value: &Value, keys: &[&str]| {
            let mut value = value.clone();
            for key in keys {
                value.as_object_mut().unwrap().remove(*key);
            }
            value.to_string()
        };
        match *kind {
            "session.created.1" => {
                let info = &data["info"];
                tx.execute(
                    "INSERT INTO session (id, directory, title, time_created, time_updated) \
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    rusqlite::params![
                        session,
                        info["directory"].as_str(),
                        info["title"].as_str(),
                        info["time"]["created"].as_i64()
                    ],
                )
                .unwrap();
            }
            "message.updated.1" => {
                let info = &data["info"];
                let created = info["time"]["created"].as_i64();
                tx.execute(
                    "INSERT INTO message (id, session_id, time_created, time_updated, data) \
                     VALUES (?1, ?2, ?3, ?3, ?4)",
                    rusqlite::params![
                        info["id"].as_str(),
                        session,
                        created,
                        without(info, &["id", "sessionID"])
                    ],
                )
                .unwrap();
            }
            _ => {
                let part = &data["part"];
                tx.execute(
                    "INSERT INTO part (id, message_id, session_id, time_created, time_updated, \
                     data) VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                    rusqlite::params![
                        part["id"].as_str(),
                        part["messageID"].as_str(),
                        session,
                        data["time"].as_i64(),
                        without(part, &["id", "messageID", "sessionID"])
                    ],
                )
                .unwrap();
            }
        }
    }
    tx.commit().unwrap();
}

/// Stand-ins for the agents' executables, on `PATH` ahead of anything installed: each prints how
/// it was run and where, and exits.
pub fn install_fakes(bin: &Path, agents: &[Agent]) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::create_dir_all(bin).unwrap();
    for agent in agents {
        let path = bin.join(agent.program());
        fs::write(&path, format!("#!/bin/sh\necho \"AGENT-RAN {} $* IN $PWD\"\n", agent.program()))
            .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// The line a fake agent prints when run with `args` in `cwd`.
pub fn ran(agent: Agent, args: &str, cwd: &Path) -> String {
    format!("AGENT-RAN {} {args} IN {}", agent.program(), cwd.display())
}
