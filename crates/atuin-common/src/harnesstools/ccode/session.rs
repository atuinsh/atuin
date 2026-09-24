use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use futures::{Stream, StreamExt};
use serde::Deserialize;
use serde::de::IgnoredAny;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::watch;
use typed_builder::TypedBuilder;

use crate::fs::tree_watcher::{FileStat, TreeWatcher};
use crate::harnesstools::ccode::Ccode;
use crate::harnesstools::session::model::{
    Content, MessageId, Role, StopReason, ToolCallId, ToolResult, ToolUse, Usage,
};
use crate::harnesstools::session::{
    Checkpoint, Listener, Message, MessageError, Observable, RuntimeError, Session, SessionId,
    Sessions, WatchError, scan_sessions,
};
use crate::io::{FollowLines, Line, PathLineReader, PooledReadLines};
use crate::json::jsonl::JsonlExt;
use crate::sync::BlockingPool;
use crate::utils::{env_nonempty, home_dir};

#[derive(Debug, Clone, TypedBuilder)]
pub struct CcodeSessions {
    #[builder(default, setter(strip_option, into))]
    root: Option<PathBuf>,
    /// Runs every file read of the sessions this finds.
    pool: BlockingPool,
}

impl CcodeSessions {
    fn resolve_root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(|| {
            env_nonempty("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home_dir().join(".claude"))
                .join("projects")
        })
    }
}

impl Sessions for CcodeSessions {
    type Listener = CcodeListener;

    fn listener(&self) -> Result<CcodeListener, RuntimeError> {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(CcodeListener {
            root,
            pool: self.pool.clone(),
        })
    }

    fn existing(
        &self,
    ) -> Result<impl Stream<Item = Result<CcodeSession, RuntimeError>> + Send + 'static, RuntimeError>
    {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        let pool = self.pool.clone();
        Ok(async_stream::stream! {
            let sessions_pool = pool.clone();
            let scan = pool
                .run(move || {
                    let projects = root.clone();
                    scan_sessions(root, |path, is_file| {
                        CcodeListener::open_session(&projects, path, is_file, &sessions_pool)
                    })
                })
                .await;
            match scan {
                Ok(items) => {
                    for item in items {
                        yield item;
                    }
                }
                Err(cancelled) => yield Err(RuntimeError::Io(std::io::Error::other(cancelled))),
            }
        })
    }
}

impl Observable for Ccode {
    type Sessions = CcodeSessions;

    fn sessions(&self, pool: BlockingPool) -> CcodeSessions {
        CcodeSessions::builder().pool(pool).build()
    }
}

#[derive(Debug, Clone)]
pub struct CcodeListener {
    root: PathBuf,
    pool: BlockingPool,
}

/// Whether `path`, under the projects directory `root`, is a transcript, the way Claude Code
/// itself tells (CC 2.1.281): a session's `<project>/<session>.jsonl` (older versions also wrote
/// subagents there, as `agent-<id>.jsonl`), or a subagent's
/// `<project>/<session>/subagents/[<subdir>/...]agent-<id>.jsonl`. Anything else it keeps
/// there is not a conversation: a workflow run's `subagents/workflows/<run>/journal.jsonl`,
/// `<session>/world.jsonl`.
fn is_transcript(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let parts: Vec<_> = relative.components().map(|c| c.as_os_str()).collect();
    let Some(stem) = parts.last().and_then(|name| name.to_str()?.strip_suffix(".jsonl")) else {
        return false;
    };
    match parts.len() {
        2 => true,
        n => n >= 4 && parts[2] == "subagents" && stem.starts_with("agent-"),
    }
}

impl CcodeListener {
    /// The session id a transcript's file name carries.
    fn session_id(path: &Path) -> Option<SessionId> {
        Some(SessionId::from(path.file_stem()?.to_string_lossy().into_owned()))
    }

    /// Build a read-once session for a transcript under `root` (no change signal), or `None`.
    fn open_session(
        root: &Path,
        path: &Path,
        is_file: bool,
        pool: &BlockingPool,
    ) -> Option<CcodeSession> {
        if !is_file || !is_transcript(root, path) {
            return None;
        }
        Some(CcodeSession::open(Self::session_id(path)?, path.to_path_buf(), pool.clone()))
    }
}

impl Listener for CcodeListener {
    type Session = CcodeSession;

    fn watch(self) -> impl Stream<Item = Result<CcodeSession, WatchError>> + Send + 'static {
        let root = self.root;
        let pool = self.pool;
        async_stream::stream! {
            // The watcher reports paths under the canonical root.
            let projects = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
            let files = TreeWatcher::builder()
                .filter(move |path| is_transcript(&projects, path))
                .watch(&root);
            let files = match files {
                Ok(files) => files,
                Err(err) => {
                    yield Err(WatchError::from(err));
                    return;
                }
            };
            for await file in files {
                let (path, stat) = file.into_parts();
                let Some(id) = Self::session_id(&path) else { continue };
                yield Ok(CcodeSession {
                    id,
                    path: path.to_path_buf(),
                    changes: Some(stat),
                    pool: pool.clone(),
                });
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct CcodeSession {
    id: SessionId,
    path: PathBuf,
    /// Wakes [`messages`](Session::messages) on each change to the file; `None` reads it once.
    changes: Option<watch::Receiver<FileStat>>,
    pool: BlockingPool,
}

impl CcodeSession {
    /// A session over the file as it stands: [`messages`](Session::messages) ends at its end.
    #[must_use]
    pub fn open(id: SessionId, path: PathBuf, pool: BlockingPool) -> Self {
        Self {
            id,
            path,
            changes: None,
            pool,
        }
    }

    /// The subagent that spawned this one, for a nested subagent's transcript. Its lines name
    /// only the root session (`sessionId`); Claude Code records the spawning agent as
    /// `parentAgentId` in the `agent-<id>.meta.json` beside the transcript, whose own transcript
    /// is `agent-<parentAgentId>.jsonl`. Read when a stream starts, since Claude Code may create
    /// the transcript before its metadata.
    async fn spawned_by(&self) -> Option<SessionId> {
        let name = self.path.file_name()?.to_str()?.strip_suffix(".jsonl")?;
        if !name.starts_with("agent-") {
            return None;
        }
        let meta = self.path.with_file_name(format!("{name}.meta.json"));
        self.pool
            .run(move || {
                let meta: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(meta).ok()?).ok()?;
                let parent = meta["parentAgentId"].as_str()?;
                Some(SessionId::from(format!("agent-{parent}")))
            })
            .await
            .ok()
            .flatten()
    }

    /// The byte offset to read from: `from`'s, if the line ending there is still the one `from`
    /// was taken after, else the transcript's start.
    async fn start(&self, from: Checkpoint) -> u64 {
        let path = self.path.clone();
        let found = self
            .pool
            .run(move || File::open(path).and_then(|file| Line::ending_at(&file, from.at)))
            .await
            .map_err(io::Error::other)
            .and_then(|found| found);
        match found {
            Ok(Some(line)) if from.names(&line.bytes) => from.at,
            Ok(_) => {
                tracing::debug!(
                    path = %self.path.display(),
                    "the checkpoint no longer names its line; reading from the start"
                );
                0
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    path = %self.path.display(),
                    "failed to read the line a checkpoint names; reading from the start"
                );
                0
            }
        }
    }
}

impl Session for CcodeSession {
    type Message = CcodeMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages_from(
        self,
        from: Option<Checkpoint>,
    ) -> impl Stream<Item = Result<(Checkpoint, CcodeMessage), MessageError>> + Send + 'static {
        async_stream::stream! {
            let spawner = self.spawned_by().await;
            let start = match from {
                Some(from) => self.start(from).await,
                None => 0,
            };
            let lines = FollowLines::new(PooledReadLines::new(
                PathLineReader::at(&self.path, start),
                self.pool,
            ));
            let messages = match self.changes {
                Some(changes) => lines.follow(changes).left_stream(),
                None => lines.read_to_end().right_stream(),
            }
            .json_with(CcodeMessage::decode);
            for await item in messages {
                yield item
                    .map(|(line, message)| {
                        (
                            Checkpoint::new(line.end, &line.bytes),
                            message.with_spawner(spawner.clone()),
                        )
                    })
                    .map_err(MessageError::from);
            }
        }
    }

    fn read(&self) -> impl Stream<Item = Result<CcodeMessage, MessageError>> + Send + 'static {
        let session = self.clone();
        async_stream::stream! {
            let spawner = session.spawned_by().await;
            let messages = FollowLines::new(PooledReadLines::new(
                PathLineReader::new(&session.path),
                session.pool,
            ))
            .read_to_end()
            .json_with(CcodeMessage::decode);
            for await item in messages {
                yield item
                    .map(|(_, message)| message.with_spawner(spawner.clone()))
                    .map_err(MessageError::from);
            }
        }
    }
}

/// One line of a Claude Code transcript (`~/.claude/projects/<project>/<session>.jsonl`).
///
/// Loosely typed fields stay [`serde_json::Value`]: a line whose shape a newer Claude Code
/// changed must still parse.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CcodeMessage {
    #[serde(rename = "type")]
    kind: String,
    subtype: Option<String>,
    uuid: Option<String>,
    timestamp: Option<String>,
    message: Option<Box<serde_json::Value>>,
    content: Option<Box<serde_json::Value>>,
    cwd: Option<PathBuf>,
    git_branch: Option<String>,
    ai_title: Option<String>,
    custom_title: Option<String>,
    agent_name: Option<String>,
    /// The title of a legacy `summary` line.
    summary: Option<Box<serde_json::Value>>,
    parent_uuid: Option<String>,
    /// The predecessor of a line written with a null `parentUuid`: a `compact_boundary`.
    logical_parent_uuid: Option<String>,
    session_id: Option<String>,
    /// `{sessionId, messageUuid}` on every line `/branch` copied. `--fork-session` (and a
    /// `--resume` Claude Code turns into a fork) copies lines without it: only `sessionId` is
    /// rewritten.
    forked_from: Option<Box<serde_json::Value>>,
    attachment: Option<Box<serde_json::Value>>,
    /// Who submitted a user line: absent or `{"kind": "human"}` for the user.
    origin: Option<Box<serde_json::Value>>,
    is_meta: Option<bool>,
    is_compact_summary: Option<bool>,
    is_api_error_message: Option<bool>,
    /// The error class of an API error line (`rate_limit`, `unknown`, ...).
    error: Option<Box<serde_json::Value>>,
    /// Present on the user lines that carry a tool's result (the tool's own record of it).
    tool_use_result: Option<IgnoredAny>,
    /// What the user typed when rejecting a tool call, on the line with its `tool_result`.
    user_feedback: Option<Box<serde_json::Value>>,
    /// The subagent that spawned a nested subagent, from its transcript's metadata file (see
    /// `CcodeSession::spawned_by`): no line names it.
    #[serde(skip)]
    spawner: Option<SessionId>,
}

/// The model Claude Code names on the assistant lines it writes itself (API errors, canned
/// replies): no model produced them.
const SYNTHETIC_MODEL: &str = "<synthetic>";

/// Elements Claude Code writes into user-role text that nobody typed: output it captured from a
/// command the user ran (`!cmd`, `/cmd`), which is execution payload, and the context it (or an
/// IDE extension) attaches to a prompt, which it strips itself before showing the prompt back
/// (`lCo` / `cCo` in CC 2.1.281; older versions put both inside the prompt's own line).
const HARNESS_TAGS: [&str; 8] = [
    "local-command-stdout",
    "local-command-stderr",
    "bash-stdout",
    "bash-stderr",
    "bash-exit-code",
    "system-reminder",
    "ide_opened_file",
    "ide_selection",
];

/// Prefixes of user-role text that Claude Code wrote itself (its own `Dre` / `W_e`
/// classification in CC 2.1.281): command output, background task notifications and teammate
/// messages, the local-command caveat, the interrupt marker.
const INJECTED_PREFIXES: [&str; 9] = [
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<bash-stdout>",
    "<bash-stderr>",
    "<local-command-caveat>",
    "<task-notification>",
    "<teammate-message",
    "<tick>",
    "[Request interrupted by user",
];

/// The sentence Claude Code puts before the model's summary on a compaction summary line: the
/// current wording (CC 2.1.281 `zG`), then the older one.
const COMPACT_PREAMBLES: [&str; 2] = [
    "The summary below covers the earlier portion of the conversation.",
    "The conversation is summarized below:",
];

/// What Claude Code appends after the model's summary on a compaction summary line (CC 2.1.281
/// `zG`): the transcript pointer, notes, and the instruction to carry on.
const COMPACT_TRAILERS: [&str; 6] = [
    "\n\nIf you need specific details from before compaction",
    "\n\nRecent messages are preserved verbatim.",
    "\n\nNote: the earliest part of the conversation was too large to include",
    "\n\nYour REPL VM state was kept across this compaction",
    "\nContinue the conversation from where it left off",
    "\nPlease continue the conversation from where we left it off",
];

fn ccode_stop_reason(raw: &str) -> StopReason {
    match raw {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "tool_use" => StopReason::ToolUse,
        "stop_sequence" => StopReason::StopSequence,
        "refusal" => StopReason::Refusal,
        other => StopReason::Other(other.to_owned()),
    }
}

/// The text between the first `<name>` and its `</name>`, or `None` without both.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = text.find(&open)? + open.len();
    let len = text[start..].find(&close)?;
    Some(&text[start..start + len])
}

/// Where the first `<name>` or `<name attr...>` opening tag in `text` starts.
fn open_tag(text: &str, name: &str) -> Option<usize> {
    let open = format!("<{name}");
    let mut from = 0;
    while let Some(at) = text[from..].find(&open) {
        let start = from + at;
        let after = start + open.len();
        if text[after..].starts_with(|c: char| c == '>' || c.is_whitespace()) {
            return Some(start);
        }
        from = after;
    }
    None
}

/// `text` without any [`HARNESS_TAGS`] element (an unclosed one runs to the end), trimmed if
/// anything was removed; `None` when there was none.
fn strip_harness_tags(text: &str) -> Option<String> {
    let mut out: Option<String> = None;
    for name in HARNESS_TAGS {
        let close = format!("</{name}>");
        while let Some(start) = open_tag(out.as_deref().unwrap_or(text), name) {
            let current = out.get_or_insert_with(|| text.to_owned());
            let end =
                current[start..].find(&close).map_or(current.len(), |i| start + i + close.len());
            current.replace_range(start..end, "");
        }
    }
    out.map(|stripped| stripped.trim().to_owned())
}

/// A user-role text as the user typed it: command output and attached context removed, and a
/// slash command (`<command-name>`), bash-mode input (`<bash-input>`) or `#` memory note
/// (`<user-memory-input>`) record rendered as the line the user typed, the way Claude Code
/// itself replays it (`ycr` / `hl` in CC 2.1.281). `None` when nothing remains.
fn typed_text(text: &str) -> Option<String> {
    let stripped = strip_harness_tags(text);
    let text = stripped.as_deref().unwrap_or(text);
    let trimmed = text.trim_start();
    if trimmed.starts_with("<command-") {
        if let Some(name) = tag(trimmed, "command-name") {
            let args = tag(trimmed, "command-args").unwrap_or_default().trim();
            return Some(if args.is_empty() {
                name.to_owned()
            } else {
                format!("{name} {args}")
            });
        }
    } else if trimmed.starts_with("<bash-input>")
        && let Some(command) = tag(trimmed, "bash-input")
    {
        return Some(format!("! {command}"));
    } else if trimmed.starts_with("<user-memory-input>")
        && let Some(note) = tag(trimmed, "user-memory-input")
    {
        return Some(format!("# {}", note.trim()));
    }
    (!trimmed.trim_end().is_empty()).then(|| text.to_owned())
}

/// The model-written part of a compaction summary line: Claude Code wraps it (CC 2.1.281 `zG`)
/// in a preamble, a pointer to the local transcript file and an instruction to carry on, and
/// turns the model's `<summary>` block into a `Summary:` heading. A text in neither known
/// shape is kept whole.
fn compact_summary(text: &str) -> &str {
    let Some(body) = COMPACT_PREAMBLES
        .iter()
        .find_map(|preamble| text.find(preamble).map(|at| &text[at + preamble.len()..]))
    else {
        return text.trim();
    };
    let end = COMPACT_TRAILERS.iter().filter_map(|trailer| body.find(trailer)).min();
    let body = body[..end.unwrap_or(body.len())].trim();
    body.strip_prefix("Summary:").map_or(body, str::trim_start)
}

/// Whether a user line's or queued command's `origin` names the user: Claude Code writes none,
/// or `{"kind": "human"}`, for a prompt the user submitted (`sE` in CC 2.1.281); anything else
/// (`task-notification`, `peer`, `auto-continuation`, ...) is the harness speaking.
fn human_origin(origin: Option<&serde_json::Value>) -> bool {
    origin.is_none_or(|o| o.is_null() || o["kind"].as_str().is_none_or(|kind| kind == "human"))
}

/// The text of every text block (or a plain string), joined.
fn joined_text(raw: &serde_json::Value) -> Option<String> {
    match raw {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(blocks) => {
            let texts: Vec<&str> = blocks.iter().filter_map(|b| b["text"].as_str()).collect();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}

impl CcodeMessage {
    /// A transcript line, read as leniently as Claude Code's own loader reads it (`Oe` in CC
    /// 2.1.281): as `JSON.parse` would (see [`crate::json::js`]; its 2.1.132 changelog notes
    /// sessions holding a lone surrogate from a tool error cut mid-emoji), and without a leading
    /// byte order mark. The error is the original line's.
    fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        crate::json::js::from_slice(bytes).or_else(|err| {
            match bytes.strip_prefix(b"\xef\xbb\xbf") {
                Some(rest) => crate::json::js::from_slice(rest).map_err(|_| err),
                None => Err(err),
            }
        })
    }

    /// The line, as read from a transcript whose subagent `spawner` spawned it.
    fn with_spawner(self, spawner: Option<SessionId>) -> Self {
        Self { spawner, ..self }
    }

    fn block(value: &serde_json::Value) -> Content {
        match value["type"].as_str() {
            Some("text") => Content::Text(value["text"].as_str().unwrap_or_default().to_owned()),
            Some("thinking" | "redacted_thinking") => Content::ReasoningSummary { tokens: None },
            // A tool the API ran itself (web search, web fetch, code execution) or an MCP
            // connector tool is a tool call like any other.
            Some("tool_use" | "server_tool_use" | "mcp_tool_use") => Content::ToolUse(ToolUse {
                id: ToolCallId::from(value["id"].as_str().unwrap_or_default().to_owned()),
                name: value["name"].as_str().unwrap_or_default().to_owned(),
                input: value["input"].clone(),
            }),
            // `tool_result`, and the result blocks of server and MCP tools
            // (`web_search_tool_result`, `mcp_tool_result`, ...), which report a failure as
            // `is_error` or as an `*_error` content object.
            Some(kind)
                if (kind == "tool_result" || kind.ends_with("_tool_result"))
                    && value["tool_use_id"].is_string() =>
            {
                Content::ToolResult(ToolResult {
                    call: ToolCallId::from(
                        value["tool_use_id"].as_str().unwrap_or_default().to_owned(),
                    ),
                    output: value["content"].clone(),
                    error: value["is_error"].as_bool().unwrap_or(false)
                        || value["content"]["type"]
                            .as_str()
                            .is_some_and(|kind| kind.ends_with("_error")),
                })
            }
            // Pasted media: keep what it was, not its (base64) bytes.
            Some(kind @ ("image" | "document")) => Content::Other(serde_json::json!({
                "type": kind,
                "source": {
                    "type": value["source"]["type"],
                    "media_type": value["source"]["media_type"],
                },
            })),
            _ => Content::Other(value.clone()),
        }
    }

    /// The line's raw content: `message.content`, or a system line's top-level `content`.
    /// Other lines' top-level `content` is bookkeeping: a `queue-operation` copies the prompt it
    /// queued, which reaches the transcript again as the user line or `queued_command` it
    /// becomes.
    fn raw_content(&self) -> Option<&serde_json::Value> {
        match &self.message {
            Some(message) => Some(&message["content"]),
            None => self.content.as_deref().filter(|_| self.kind == "system"),
        }
    }

    /// The line's role as written: `message.role`, else its `type`.
    fn raw_role(&self) -> &str {
        self.message.as_deref().and_then(|m| m["role"].as_str()).unwrap_or(self.kind.as_str())
    }

    /// The text a line opens with, peeked without cloning its blocks.
    fn first_text(&self) -> Option<&str> {
        match self.raw_content()? {
            serde_json::Value::String(text) => Some(text.as_str()),
            serde_json::Value::Array(blocks) => blocks.first().and_then(|b| b["text"].as_str()),
            _ => None,
        }
    }

    /// A synthetic assistant line Claude Code wrote for a failed API call (`isApiErrorMessage`).
    fn is_api_error(&self) -> bool {
        self.is_api_error_message == Some(true)
    }

    /// Claude Code wrote this assistant line itself (an API error, a canned reply).
    fn is_synthetic(&self) -> bool {
        self.is_api_error()
            || self.message.as_deref().is_some_and(|m| m["model"].as_str() == Some(SYNTHETIC_MODEL))
    }

    /// A `queued_command` attachment: a prompt delivered while the agent was busy (the user's,
    /// or a background task's notification).
    fn queued_command(&self) -> Option<&serde_json::Value> {
        self.attachment.as_deref().filter(|a| a["type"].as_str() == Some("queued_command"))
    }

    /// Whether a queued command is one the user typed. Claude Code shows it as the user's turn
    /// when `commandMode` is `prompt`, it is not `isMeta` and its origin is human (`d8` / `Iie`
    /// in CC 2.1.281); a task notification has `commandMode: "task-notification"`.
    fn queued_by_user(queued: &serde_json::Value) -> bool {
        queued["isMeta"].as_bool() != Some(true)
            && queued["commandMode"].as_str().is_none_or(|mode| mode == "prompt")
            && human_origin(queued.get("origin"))
    }

    /// A user-role line Claude Code wrote itself rather than the user typing it.
    fn injected_user_line(&self) -> bool {
        self.is_meta == Some(true)
            || !human_origin(self.origin.as_deref())
            || self.first_text().is_some_and(|text| {
                let text = text.trim_start();
                INJECTED_PREFIXES.iter().any(|prefix| text.starts_with(prefix))
            })
    }

    /// A user line carrying a tool's result rather than a prompt: Claude Code sends tool results
    /// in the user's turn, and writes each such line with the tool's `toolUseResult`.
    fn tool_result_line(&self) -> bool {
        self.tool_use_result.is_some()
            || matches!(self.raw_content(), Some(serde_json::Value::Array(blocks))
                if !blocks.is_empty()
                    && blocks.iter().all(|b| b["type"].as_str() == Some("tool_result")))
    }

    /// What the user typed when rejecting a tool call (`userFeedback`, which Claude Code sets
    /// only for text that came from the user, `feedbackIsFromUser` in CC 2.1.281). The tool
    /// result embeds it too, but it is the user's own words.
    fn user_feedback(&self) -> Option<&str> {
        self.user_feedback.as_deref()?.as_str().filter(|text| !text.trim().is_empty())
    }

    /// A `local_command` system line recording the slash command the user typed (as opposed
    /// to its output, written as another `local_command` line).
    fn typed_local_command(&self) -> bool {
        self.kind == "system"
            && self.subtype.as_deref() == Some("local_command")
            && self.first_text().is_some_and(|text| text.trim_start().starts_with("<command-"))
    }

    /// Thinking tokens the call reported (`usage.output_tokens_details.thinking_tokens`).
    fn thinking_tokens(&self) -> Option<u64> {
        self.message.as_deref()?["usage"]["output_tokens_details"]["thinking_tokens"].as_u64()
    }
}

impl Message for CcodeMessage {
    fn id(&self) -> Option<MessageId> {
        self.uuid.clone().map(MessageId::from)
    }

    fn role(&self) -> Role {
        if self.kind == "attachment" {
            return match self.queued_command() {
                Some(queued) if Self::queued_by_user(queued) => Role::User,
                Some(_) => Role::System,
                None => Role::Other(self.kind.clone()),
            };
        }
        if self.is_compact_summary == Some(true) {
            return Role::System;
        }
        if self.typed_local_command() {
            return Role::User;
        }
        match self.raw_role() {
            "user" if self.user_feedback().is_some() => Role::User,
            "user" if self.injected_user_line() => Role::System,
            "user" if self.tool_result_line() => Role::Tool,
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "system" => Role::System,
            "tool" => Role::Tool,
            other => Role::Other(other.to_owned()),
        }
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.timestamp.as_deref().and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
    }

    fn content(&self) -> Vec<Content> {
        if let Some(queued) = self.queued_command() {
            return match &queued["prompt"] {
                serde_json::Value::String(text) => {
                    typed_text(text).map(Content::Text).into_iter().collect()
                }
                serde_json::Value::Array(blocks) => blocks
                    .iter()
                    .filter_map(|block| match block["type"].as_str() {
                        Some("text") => typed_text(block["text"].as_str().unwrap_or_default())
                            .map(Content::Text),
                        _ => Some(Self::block(block)),
                    })
                    .collect(),
                _ => Vec::new(),
            };
        }
        let Some(raw) = self.raw_content() else {
            return Vec::new();
        };
        if self.is_compact_summary == Some(true) {
            return joined_text(raw)
                .map(|text| Content::Summary(compact_summary(&text).to_owned()))
                .into_iter()
                .collect();
        }
        if self.is_api_error() {
            let text =
                joined_text(raw).or_else(|| self.error.as_ref()?.as_str().map(str::to_owned));
            return text.map(Content::Error).into_iter().collect();
        }
        // Everything but model output can hold a record of a command the user ran.
        let typed = self.raw_role() != "assistant";
        let text = |text: &str| {
            if typed {
                typed_text(text).map(Content::Text)
            } else {
                Some(Content::Text(text.to_owned()))
            }
        };
        let mut content: Vec<_> = match raw {
            serde_json::Value::String(s) => text(s).into_iter().collect(),
            serde_json::Value::Array(blocks) => blocks
                .iter()
                .filter_map(|block| match block["type"].as_str() {
                    Some("text") => text(block["text"].as_str().unwrap_or_default()),
                    _ => Some(Self::block(block)),
                })
                .collect(),
            _ => Vec::new(),
        };
        // One reasoning marker per thinking block, so a call split over several lines is
        // marked on the line that did the thinking only. The call's thinking tokens (repeated
        // on every line; the capture engine counts them once per call) ride on that marker.
        if let Some(Content::ReasoningSummary { tokens }) =
            content.iter_mut().find(|block| matches!(block, Content::ReasoningSummary { .. }))
        {
            *tokens = self.thinking_tokens();
        }
        if let Some(feedback) = self.user_feedback() {
            content.push(Content::Text(feedback.to_owned()));
        }
        content
    }

    fn model(&self) -> Option<String> {
        if self.is_synthetic() {
            return None;
        }
        self.message.as_deref()?.get("model")?.as_str().map(str::to_owned)
    }

    fn usage(&self) -> Option<Usage> {
        // A line Claude Code wrote itself made no API call; its zeroed usage is not one.
        if self.is_synthetic() {
            return None;
        }
        let usage = self.message.as_deref()?.get("usage")?;
        if usage.is_null() {
            return None;
        }
        let field = |name: &str| usage.get(name).and_then(serde_json::Value::as_u64);
        // The split by cache lifetime, for a writer that leaves out the total.
        let cache_write = field("cache_creation_input_tokens").or_else(|| {
            let split = usage.get("cache_creation")?.as_object()?;
            split.values().filter_map(serde_json::Value::as_u64).reduce(|a, b| a + b)
        });
        Some(Usage {
            input: field("input_tokens"),
            output: field("output_tokens"),
            cache_read: field("cache_read_input_tokens"),
            cache_write,
            reasoning: self.thinking_tokens(),
        })
    }

    fn stop_reason(&self) -> Option<StopReason> {
        if self.is_api_error() {
            return Some(StopReason::Error);
        }
        // An interrupt is recorded as a user line; it is the turn that it ends.
        if self.raw_role() == "user"
            && self
                .first_text()
                .is_some_and(|t| t.trim_start().starts_with("[Request interrupted by user"))
        {
            return Some(StopReason::Aborted);
        }
        Some(ccode_stop_reason(self.message.as_deref()?.get("stop_reason")?.as_str()?))
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.cwd.clone()
    }

    fn git_branch(&self) -> Option<String> {
        self.git_branch.clone()
    }

    fn parent_id(&self) -> Option<MessageId> {
        self.parent_uuid.clone().or_else(|| self.logical_parent_uuid.clone()).map(MessageId::from)
    }

    /// The session a fork was copied from (`forkedFrom`), else the subagent that spawned a nested
    /// subagent, else the session the line names: a subagent's lines name the root session.
    fn parent_session(&self) -> Option<SessionId> {
        self.forked_from
            .as_ref()
            .and_then(|f| f["sessionId"].as_str())
            .map(|id| SessionId::from(id.to_owned()))
            .or_else(|| self.spawner.clone())
            .or_else(|| self.session_id.clone().map(SessionId::from))
    }

    /// The API message id: one per model call, shared by every line the response is split
    /// into, and kept when Claude Code copies the line into a fork or a `/btw` replay.
    fn turn_id(&self) -> Option<String> {
        if self.is_synthetic() {
            return None;
        }
        self.message.as_deref()?.get("id")?.as_str().map(str::to_owned)
    }

    /// The title a metadata line sets: `custom-title` (`/rename`), `agent-name` (the name
    /// `/rename` and background agents give the session, which Claude Code shows first), the
    /// generated `ai-title`, or a legacy `summary`. Claude Code re-appends its metadata lines
    /// (`reAppendSessionMetadata` in CC 2.1.281) in the order custom title, generated title,
    /// agent name.
    fn title(&self) -> Option<String> {
        let summary = || {
            (self.kind == "summary")
                .then(|| self.summary.as_ref()?.as_str().map(str::to_owned))
                .flatten()
        };
        let agent_name = || (self.kind == "agent-name").then(|| self.agent_name.clone()).flatten();
        self.custom_title
            .clone()
            .or_else(agent_name)
            .or_else(|| self.ai_title.clone())
            .or_else(summary)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use futures::{StreamExt, TryStreamExt};
    use rstest::{fixture, rstest};

    use super::*;
    use crate::sync::BlockingPool;

    fn pool() -> BlockingPool {
        BlockingPool::new(std::num::NonZeroUsize::MIN)
    }

    /// A projects directory (removed on drop) holding one project directory.
    struct Projects {
        root: tempfile::TempDir,
        project: PathBuf,
    }

    #[fixture]
    fn projects() -> Projects {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("-work-proj");
        std::fs::create_dir(&project).unwrap();
        Projects { root, project }
    }

    /// Only transcripts are sessions: not the other `jsonl` files Claude Code keeps beside them.
    #[rstest]
    #[case::session("-work-proj/0b3c.jsonl", true)]
    #[case::legacy_subagent("-work-proj/agent-a1.jsonl", true)]
    #[case::subagent("-work-proj/0b3c/subagents/agent-a1.jsonl", true)]
    #[case::workflow_agent("-work-proj/0b3c/subagents/workflows/run1/agent-a2.jsonl", true)]
    #[case::workflow_journal("-work-proj/0b3c/subagents/workflows/run1/journal.jsonl", false)]
    #[case::session_sidecar("-work-proj/0b3c/world.jsonl", false)]
    #[case::at_the_root("0b3c.jsonl", false)]
    #[case::not_jsonl("-work-proj/0b3c/tool-results/b1.txt", false)]
    fn transcripts_are_the_sessions(#[case] relative: &str, #[case] expected: bool) {
        let root = Path::new("/home/u/.claude/projects");
        assert_eq!(is_transcript(root, &root.join(relative)), expected);
    }

    /// A subagent's lines name the root session; a nested one's parent is the subagent that
    /// spawned it, named only in its metadata file (real layout, CC 2.1.281).
    #[rstest]
    #[case::nested(
        Some(r#"{"agentType":"general-purpose","parentAgentId":"a1","spawnDepth":2}"#),
        "agent-a1"
    )]
    #[case::top_level(Some(r#"{"agentType":"general-purpose","spawnDepth":1}"#), "s1")]
    #[case::no_metadata(None, "s1")]
    #[tokio::test]
    async fn subagent_parent_is_the_session_that_spawned_it(
        projects: Projects,
        #[case] meta: Option<&str>,
        #[case] expected: &str,
    ) {
        let dir = projects.project.join("s1/subagents");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent-a2.jsonl");
        let raw = serde_json::json!({"type": "user", "uuid": "u1", "sessionId": "s1",
            "agentId": "a2", "isSidechain": true, "message": {"role": "user", "content": "go"}});
        std::fs::write(&path, raw.to_string() + "\n").unwrap();
        if let Some(meta) = meta {
            std::fs::write(dir.join("agent-a2.meta.json"), meta).unwrap();
        }
        let session = CcodeSession::open(SessionId::from("agent-a2".to_owned()), path, pool());
        let expected = Some(SessionId::from(expected.to_owned()));
        let read: Vec<CcodeMessage> = session.read().try_collect().await.unwrap();
        assert_eq!(read[0].parent_session(), expected);
        let mut followed = std::pin::pin!(session.messages_from(None));
        let (_, followed) = followed.next().await.unwrap().unwrap();
        assert_eq!(followed.parent_session(), expected);
    }

    #[rstest]
    #[tokio::test]
    async fn existing_skips_files_that_are_not_transcripts(projects: Projects) {
        let workflow = projects.project.join("0b3c/subagents/workflows/run1");
        std::fs::create_dir_all(&workflow).unwrap();
        for path in [projects.project.join("0b3c.jsonl"), workflow.join("journal.jsonl")] {
            std::fs::write(path, line("user", "user", serde_json::json!("hi")) + "\n").unwrap();
        }
        let sessions =
            CcodeSessions::builder().root(projects.root.path().to_path_buf()).pool(pool()).build();
        let ids: Vec<SessionId> =
            sessions.existing().unwrap().map_ok(|s| s.id()).try_collect().await.unwrap();
        assert_eq!(ids, [SessionId::from("0b3c".to_owned())]);
    }
    use crate::harnesstools::session::model::{Content, Role};
    use crate::harnesstools::session::{Message, Session, SessionEvent, Sessions};

    #[allow(clippy::needless_pass_by_value)]
    fn line(kind: &str, role: &str, content: serde_json::Value) -> String {
        serde_json::json!({
            "type": kind,
            "sessionId": "11111111-1111-1111-1111-111111111111",
            "uuid": "aaaa",
            "timestamp": "2026-09-18T10:00:00Z",
            "message": {"role": role, "content": content},
        })
        .to_string()
    }

    #[rstest]
    fn normalizes_a_user_string_message() {
        let raw = line("user", "user", serde_json::json!("hi there"));
        let m: CcodeMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.role(), Role::User);
        assert_eq!(m.content(), vec![Content::Text("hi there".into())]);
    }

    #[rstest]
    fn normalizes_assistant_tool_use_blocks() {
        let raw = line(
            "assistant",
            "assistant",
            serde_json::json!([
                {"type": "text", "text": "running"},
                {"type": "tool_use", "id": "call_1", "name": "Bash", "input": {"cmd": "ls"}},
            ]),
        );
        let m: CcodeMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.role(), Role::Assistant);
        let content = m.content();
        assert_eq!(content.len(), 2);
        assert!(matches!(content[1], Content::ToolUse(_)));
    }

    #[rstest]
    fn normalizes_assistant_enrichment_fields() {
        let raw = serde_json::json!({
            "type": "assistant",
            "uuid": "aaaa",
            "cwd": "/work/atuin",
            "gitBranch": "main",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "done"}],
                "model": "claude-opus-4-8",
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 20,
                    "cache_read_input_tokens": 5,
                    "cache_creation_input_tokens": 2,
                },
            },
        })
        .to_string();
        let m: CcodeMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.model(), Some("claude-opus-4-8".to_owned()));
        assert_eq!(m.stop_reason(), Some(StopReason::EndTurn));
        assert_eq!(m.cwd(), Some(PathBuf::from("/work/atuin")));
        assert_eq!(m.git_branch(), Some("main".to_owned()));
        assert_eq!(
            m.usage(),
            Some(Usage {
                input: Some(10),
                output: Some(20),
                cache_read: Some(5),
                cache_write: Some(2),
                reasoning: None,
            })
        );
    }

    #[rstest]
    #[case("max_tokens", StopReason::MaxTokens)]
    #[case("tool_use", StopReason::ToolUse)]
    #[case("stop_sequence", StopReason::StopSequence)]
    #[case("refusal", StopReason::Refusal)]
    #[case("weird", StopReason::Other("weird".to_owned()))]
    fn maps_stop_reason_vocabulary(#[case] raw: &str, #[case] expected: StopReason) {
        let m: CcodeMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "assistant",
                "message": {"role": "assistant", "content": [], "stop_reason": raw},
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(m.stop_reason(), Some(expected));
    }

    #[rstest]
    fn enrichment_is_none_when_the_harness_did_not_provide_it() {
        let raw = line("user", "user", serde_json::json!("hi there"));
        let m: CcodeMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.model(), None);
        assert_eq!(m.usage(), None);
        assert_eq!(m.stop_reason(), None);
        assert_eq!(m.cwd(), None);
        assert_eq!(m.git_branch(), None);
    }

    #[rstest]
    fn exposes_parent_line_parent_session_and_turn() {
        let m: CcodeMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "assistant",
                "uuid": "bbbb",
                "parentUuid": "aaaa",
                "sessionId": "p",
                "message": {"role": "assistant", "id": "msg_01", "content": []},
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(m.parent_id(), Some(MessageId::from("aaaa".to_owned())));
        assert_eq!(m.parent_session(), Some(SessionId::from("p".to_owned())));
        assert_eq!(m.turn_id().as_deref(), Some("msg_01"));
    }

    #[rstest]
    #[case(serde_json::json!({"type": "ai-title", "aiTitle": "generated"}), "generated")]
    #[case(serde_json::json!({"type": "custom-title", "customTitle": "by hand"}), "by hand")]
    #[case(serde_json::json!({"type": "agent-name", "agentName": "code-review", "sessionId": "s"}), "code-review")]
    fn title_lines_expose_the_title(#[case] raw: serde_json::Value, #[case] expected: &str) {
        let m: CcodeMessage = serde_json::from_str(&raw.to_string()).unwrap();
        assert_eq!(m.title().as_deref(), Some(expected));
        assert!(m.content().is_empty());
    }

    #[rstest]
    #[case(
        serde_json::json!({"type": "user", "isCompactSummary": true,
            "message": {"role": "user", "content": "summary"}}),
    )]
    #[case(
        serde_json::json!({"type": "system", "subtype": "compact_boundary",
            "compactMetadata": {"trigger": "auto"}, "content": "boundary"}),
    )]
    fn compaction_lines_are_system(#[case] raw: serde_json::Value) {
        let m: CcodeMessage = serde_json::from_str(&raw.to_string()).unwrap();
        assert_eq!(m.role(), Role::System);
    }

    #[rstest]
    #[case("[Request interrupted by user]", Some(StopReason::Aborted))]
    #[case("[Request interrupted by user for tool use]", Some(StopReason::Aborted))]
    #[case("please continue", None)]
    fn interrupt_lines_end_the_turn(#[case] text: &str, #[case] expected: Option<StopReason>) {
        let raw = line("user", "user", serde_json::json!([{"type": "text", "text": text}]));
        let m: CcodeMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.stop_reason(), expected);
    }

    #[rstest]
    fn listener_reports_not_found_for_a_missing_root() {
        let sessions =
            CcodeSessions::builder().root(PathBuf::from("/no/such/claude")).pool(pool()).build();
        assert!(matches!(sessions.listener(), Err(RuntimeError::NotFound(_))));
    }

    #[rstest]
    fn listener_opens_an_existing_root() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = CcodeSessions::builder().root(dir.path().to_path_buf()).pool(pool()).build();
        assert!(sessions.listener().is_ok());
    }

    #[rstest]
    #[tokio::test]
    async fn messages_streams_each_turn_of_a_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        // Trailing newline required: messages() withholds an unterminated final line until a
        // later write completes it (a real session ends every record with a newline).
        let body = [
            line("user", "user", serde_json::json!("first")),
            line("assistant", "assistant", serde_json::json!([{"type": "text", "text": "second"}])),
        ]
        .join("\n")
            + "\n";
        std::fs::write(&path, body).unwrap();

        let session = CcodeSession::open(
            SessionId::from("11111111-1111-1111-1111-111111111111".to_owned()),
            path,
            pool(),
        );
        let got: Vec<Role> =
            session.messages().take(2).map_ok(|m| m.role()).try_collect().await.unwrap();
        assert_eq!(got, vec![Role::User, Role::Assistant]);
    }

    #[rstest]
    #[tokio::test]
    async fn watch_emits_sessions_as_files_appear() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("project-a");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("22222222-2222-2222-2222-222222222222.jsonl"),
            line("user", "user", serde_json::json!("hi")),
        )
        .unwrap();

        let listener = CcodeSessions::builder()
            .root(dir.path().to_path_buf())
            .pool(pool())
            .build()
            .listener()
            .unwrap();
        let seen: Vec<SessionId> = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            listener.watch().take(1).map_ok(|s| s.id()).try_collect(),
        )
        .await
        .expect("watch() did not emit a session within 10s")
        .unwrap();
        assert_eq!(seen.len(), 1);
    }

    #[rstest]
    #[tokio::test]
    async fn events_yields_messages_tagged_with_their_session() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("project-a");
        std::fs::create_dir_all(&sub).unwrap();
        // Trailing newline required: each session's messages() withholds an unterminated final
        // line until a later write completes it (a real session ends every record with a newline).
        let body = [
            line("user", "user", serde_json::json!("hi")),
            line("assistant", "assistant", serde_json::json!([{"type": "text", "text": "yo"}])),
        ]
        .join("\n")
            + "\n";
        std::fs::write(sub.join("33333333-3333-3333-3333-333333333333.jsonl"), &body).unwrap();

        let listener = CcodeSessions::builder()
            .root(dir.path().to_path_buf())
            .pool(pool())
            .build()
            .listener()
            .unwrap();
        let events: Vec<SessionEvent<CcodeMessage>> = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            listener.events(|_| std::future::ready(None)).take(2).try_collect(),
        )
        .await
        .expect("events() did not produce within 10s")
        .unwrap();

        let sid = SessionId::from("33333333-3333-3333-3333-333333333333".to_owned());
        assert!(events.iter().all(|event| event.session == sid));
        let roles: Vec<Role> = events.iter().map(|event| event.message.role()).collect();
        assert_eq!(roles, vec![Role::User, Role::Assistant]);
        // Each event carries the checkpoint past its line; the last one is at the file's length.
        assert!(events[0].checkpoint.at < events[1].checkpoint.at);
        assert_eq!(events[1].checkpoint.at, u64::try_from(body.len()).unwrap());
    }

    /// The checkpoint a caller resumes from is honoured: only lines past it are yielded.
    #[rstest]
    #[tokio::test]
    async fn events_resume_each_session_from_its_checkpoint(projects: Projects) {
        let first = line("user", "user", serde_json::json!("hi")) + "\n";
        let body = first.clone()
            + &line("assistant", "assistant", serde_json::json!([{"type": "text", "text": "yo"}]))
            + "\n";
        std::fs::write(projects.project.join("66666666-6666-6666-6666-666666666666.jsonl"), &body)
            .unwrap();

        let listener = CcodeSessions::builder()
            .root(projects.root.path().to_path_buf())
            .pool(pool())
            .build()
            .listener()
            .unwrap();
        let from =
            Checkpoint::new(u64::try_from(first.len()).unwrap(), first.trim_end().as_bytes());
        let events: Vec<SessionEvent<CcodeMessage>> = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            listener.events(move |_| std::future::ready(Some(from))).take(1).try_collect(),
        )
        .await
        .expect("events() did not produce within 10s")
        .unwrap();
        assert_eq!(events[0].message.role(), Role::Assistant);
        assert_eq!(events[0].checkpoint.at, u64::try_from(body.len()).unwrap());
    }

    #[rstest]
    #[tokio::test]
    async fn messages_yields_lines_appended_after_discovery(projects: Projects) {
        let path = projects.project.join("44444444-4444-4444-4444-444444444444.jsonl");
        std::fs::write(&path, line("user", "user", serde_json::json!("hi")) + "\n").unwrap();

        let listener = CcodeSessions::builder()
            .root(projects.root.path().to_path_buf())
            .pool(pool())
            .build()
            .listener()
            .unwrap();
        // The watch stream owns the watcher: it must outlive the message stream.
        let mut sessions = std::pin::pin!(listener.watch());
        let session = sessions.next().await.unwrap().unwrap();
        let mut messages = std::pin::pin!(session.messages());
        assert_eq!(messages.next().await.unwrap().unwrap().role(), Role::User);

        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(
            &mut file,
            (line("assistant", "assistant", serde_json::json!([{"type": "text", "text": "yo"}]))
                + "\n")
                .as_bytes(),
        )
        .unwrap();
        drop(file);
        assert_eq!(messages.next().await.unwrap().unwrap().role(), Role::Assistant);
    }

    #[rstest]
    #[tokio::test]
    async fn messages_ends_when_the_session_file_is_removed(projects: Projects) {
        let path = projects.project.join("55555555-5555-5555-5555-555555555555.jsonl");
        std::fs::write(&path, line("user", "user", serde_json::json!("hi")) + "\n").unwrap();

        let listener = CcodeSessions::builder()
            .root(projects.root.path().to_path_buf())
            .pool(pool())
            .build()
            .listener()
            .unwrap();
        let mut sessions = std::pin::pin!(listener.watch());
        let session = sessions.next().await.unwrap().unwrap();
        let mut messages = std::pin::pin!(session.messages());
        assert!(messages.next().await.unwrap().is_ok());

        std::fs::remove_file(&path).unwrap();
        // A change signalled for the vanished path may surface as an I/O error first; the
        // stream must still end once the watcher drops the file's handler. Removal is detected
        // by a filesystem event or, if that is missed, by the periodic full scan (the content
        // poll cannot see a vanished file).
        loop {
            match messages.next().await {
                None => break,
                Some(Err(_)) => {}
                Some(Ok(m)) => panic!("unexpected message after removal: {m:?}"),
            }
        }
    }

    #[rstest]
    #[case(include_str!("../../../tests/fixtures/ccode/session1.jsonl"))]
    #[case(include_str!("../../../tests/fixtures/ccode/session2.jsonl"))]
    #[case(include_str!("../../../tests/fixtures/ccode/session3.jsonl"))]
    #[case(include_str!("../../../tests/fixtures/ccode/session4.jsonl"))]
    fn normalizes_a_real_redacted_session(#[case] jsonl: &str) {
        let msgs: Vec<CcodeMessage> = jsonl
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<CcodeMessage>(l).expect("fixture record parses"))
            .collect();
        assert!(msgs.len() >= 20);

        let mut saw_assistant = false;
        let mut tool_uses = 0usize;
        let mut tool_results = 0usize;
        for m in &msgs {
            let _ = m.timestamp();
            saw_assistant |= m.role() == Role::Assistant;
            for c in m.content() {
                match c {
                    Content::ToolUse(u) => {
                        assert!(!u.name.is_empty(), "tool_use normalized to an empty name");
                        assert!(!u.id.to_string().is_empty(), "tool_use normalized to an empty id");
                        tool_uses += 1;
                    }
                    Content::ToolResult(r) => {
                        assert!(!r.call.to_string().is_empty(), "tool_result lost its call id");
                        assert_eq!(m.role(), Role::Tool, "a tool result is not the user's turn");
                        tool_results += 1;
                    }
                    _ => {}
                }
            }
        }
        assert!(saw_assistant, "expected assistant turns");
        assert!(tool_uses >= 1, "expected at least one normalized tool_use");
        assert!(tool_results >= 1, "expected at least one normalized tool_result");
    }

    fn parse(raw: &serde_json::Value) -> CcodeMessage {
        serde_json::from_str(&raw.to_string()).unwrap()
    }

    /// Legacy `summary` lines carry the session title Claude Code's `/resume` shows
    /// (`r.set(kn.leafUuid, kn.summary)` in CC 2.1.281).
    #[rstest]
    fn summary_line_exposes_its_title() {
        let m = parse(&serde_json::json!({
            "type": "summary", "summary": "Fix the flaky sync test", "leafUuid": "u9",
        }));
        assert_eq!(m.title().as_deref(), Some("Fix the flaky sync test"));
        assert!(m.content().is_empty());
    }

    /// A prompt the user types while the agent is busy is written only as a `queued_command`
    /// attachment (`attachment.prompt`); Claude Code treats it as the user's turn.
    #[rstest]
    #[case::origin_human(serde_json::json!({"origin": {"kind": "human"}}))]
    #[case::prompt_mode(serde_json::json!({"commandMode": "prompt"}))]
    fn queued_prompt_is_user_text(#[case] extra: serde_json::Value) {
        let mut attachment =
            serde_json::json!({"type": "queued_command", "prompt": "also check forks please"});
        attachment.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let m = parse(&serde_json::json!({
            "type": "attachment", "uuid": "a1", "parentUuid": "u0", "sessionId": "s",
            "timestamp": "2026-09-23T22:44:00Z", "attachment": attachment,
        }));
        assert_eq!(m.role(), Role::User);
        assert_eq!(m.content(), vec![Content::Text("also check forks please".into())]);
    }

    /// Background task results are delivered through the same queue; they are not the user's.
    #[rstest]
    #[case::task_notification(serde_json::json!({"commandMode": "task-notification"}))]
    #[case::peer(serde_json::json!({"origin": {"kind": "peer", "from": "a1"}}))]
    #[case::meta(serde_json::json!({"isMeta": true}))]
    fn queued_harness_message_is_system(#[case] extra: serde_json::Value) {
        let mut attachment = serde_json::json!({"type": "queued_command",
            "prompt": "<task-notification>\n<task-id>b1</task-id>\n</task-notification>"});
        attachment.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let m = parse(&serde_json::json!({"type": "attachment", "uuid": "a1",
            "attachment": attachment}));
        assert_eq!(m.role(), Role::System);
    }

    /// `queue-operation` lines are Claude Code's queue bookkeeping. `remove` records a command
    /// leaving the queue, whether delivered or discarded (`cr(..., commandsDiscarded)` in CC
    /// 2.1.281), and is written for task notifications too. claude-code-log renders it as
    /// steering only for Claude Code before ~2.1.101; later versions write a `queued_command`
    /// attachment for each delivery, which is where the user's text is taken from. Reading
    /// `remove` as user text would capture it twice.
    #[rstest]
    #[case("remove")]
    #[case("enqueue")]
    #[case("popAll")]
    fn queue_operation_is_not_user_text(#[case] operation: &str) {
        let m = parse(&serde_json::json!({
            "type": "queue-operation", "operation": operation, "sessionId": "s",
            "timestamp": "2026-09-23T22:44:00Z", "content": "stop and use rstest",
        }));
        assert_ne!(m.role(), Role::User);
    }

    /// `compact_boundary` lines are written with `parentUuid: null` and the real predecessor in
    /// `logicalParentUuid`.
    #[rstest]
    fn compact_boundary_keeps_its_logical_parent() {
        let m = parse(&serde_json::json!({
            "type": "system", "subtype": "compact_boundary", "uuid": "b1",
            "parentUuid": null, "logicalParentUuid": "u41",
            "content": "Conversation compacted", "compactMetadata": {"trigger": "auto"},
        }));
        assert_eq!(m.parent_id(), Some(MessageId::from("u41".to_owned())));
    }

    /// The compaction summary is the only record of the conversation it replaced.
    #[rstest]
    #[case(serde_json::json!("Summary: fixed forks"))]
    #[case(serde_json::json!([{"type": "text", "text": "Summary: fixed forks"}]))]
    fn compact_summary_is_a_summary(#[case] content: serde_json::Value) {
        let m = parse(&serde_json::json!({
            "type": "user", "uuid": "c1", "isCompactSummary": true,
            "message": {"role": "user", "content": content},
        }));
        assert_eq!(m.content(), vec![Content::Summary("Summary: fixed forks".into())]);
    }

    /// `/branch` (`--fork-session`) copies each line with `sessionId` rewritten to the new session
    /// and the origin in `forkedFrom.sessionId`.
    #[rstest]
    fn forked_line_names_the_session_it_was_forked_from() {
        let m = parse(&serde_json::json!({
            "type": "user", "uuid": "u1", "parentUuid": null, "sessionId": "new",
            "forkedFrom": {"sessionId": "old", "messageUuid": "u1"},
            "message": {"role": "user", "content": "hi"},
        }));
        assert_eq!(m.parent_session(), Some(SessionId::from("old".to_owned())));
    }

    /// Synthetic API-error assistant lines (`isApiErrorMessage`, model `<synthetic>`,
    /// `stop_reason: "stop_sequence"`, zero usage) are a failed call, not a model's reply.
    #[rstest]
    fn api_error_line_is_an_error_not_a_synthetic_model() {
        let m = parse(&serde_json::json!({
            "type": "assistant", "uuid": "e1", "isApiErrorMessage": true,
            "error": "rate_limit",
            "message": {"id": "0b5c", "role": "assistant", "model": "<synthetic>",
                "stop_reason": "stop_sequence", "stop_sequence": "",
                "content": [{"type": "text", "text": "API Error: 529 Overloaded"}],
                "usage": {"input_tokens": 0, "output_tokens": 0,
                    "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}},
        }));
        assert_eq!(m.stop_reason(), Some(StopReason::Error));
        assert_eq!(m.model(), None, "`<synthetic>` is not a model");
        assert_eq!(m.usage(), None);
        assert_eq!(m.turn_id(), None);
        assert_eq!(m.content(), vec![Content::Error("API Error: 529 Overloaded".into())]);
    }

    /// Lines Claude Code writes into the user's side of the conversation are not user text.
    #[rstest]
    #[case::meta(serde_json::json!({"isMeta": true,
        "message": {"role": "user", "content": "<local-command-caveat>Caveat</local-command-caveat>"}}))]
    #[case::task_notification(serde_json::json!({"origin": {"kind": "task-notification"},
        "message": {"role": "user", "content": "<task-notification>done</task-notification>"}}))]
    #[case::peer(serde_json::json!({"origin": {"kind": "peer"},
        "message": {"role": "user", "content": "<agent-message>report</agent-message>"}}))]
    #[case::command_output(serde_json::json!({
        "message": {"role": "user", "content": "<local-command-stdout>ok</local-command-stdout>"}}))]
    #[case::interrupt(serde_json::json!({
        "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user]"}]}}))]
    fn injected_user_line_is_system(#[case] extra: serde_json::Value) {
        let mut raw = serde_json::json!({"type": "user", "uuid": "m1"});
        raw.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        assert_eq!(parse(&raw).role(), Role::System);
    }

    /// Command output never leaves the parser; the command the user typed does, as they typed it.
    #[rstest]
    #[case::stdout("<local-command-stdout>PRIVATE</local-command-stdout>", vec![])]
    #[case::stderr("<local-command-stderr>PRIVATE</local-command-stderr>", vec![])]
    #[case::bash_output(
        "<bash-stdout>PRIVATE</bash-stdout><bash-stderr></bash-stderr><bash-exit-code>0</bash-exit-code>",
        vec![]
    )]
    #[case::slash_command(
        "<command-name>/model</command-name>\n  <command-message>model</command-message>\n  <command-args>opus</command-args>",
        vec![Content::Text("/model opus".into())]
    )]
    #[case::slash_command_without_args(
        "<command-message>login</command-message>\n<command-name>/login</command-name>\n<command-args></command-args>",
        vec![Content::Text("/login".into())]
    )]
    #[case::bash_input("<bash-input>ls -la</bash-input>", vec![Content::Text("! ls -la".into())])]
    #[case::inline_output(
        "see <local-command-stdout>PRIVATE</local-command-stdout>this",
        vec![Content::Text("see this".into())]
    )]
    #[case::unclosed_output("hi <bash-stdout>PRIVATE", vec![Content::Text("hi".into())])]
    // Context Claude Code or an IDE extension attached to the prompt (older versions wrote it
    // into the prompt's own line); Claude Code strips it before showing the prompt back.
    #[case::system_reminder(
        "fix it\n<system-reminder>\nPRIVATE context\n</system-reminder>",
        vec![Content::Text("fix it".into())]
    )]
    #[case::ide_opened_file(
        "<ide_opened_file>The user opened the file /PRIVATE.rs in the IDE.</ide_opened_file>\nwhat does this do?",
        vec![Content::Text("what does this do?".into())]
    )]
    #[case::ide_selection(
        "<ide_selection lines=\"1-2\">PRIVATE</ide_selection>explain",
        vec![Content::Text("explain".into())]
    )]
    #[case::reminder_only("<system-reminder>PRIVATE</system-reminder>", vec![])]
    #[case::not_a_known_tag("<ide_other>kept</ide_other>", vec![Content::Text("<ide_other>kept</ide_other>".into())])]
    // `#` memory mode, which Claude Code shows back as `# note` (`hl` in CC 2.1.281).
    #[case::memory_note(
        "<user-memory-input>prefer rstest</user-memory-input>",
        vec![Content::Text("# prefer rstest".into())]
    )]
    fn user_text_is_what_the_user_typed(#[case] text: &str, #[case] expected: Vec<Content>) {
        let m = parse(&serde_json::json!({"type": "user", "uuid": "c1",
            "message": {"role": "user", "content": text}}));
        assert_eq!(m.content(), expected);
    }

    /// Claude Code also records a typed slash command as a `local_command` system line.
    #[rstest]
    fn local_command_record_is_user_text() {
        let m = parse(&serde_json::json!({"type": "system", "subtype": "local_command",
            "uuid": "l1", "content": "<command-name>/cost</command-name><command-args></command-args>"}));
        assert_eq!(m.role(), Role::User);
        assert_eq!(m.content(), vec![Content::Text("/cost".into())]);

        let output = parse(&serde_json::json!({"type": "system", "subtype": "local_command",
            "uuid": "l2", "content": "<local-command-stdout>$0.12</local-command-stdout>"}));
        assert_eq!(output.role(), Role::System);
        assert!(output.content().is_empty());
    }

    fn assistant_line(blocks: &serde_json::Value, usage: &serde_json::Value) -> CcodeMessage {
        parse(&serde_json::json!({
            "type": "assistant", "uuid": "t1",
            "message": {"id": "msg_01", "role": "assistant", "content": blocks, "usage": usage},
        }))
    }

    /// A call split over several lines repeats its usage on each; only the line with the
    /// thinking block is marked as reasoning.
    #[rstest]
    #[case::tool_use(serde_json::json!([{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {}}]))]
    #[case::text(serde_json::json!([{"type": "text", "text": "done"}]))]
    fn split_line_without_thinking_gains_no_reasoning_block(#[case] blocks: serde_json::Value) {
        let usage = serde_json::json!({"input_tokens": 2, "output_tokens": 206,
            "output_tokens_details": {"thinking_tokens": 21}});
        let content = assistant_line(&blocks, &usage).content();
        assert!(
            !content.iter().any(|c| matches!(c, Content::ReasoningSummary { .. })),
            "{content:?}"
        );
    }

    #[rstest]
    #[case::final_usage(serde_json::json!({"output_tokens": 206,
        "output_tokens_details": {"thinking_tokens": 21}}), Some(21))]
    // Subagent transcripts write the thinking line with the stream's opening usage.
    #[case::opening_usage(serde_json::json!({"output_tokens": 5}), None)]
    fn thinking_line_carries_the_calls_thinking_tokens(
        #[case] usage: serde_json::Value,
        #[case] expected: Option<u64>,
        #[values("thinking", "redacted_thinking")] kind: &str,
    ) {
        let blocks = serde_json::json!([{"type": kind}]);
        assert_eq!(assistant_line(&blocks, &usage).content(), vec![Content::ReasoningSummary {
            tokens: expected
        }]);
    }

    #[rstest]
    #[case::total(serde_json::json!({"cache_creation_input_tokens": 7,
        "cache_creation": {"ephemeral_5m_input_tokens": 3, "ephemeral_1h_input_tokens": 1}}), Some(7))]
    #[case::split_only(serde_json::json!({
        "cache_creation": {"ephemeral_5m_input_tokens": 3, "ephemeral_1h_input_tokens": 4}}), Some(7))]
    #[case::neither(serde_json::json!({"input_tokens": 1}), None)]
    fn cache_write_falls_back_to_the_lifetime_split(
        #[case] usage: serde_json::Value,
        #[case] expected: Option<u64>,
    ) {
        let usage = assistant_line(&serde_json::json!([]), &usage).usage().unwrap();
        assert_eq!(usage.cache_write, expected);
    }

    #[rstest]
    fn image_blocks_drop_their_bytes() {
        let m = parse(&serde_json::json!({"type": "user", "uuid": "i1",
        "message": {"role": "user", "content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/png",
                "data": "PRIVATE_BYTES"}},
            {"type": "text", "text": "what is this?"},
        ]}}));
        assert_eq!(m.role(), Role::User);
        let content = m.content();
        assert!(!format!("{content:?}").contains("PRIVATE_BYTES"));
        assert_eq!(content[1], Content::Text("what is this?".into()));
    }

    /// The session Claude Code 2.1.281 wrote for `-p` turns driven against a mock API: a
    /// thinking reply, a tool call (blocked by a `PreToolUse` hook), `--continue`, `/compact`
    /// and a turn after it. Paths are redacted, harness attachments cut down to their type.
    const MOCK_SESSION: &str = include_str!("../../../tests/fixtures/ccode/session4.jsonl");

    fn fixture(jsonl: &str) -> Vec<CcodeMessage> {
        jsonl.lines().map(|l| serde_json::from_str(l).expect("fixture line parses")).collect()
    }

    fn texts(messages: &[CcodeMessage], role: &Role) -> Vec<String> {
        messages
            .iter()
            .filter(|m| &m.role() == role)
            .flat_map(Message::content)
            .filter_map(|c| match c {
                Content::Text(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    /// Every prompt the user typed comes out once, as typed; the harness's copies of them
    /// (`queue-operation`), its caveat and the command's output do not.
    #[rstest]
    fn mock_session_user_text_is_each_prompt_once() {
        let messages = fixture(MOCK_SESSION);
        assert_eq!(texts(&messages, &Role::User), [
            "BASE turn one SC_THINK",
            "second turn via resume SC_TOOL",
            "third turn via continue",
            "/compact",
            "after compact turn",
        ]);
        assert!(
            messages.iter().filter(|m| m.kind == "queue-operation").all(|m| m.content().is_empty())
        );
    }

    /// Each model call has its own turn id on every line of it, with the call's usage.
    #[rstest]
    fn mock_session_calls_keep_their_turn_and_usage() {
        let messages = fixture(MOCK_SESSION);
        let assistant: Vec<_> = messages.iter().filter(|m| m.role() == Role::Assistant).collect();
        assert_eq!(assistant.len(), 8);
        assert!(assistant.iter().all(|m| m.turn_id().is_some() && m.usage().is_some()));
        let mut turns: Vec<_> = assistant.iter().filter_map(|m| m.turn_id()).collect();
        turns.dedup();
        assert_eq!(turns.len(), 5, "{turns:?}");
        let thinking = assistant[0];
        assert_eq!(thinking.content(), vec![Content::ReasoningSummary { tokens: Some(12) }]);
        assert_eq!(thinking.usage().unwrap().reasoning, Some(12));
    }

    /// The compaction summary is the model's summary, without the wrapper Claude Code adds (which
    /// names the local transcript file); the boundary keeps the chain across the compaction.
    #[rstest]
    fn mock_session_compaction() {
        let messages = fixture(MOCK_SESSION);
        let summaries: Vec<_> = messages
            .iter()
            .flat_map(Message::content)
            .filter_map(|c| match c {
                Content::Summary(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(summaries, ["MOCK COMPACT SUMMARY of the chat"]);
        let boundary =
            messages.iter().find(|m| m.subtype.as_deref() == Some("compact_boundary")).unwrap();
        assert!(boundary.parent_uuid.is_none());
        assert_eq!(boundary.parent_id(), boundary.logical_parent_uuid.clone().map(MessageId::from));
    }

    /// A tool's result is sent in the user's turn but is not the user speaking.
    #[rstest]
    fn mock_session_tool_result_is_a_tool_line() {
        let messages = fixture(MOCK_SESSION);
        let result = messages
            .iter()
            .find(|m| m.content().iter().any(|c| matches!(c, Content::ToolResult(_))))
            .unwrap();
        assert_eq!(result.role(), Role::Tool);
    }

    /// Claude Code's wrapper around the model's summary, current and older.
    #[rstest]
    #[case::current(
        "This session is being continued from a previous conversation that ran out of context. \
         The summary below covers the earlier portion of the conversation.\n\nSummary:\n1. Fixed \
         forks.\n\n2. Tests pass.\n\nIf you need specific details from before compaction (like \
         exact code snippets, error messages, or content you generated), read the full transcript \
         at: /home/PRIVATE/s.jsonl\n\nRecent messages are preserved verbatim.\nContinue the \
         conversation from where it left off without asking the user any further questions.",
        "1. Fixed forks.\n\n2. Tests pass."
    )]
    #[case::no_summary_tags(
        "This session is being continued from a previous conversation that ran out of context. \
         The summary below covers the earlier portion of the conversation.\n\nFixed forks.",
        "Fixed forks."
    )]
    #[case::older(
        "This session is being continued from a previous conversation that ran out of context. \
         The conversation is summarized below:\nAnalysis: fixed forks.\nPlease continue the \
         conversation from where we left it off without asking the user any further questions.",
        "Analysis: fixed forks."
    )]
    #[case::unknown_wrapper("Summary of the work so far.", "Summary of the work so far.")]
    fn compact_summary_is_the_models_summary(#[case] text: &str, #[case] expected: &str) {
        let m = parse(&serde_json::json!({"type": "user", "uuid": "c1", "isCompactSummary": true,
            "message": {"role": "user", "content": text}}));
        assert_eq!(m.content(), vec![Content::Summary(expected.into())]);
    }

    /// Tool results are the tool's turn, whether the line carries only `tool_result` blocks or
    /// is one Claude Code split off it (an image a tool returned), marked by `toolUseResult`.
    #[rstest]
    #[case::tool_result(serde_json::json!({"toolUseResult": {"stdout": "PRIVATE"},
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_1", "content": "PRIVATE"}]}}))]
    #[case::without_tool_use_result(serde_json::json!({
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_1", "content": "PRIVATE"}]}}))]
    #[case::split_image(serde_json::json!({"toolUseResult": "Error: rejected",
        "message": {"role": "user", "content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "x"}}]}}))]
    fn tool_result_line_is_the_tools(#[case] extra: serde_json::Value) {
        let mut raw = serde_json::json!({"type": "user", "uuid": "t1"});
        raw.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        assert_eq!(parse(&raw).role(), Role::Tool);
    }

    /// What the user typed when rejecting a tool call is their turn (`userFeedback`, CC
    /// 2.1.281); a rejection without it is the tool's result.
    #[rstest]
    #[case::with_feedback(Some("use rstest instead"), Role::User)]
    #[case::without_feedback(None, Role::Tool)]
    #[case::blank_feedback(Some("  "), Role::Tool)]
    fn rejected_tool_call_feedback_is_the_users(
        #[case] feedback: Option<&str>,
        #[case] role: Role,
    ) {
        let mut raw = serde_json::json!({"type": "user", "uuid": "r1",
            "toolUseResult": "Error: The user doesn't want to proceed with this tool use.",
            "toolDenialKind": "user-rejected",
            "message": {"role": "user", "content": [{"type": "tool_result", "is_error": true,
                "tool_use_id": "toolu_1",
                "content": "The user doesn't want to proceed with this tool use. The tool use \
                    was rejected (eg. if it was a file edit, the new_string was NOT written to \
                    the file). To tell you how to proceed, the user said:\nuse rstest instead"}]}});
        if let Some(feedback) = feedback {
            raw["userFeedback"] = serde_json::json!(feedback);
        }
        let m = parse(&raw);
        assert_eq!(m.role(), role);
        let content = m.content();
        assert!(matches!(content[0], Content::ToolResult(ToolResult { error: true, .. })));
        assert_eq!(
            content.get(1),
            feedback.filter(|f| !f.trim().is_empty()).map(|f| Content::Text(f.into())).as_ref()
        );
    }

    /// Bookkeeping lines with a top-level `content` carry no conversation: a `queue-operation`
    /// copies the prompt that is written again as the user's line.
    #[rstest]
    #[case::enqueue("enqueue")]
    #[case::remove("remove")]
    fn queue_operation_has_no_content(#[case] operation: &str) {
        let m = parse(&serde_json::json!({"type": "queue-operation", "operation": operation,
            "timestamp": "2026-09-24T02:53:53.183Z", "sessionId": "s", "content": "a prompt"}));
        assert!(m.content().is_empty());
    }

    /// Tools the API runs itself (web search) are tool calls and results like any other; the
    /// text that cites them is the reply.
    #[rstest]
    fn server_tool_blocks_are_tool_calls() {
        let m = parse(&serde_json::json!({"type": "assistant", "uuid": "w1",
        "message": {"id": "msg_1", "role": "assistant", "content": [
            {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
                "input": {"query": "atuin"}},
            {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1",
                "content": {"type": "web_search_tool_result_error", "error_code": "unavailable"}},
            {"type": "text", "text": "Atuin is a shell history tool.", "citations": [
                {"type": "web_search_result_location", "url": "https://example.invalid/"}]},
        ]}}));
        let content = m.content();
        assert!(
            matches!(&content[0], Content::ToolUse(ToolUse { name, .. }) if name == "web_search")
        );
        assert!(matches!(&content[1], Content::ToolResult(ToolResult { error: true, call, .. })
            if call.as_ref() == "srvtoolu_1"));
        assert_eq!(content[2], Content::Text("Atuin is a shell history tool.".into()));
    }

    /// Teammate messages are delivered in the user's turn (`W_e` in CC 2.1.281).
    #[rstest]
    fn teammate_message_is_system() {
        let m =
            parse(&serde_json::json!({"type": "user", "uuid": "m1", "message": {"role": "user",
            "content": "<teammate-message teammate_id=\"t1\">done</teammate-message>"}}));
        assert_eq!(m.role(), Role::System);
    }

    /// A user line whose raw JSON content string is `content` (escapes as written).
    fn raw_user_line(content: &[u8]) -> Vec<u8> {
        [
            br#"{"type":"user","uuid":"u1","message":{"role":"user","content":""#.as_slice(),
            content,
            br#""}}"#,
        ]
        .concat()
    }

    /// Lines `serde_json` rejects but Claude Code's own loader reads (`JSON.parse` of the lossy
    /// UTF-8 text, a leading byte order mark dropped) are read the way it reads them.
    #[rstest]
    #[case::lone_high_surrogate_at_the_end(raw_user_line(br"cut \ud83d"), "cut \u{fffd}")]
    #[case::lone_high_surrogate_before_text(raw_user_line(br"a\ud83db"), "a\u{fffd}b")]
    #[case::two_high_surrogates(raw_user_line(br"\ud83d\ud83d\ude00"), "\u{fffd}\u{1f600}")]
    #[case::lone_low_surrogate(raw_user_line(br"a\ude00b"), "a\u{fffd}b")]
    #[case::byte_order_mark([b"\xef\xbb\xbf".as_slice(), &raw_user_line(b"hi")].concat(), "hi")]
    #[case::invalid_utf8(raw_user_line(b"a\xffb"), "a\u{fffd}b")]
    fn a_line_claude_code_reads_is_read(#[case] raw: Vec<u8>, #[case] expected: &str) {
        let m = CcodeMessage::decode(&raw).unwrap();
        assert_eq!(m.id(), Some(MessageId::from("u1".to_owned())));
        assert_eq!(m.content(), vec![Content::Text(expected.to_owned())]);
    }

    #[rstest]
    fn a_line_nothing_repairs_keeps_its_error() {
        let err = CcodeMessage::decode(br#"{"type":"user","uuid":"u0","mess"#).unwrap_err();
        assert!(err.is_eof());
    }

    /// End to end: the lines Claude Code reads reach the session's messages, a torn line is an
    /// error that does not end them.
    #[rstest]
    #[tokio::test]
    async fn a_transcript_is_read_as_claude_code_reads_it(projects: Projects) {
        let path = projects.project.join("s1.jsonl");
        let lines = [
            [b"\xef\xbb\xbf".as_slice(), &raw_user_line(b"first")].concat(),
            br#"{"type":"user","uuid":"u0","mess"#.to_vec(),
            br#"{"type":"user","uuid":"u2","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"Error: \ud83d"}]},"toolUseResult":"Error: \ud83d"}"#.to_vec(),
        ];
        let mut file = lines.join(&b'\n');
        file.push(b'\n');
        std::fs::write(&path, file).unwrap();
        let session = CcodeSession::open(SessionId::from("s1".to_owned()), path, pool());
        let read: Vec<_> = session.read().collect().await;
        assert!(matches!(
            &read[1],
            Err(MessageError::Jsonl(crate::json::jsonl::JsonlError::Parse { .. }))
        ));
        let read: Vec<CcodeMessage> = read.into_iter().filter_map(Result::ok).collect();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].content(), vec![Content::Text("first".to_owned())]);
        assert_eq!(read[1].role(), Role::Tool);
        assert!(matches!(&read[1].content()[..],
            [Content::ToolResult(result)] if result.output == "Error: \u{fffd}"));
    }
}
