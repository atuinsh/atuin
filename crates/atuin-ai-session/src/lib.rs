//! Normalised AI coding-agent sessions.
//!
//! Every supported agent (Claude Code, Codex, OpenCode, Cursor) flattens onto one [`Message`]
//! per transcript line. There are no session records: a [`Session`] is whatever shares an
//! `(agent, session_id)`, derived on demand, the same way shell history infers sessions from a
//! session id. Session context (cwd, branch, model) is denormalised onto every message so a
//! partially synced session is still searchable.

pub mod claude_code;
pub mod codex;
pub mod cursor;
pub mod handoff;
pub mod ingest;
pub mod opencode;
pub mod pi;
pub mod resume;
pub mod store;

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use eyre::Result;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

/// Tool results larger than this are cut at ingest.
///
/// Measured p99 across all four agents on a real machine was under 110KB; the max was a 4MB
/// Codex command output nobody wants to sync.
pub const MAX_TOOL_OUTPUT: usize = 64 * 1024;

/// Longest derived title, in chars.
const TITLE_LEN: usize = 80;

/// Which agent produced a message. Stored as its `u8` discriminant.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    strum_macros::Display,
    strum_macros::EnumString,
    strum_macros::FromRepr,
)]
#[strum(serialize_all = "kebab-case")]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum Agent {
    ClaudeCode = 1,
    Codex = 2,
    #[strum(serialize = "opencode")]
    #[serde(rename = "opencode")]
    OpenCode = 3,
    Cursor = 4,
    Pi = 5,
}

impl Agent {
    pub const ALL: [Self; 5] =
        [Self::ClaudeCode, Self::Codex, Self::OpenCode, Self::Cursor, Self::Pi];
}

/// Who wrote a message. Stored as its `u8` discriminant.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    strum_macros::Display,
    strum_macros::EnumString,
    strum_macros::FromRepr,
)]
#[strum(serialize_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Role {
    User = 1,
    Assistant = 2,
    /// Agent-injected context, mostly compaction summaries.
    System = 3,
    /// A tool result. `Message::tool_use_id` names the call it answers.
    Tool = 4,
    /// A title the agent assigned to the session. The newest one wins.
    Title = 5,
}

/// Why an assistant turn ended. Stored as its `u8` discriminant; reasons an agent does not
/// report, or that we do not model, are simply absent.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    strum_macros::Display,
    strum_macros::FromRepr,
)]
#[strum(serialize_all = "kebab-case")]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum StopReason {
    EndTurn = 1,
    ToolUse = 2,
    MaxTokens = 3,
    /// The user interrupted the turn.
    Aborted = 4,
    /// The provider or the agent failed mid-turn.
    Error = 5,
}

impl StopReason {
    /// Whether the turn ended without the agent finishing what it was doing.
    #[must_use]
    pub fn is_unfinished(self) -> bool {
        matches!(self, Self::MaxTokens | Self::Aborted | Self::Error)
    }
}

/// `source_id` prefix of a title the user set by hand. It outranks any title an agent generated,
/// however new.
pub const CUSTOM_TITLE_PREFIX: &str = "custom-title:";

/// Whether user text is something the user typed, as opposed to markup an agent injected into
/// the user turn (`<command-name>`, `<environment_context>`, `[Request interrupted…]`).
#[must_use]
pub fn is_prompt(text: &str) -> bool {
    !matches!(text.trim_start().chars().next(), None | Some('<' | '['))
}

/// A tool invocation on an assistant message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The agent's call id. The matching `Role::Tool` message carries it as `tool_use_id`.
    pub id: String,
    pub name: String,
    /// Arguments, JSON-encoded exactly as the agent recorded them.
    pub input: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl std::ops::AddAssign for Tokens {
    fn add_assign(&mut self, rhs: Self) {
        self.input += rhs.input;
        self.output += rhs.output;
        self.cache_read += rhs.cache_read;
        self.cache_write += rhs.cache_write;
    }
}

/// One line of an agent transcript, in a shape shared by every agent.
///
/// Dedupe key is `(agent, session_id, source_id)`, never `id`: `id` is Atuin's, assigned at
/// ingest, so re-reading the same transcript is idempotent by the key, not by the id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub agent: Agent,
    /// The agent's own session id.
    pub session_id: String,
    /// The session this one descends from: an OpenCode subagent's parent, or the session Atuin
    /// handed to this agent.
    pub parent_session_id: Option<String>,
    /// The parent's agent when it is not this one, which is what marks a handoff.
    #[serde(default)]
    pub parent_agent: Option<Agent>,
    /// A sidechain within the session (Claude Code subagent id). `None` is the main thread.
    pub thread: Option<String>,
    /// The agent's own message id, or its line number when it has none (Codex).
    pub source_id: String,
    pub parent_source_id: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    pub role: Role,
    /// Text, or for `Role::Tool` the result capped by [`cap_output`].
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_use_id: Option<String>,
    /// On `Role::Tool`: the call reported failure.
    #[serde(default)]
    pub is_error: bool,
    /// On `Role::Assistant`: why the turn ended, when the agent says.
    #[serde(default)]
    pub stop_reason: Option<StopReason>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    pub tokens: Option<Tokens>,
}

impl Message {
    pub fn new(
        agent: Agent,
        session_id: impl Into<String>,
        source_id: impl Into<String>,
        timestamp: OffsetDateTime,
        role: Role,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            agent,
            session_id: session_id.into(),
            parent_session_id: None,
            parent_agent: None,
            thread: None,
            source_id: source_id.into(),
            parent_source_id: None,
            timestamp,
            role,
            content: String::new(),
            tool_calls: Vec::new(),
            tool_use_id: None,
            is_error: false,
            stop_reason: None,
            cwd: None,
            git_branch: None,
            model: None,
            tokens: None,
        }
    }
}

/// Read an agent's own history on this machine into the store.
#[allow(async_fn_in_trait, reason = "dispatched statically by `Agent`, never boxed")]
pub trait FromNative {
    const AGENT: Agent;

    /// Pull whatever is new since the last run.
    async fn ingest(store: &store::Store) -> Result<ingest::Stats>;
}

/// Write a session into an agent's own history so that agent can reopen it.
#[allow(async_fn_in_trait, reason = "dispatched statically by `Agent`, never boxed")]
pub trait ToNative {
    const AGENT: Agent;

    /// Returns the id the agent knows the session by. A no-op when the agent already has it.
    async fn write(session: &Session, messages: &[Message]) -> Result<String>;

    /// The command that reopens `native_id` in this agent. The caller sets the directory.
    fn resume(native_id: &str) -> Command;
}

/// One user or assistant turn of a session's main thread, with each tool call already paired
/// to its result.
///
/// What every [`ToNative`] writes. Agents reject history where a call has no result or a result
/// names a call that was never made, so pairing happens once, here: a call with no recorded
/// result keeps an empty `output`, and a result with no call becomes a plain user turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub role: Role,
    pub text: String,
    pub timestamp: OffsetDateTime,
    pub calls: Vec<Call>,
}

/// A tool call and what came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub output: String,
    pub is_error: bool,
}

/// Names the agents give their shell tool. A shell call is worth translating between agents,
/// since each model recognises its own; every other tool passes through by name.
const SHELL_TOOLS: [&str; 6] =
    ["bash", "shell", "shell_command", "exec_command", "local_shell", "run_terminal_cmd"];

impl Call {
    /// The command line, when this is a shell call.
    #[must_use]
    pub fn shell_command(&self) -> Option<String> {
        if !SHELL_TOOLS.contains(&self.name.to_ascii_lowercase().as_str()) {
            return None;
        }
        match self.input.get("command").or_else(|| self.input.get("cmd"))? {
            serde_json::Value::String(s) => Some(s.clone()),
            // Codex's older `shell` tool takes argv, usually `["bash", "-lc", "<command>"]`.
            serde_json::Value::Array(argv) => {
                let argv: Vec<&str> = argv.iter().filter_map(serde_json::Value::as_str).collect();
                match argv.as_slice() {
                    [_, "-lc" | "-c", command] => Some((*command).to_owned()),
                    _ => Some(argv.join(" ")),
                }
            }
            _ => None,
        }
    }
}

const ORPHAN_RESULT_LEN: usize = 1_000;

/// A session's main thread as [`Turn`]s.
#[must_use]
pub fn turns(messages: &[Message]) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    for m in messages.iter().filter(|m| m.thread.is_none()) {
        let turn = |role, text| Turn {
            role,
            text,
            timestamp: m.timestamp,
            calls: Vec::new(),
        };
        match m.role {
            Role::Title => {}
            Role::User => out.push(turn(Role::User, m.content.clone())),
            Role::System => out.push(turn(Role::User, format!("[context summary]\n{}", m.content))),
            Role::Assistant => {
                let mut t = turn(Role::Assistant, m.content.clone());
                t.calls = m
                    .tool_calls
                    .iter()
                    .map(|tc| Call {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        // Agents record arguments as JSON, or as a bare string (Codex patches).
                        input: serde_json::from_str(&tc.input)
                            .unwrap_or_else(|_| serde_json::Value::String(tc.input.clone())),
                        output: String::new(),
                        is_error: false,
                    })
                    .collect();
                out.push(t);
            }
            Role::Tool => {
                let call = m.tool_use_id.as_deref().and_then(|id| {
                    out.iter_mut().rev().flat_map(|t| t.calls.iter_mut()).find(|c| c.id == id)
                });
                match call {
                    Some(call) => {
                        call.output.clone_from(&m.content);
                        call.is_error = m.is_error;
                    }
                    None => out.push(turn(
                        Role::User,
                        format!("[tool result]\n{}", clip(&m.content, ORPHAN_RESULT_LEN)),
                    )),
                }
            }
        }
    }
    out.retain(|t| !t.text.trim().is_empty() || !t.calls.is_empty());
    out
}

/// The first `max` chars, with an ellipsis when anything was dropped.
#[must_use]
pub fn clip(s: &str, max: usize) -> String {
    let mut it = s.chars();
    let head: String = it.by_ref().take(max).collect();
    if it.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// A stable UUID for a session in another agent: the session's own id when it is one, else a
/// v5 UUID over `(agent, session_id)`, so re-opening the same session lands on the same id.
#[must_use]
pub fn native_uuid(session: &Session) -> Uuid {
    session.session_id.parse().unwrap_or_else(|_| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_OID,
            format!("{}:{}", session.agent, session.session_id).as_bytes(),
        )
    })
}

/// The session's directory if it exists here, else where we are. Every resume runs there.
#[must_use]
pub fn working_dir(session: &Session) -> std::path::PathBuf {
    session
        .cwd
        .as_deref()
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

/// Cut a tool result to [`MAX_TOOL_OUTPUT`] bytes on a char boundary, noting what was dropped.
#[must_use]
pub fn cap_output(mut s: String) -> String {
    if s.len() <= MAX_TOOL_OUTPUT {
        return s;
    }
    let dropped = s.len() - MAX_TOOL_OUTPUT;
    let mut cut = MAX_TOOL_OUTPUT;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s.push_str(&format!("\n… [{dropped} bytes truncated]"));
    s
}

pub(crate) fn ts_rfc3339(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

pub(crate) fn ts_millis(ms: i64) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()
}

/// Lines of a JSONL body with the absolute byte offset each one starts at.
pub(crate) fn lines_with_offsets(body: &str, base: u64) -> impl Iterator<Item = (u64, &str)> {
    let mut offset = base;
    body.split_inclusive('\n').map(move |line| {
        let at = offset;
        offset += line.len() as u64;
        (at, line.trim_end_matches(['\n', '\r']))
    })
}

/// A session, derived from its messages. Never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub agent: Agent,
    pub session_id: String,
    pub parent_session_id: Option<String>,
    /// The newest `Role::Title`, else the first line of the first user message.
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ended_at: OffsetDateTime,
    pub messages: usize,
    pub tool_calls: usize,
    /// Distinct sidechains, excluding the main thread.
    pub threads: usize,
    pub tokens: Tokens,
    /// How the newest main-thread assistant turn ended.
    #[serde(default)]
    pub last_stop: Option<StopReason>,
}

/// Group messages into sessions, newest activity first.
pub fn sessions<'a>(messages: impl IntoIterator<Item = &'a Message>) -> Vec<Session> {
    struct Acc<'a> {
        session: Session,
        /// Ranked by (set by hand, newest).
        title: Option<((bool, OffsetDateTime), &'a str)>,
        first_user: Option<(OffsetDateTime, &'a str)>,
        last_assistant: Option<OffsetDateTime>,
        threads: BTreeSet<&'a str>,
    }

    let mut by_key: BTreeMap<(Agent, &str), Acc<'_>> = BTreeMap::new();
    for m in messages {
        let acc = by_key.entry((m.agent, &m.session_id)).or_insert_with(|| Acc {
            session: Session {
                agent: m.agent,
                session_id: m.session_id.clone(),
                parent_session_id: None,
                title: None,
                cwd: None,
                git_branch: None,
                model: None,
                started_at: m.timestamp,
                ended_at: m.timestamp,
                messages: 0,
                tool_calls: 0,
                threads: 0,
                tokens: Tokens::default(),
                last_stop: None,
            },
            title: None,
            first_user: None,
            last_assistant: None,
            threads: BTreeSet::new(),
        });
        let s = &mut acc.session;
        s.started_at = s.started_at.min(m.timestamp);
        s.ended_at = s.ended_at.max(m.timestamp);
        s.messages += 1;
        s.tool_calls += m.tool_calls.len();
        if let Some(t) = m.tokens {
            s.tokens += t;
        }
        // Newest value wins for context that can change mid-session.
        s.parent_session_id = m.parent_session_id.clone().or(s.parent_session_id.take());
        s.cwd = m.cwd.clone().or(s.cwd.take());
        s.git_branch = m.git_branch.clone().or(s.git_branch.take());
        s.model = m.model.clone().or(s.model.take());
        if let Some(t) = &m.thread {
            acc.threads.insert(t);
        }
        match m.role {
            Role::Title => {
                let rank = (m.source_id.starts_with(CUSTOM_TITLE_PREFIX), m.timestamp);
                if acc.title.is_none_or(|(best, _)| rank >= best) {
                    acc.title = Some((rank, &m.content));
                }
            }
            Role::User
                if m.thread.is_none()
                    && is_prompt(&m.content)
                    && acc.first_user.is_none_or(|(ts, _)| m.timestamp < ts) =>
            {
                acc.first_user = Some((m.timestamp, &m.content));
            }
            Role::Assistant
                if m.thread.is_none() && acc.last_assistant.is_none_or(|ts| m.timestamp >= ts) =>
            {
                acc.last_assistant = Some(m.timestamp);
                s.last_stop = m.stop_reason;
            }
            _ => {}
        }
    }

    let mut out: Vec<Session> = by_key
        .into_values()
        .map(|acc| {
            let mut s = acc.session;
            s.threads = acc.threads.len();
            s.title = acc
                .title
                .map(|(_, text)| text)
                .or(acc.first_user.map(|(_, text)| text))
                .map(|text| {
                    text.lines().next().unwrap_or("").chars().take(TITLE_LEN).collect::<String>()
                })
                .filter(|t| !t.trim().is_empty());
            s
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.ended_at));
    out
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn msg(agent: Agent, session: &str, at: OffsetDateTime, role: Role, content: &str) -> Message {
        let mut m = Message::new(agent, session, Uuid::now_v7().to_string(), at, role);
        m.content = content.into();
        m
    }

    #[test]
    fn cap_output_cuts_on_char_boundary_and_notes_dropped_bytes() {
        let s = "é".repeat(MAX_TOOL_OUTPUT); // 2 bytes each, so byte MAX_TOOL_OUTPUT splits a char
        let capped = cap_output(s);
        assert!(capped.starts_with("éé"));
        assert!(capped.ends_with(&format!("[{MAX_TOOL_OUTPUT} bytes truncated]")));
        assert!(cap_output("short".into()) == "short");
    }

    #[test]
    fn sessions_are_derived_from_messages() {
        let t0 = datetime!(2026-09-01 10:00 UTC);
        let t1 = datetime!(2026-09-01 10:05 UTC);
        let t2 = datetime!(2026-09-01 12:00 UTC); // a resume, two hours later
        let t3 = datetime!(2026-09-02 09:00 UTC);

        let mut a0 = msg(Agent::ClaudeCode, "a", t0, Role::User, "fix the flaky test\nplease");
        a0.cwd = Some("/proj".into());
        let mut a1 = msg(Agent::ClaudeCode, "a", t1, Role::Assistant, "");
        a1.tool_calls.push(ToolCall {
            id: "t1".into(),
            name: "Bash".into(),
            input: "{}".into(),
        });
        a1.tokens = Some(Tokens {
            input: 10,
            output: 5,
            ..Default::default()
        });
        let mut a2 = msg(Agent::ClaudeCode, "a", t2, Role::User, "sub task");
        a2.thread = Some("agent-1".into());
        let mut a3 = msg(Agent::ClaudeCode, "a", t1, Role::Title, "Flaky auth test");
        a3.cwd = None;
        let b0 = msg(Agent::Codex, "b", t3, Role::User, "<environment_context>x");
        let mut b1 = msg(Agent::Codex, "b", t3, Role::User, "refactor sync");
        b1.tokens = Some(Tokens {
            input: 1,
            output: 1,
            ..Default::default()
        });

        let got = sessions([&a0, &a1, &a2, &a3, &b0, &b1]);
        assert_eq!(got.len(), 2);

        let b = &got[0]; // newest activity first
        assert_eq!(b.agent, Agent::Codex);
        assert_eq!(b.title.as_deref(), Some("refactor sync")); // injected markup is not a prompt
        assert!(
            !is_prompt("  [Request interrupted by user]") && !is_prompt("") && is_prompt("fix it")
        );
        assert_eq!(b.messages, 2);

        let a = &got[1];
        assert_eq!(a.title.as_deref(), Some("Flaky auth test")); // ai title beats first prompt
        assert_eq!((a.started_at, a.ended_at), (t0, t2)); // resume gap stays one session
        assert_eq!(a.cwd.as_deref(), Some("/proj")); // None on later messages does not erase
        assert_eq!((a.messages, a.tool_calls, a.threads), (4, 1, 1));
        assert_eq!(
            a.tokens,
            Tokens {
                input: 10,
                output: 5,
                ..Default::default()
            }
        );
    }

    #[test]
    fn turns_pair_calls_with_results_and_demote_orphans() {
        let t = datetime!(2026-09-01 10:00 UTC);
        let user = msg(Agent::Codex, "s", t, Role::User, "ls please");
        let mut call = msg(Agent::Codex, "s", t, Role::Assistant, "Sure.");
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
        let mut result = msg(Agent::Codex, "s", t, Role::Tool, "a\nb");
        result.tool_use_id = Some("c".into());
        let mut orphan = msg(Agent::Codex, "s", t, Role::Tool, "stray");
        orphan.tool_use_id = Some("never-called".into());
        let title = msg(Agent::Codex, "s", t, Role::Title, "Listing");
        let mut side = msg(Agent::Codex, "s", t, Role::User, "hidden");
        side.thread = Some("sub".into());

        let got = turns(&[user, call, result, orphan, title, side]);
        assert_eq!(
            got.iter().map(|t| (t.role, t.text.as_str())).collect::<Vec<_>>(),
            [
                (Role::User, "ls please"),
                (Role::Assistant, "Sure."),
                (Role::User, "[tool result]\nstray"),
            ]
        );
        let calls = &got[1].calls;
        assert_eq!(
            (calls[0].output.as_str(), calls[0].shell_command().as_deref()),
            ("a\nb", Some("ls"))
        );
        // A bare-string input survives, a call with no result keeps an empty output.
        assert_eq!(
            (&calls[1].input, calls[1].output.as_str()),
            (&serde_json::json!("*** Begin Patch"), "")
        );
        assert_eq!(calls[1].shell_command(), None);
        let argv = Call {
            id: String::new(),
            name: "shell".into(),
            input: serde_json::json!({"command": ["bash", "-lc", "cargo test"]}),
            output: String::new(),
            is_error: false,
        };
        assert_eq!(argv.shell_command().as_deref(), Some("cargo test"));
    }

    #[test]
    fn agent_names_round_trip() {
        assert_eq!(Agent::ClaudeCode.to_string(), "claude-code");
        assert_eq!("claude-code".parse::<Agent>().unwrap(), Agent::ClaudeCode);
        assert_eq!(Role::Tool.to_string(), "tool");
        assert_eq!("opencode".parse::<Agent>().unwrap().to_string(), "opencode");
    }
}
