//! Pi coding agent sessions: one JSONL file per session, a `session` header line followed by
//! tree-linked entries (pi-mono `packages/coding-agent/src/core/session-manager.ts`).
//!
//! Three pi behaviours shape this reader:
//!
//! - **Forks copy entries verbatim.** `/fork` (`forkFrom`) and `/tree` into a new session
//!   (`createBranchedSession`) copy every prior entry, ids, timestamps and usage included, into a
//!   new file whose header names the parent *file path* (`parentSession`). So a model call's
//!   [`turn_id`](Message::turn_id) is derived only from what the copy keeps, and the parent path
//!   is resolved to the parent's session id.
//! - **The session id is the header's `id`.** Pi names files `<timestamp>_<id>.jsonl`, but
//!   `pi --session <path>` keeps any explicit name, so the header is authoritative. A file
//!   without one is no session to pi (session-manager.ts `loadEntriesFromFile`), nor here:
//!   extensions keep other JSONL beside sessions, such as pi-subagents' event logs.
//! - **Old files are migrated in place.** Opening a v1 file rewrites it with random entry ids
//!   (`migrateV1ToV2`). Those ids are ignored (see [`PiMessage`]) so a line keeps the identity it
//!   had before the rewrite.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use futures::stream::FuturesUnordered;
use futures::{Stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Deserializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::watch;
use typed_builder::TypedBuilder;

use crate::fs::tree_watcher::{FileStat, TreeWatcher};
use crate::harnesstools::pi::{Pi, agent_dir, expand_tilde};
use crate::harnesstools::session::model::{
    Content, MessageId, Role, StopReason, TitleChange, TitleSource, ToolCallId, ToolResult,
    ToolUse, Usage,
};
use crate::harnesstools::session::{
    Checkpoint, Listener, Message, MessageError, Observable, RuntimeError, Session, SessionId,
    Sessions, WatchError, scan_sessions,
};
use crate::io::{FollowLines, Line, PathLineReader, PooledReadLines};
use crate::json::jsonl::JsonlExt;
use crate::sync::BlockingPool;
use crate::utils::env_nonempty;

/// How far into a file the header line is looked for, like pi's own bounded header scan
/// (session-manager.ts `MAX_SESSION_HEADER_SCAN_BYTES`).
const MAX_HEADER_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, TypedBuilder)]
pub struct PiSessions {
    #[builder(default, setter(strip_option, into))]
    root: Option<PathBuf>,
    /// Runs every file read of the sessions this finds.
    pool: BlockingPool,
}

impl PiSessions {
    fn resolve_root(&self) -> PathBuf {
        if let Some(root) = &self.root {
            return root.clone();
        }
        session_root(&agent_dir(), env_nonempty("PI_CODING_AGENT_SESSION_DIR").as_deref())
    }
}

/// Where pi keeps sessions when no `--session-dir` is given (pi-mono coding-agent `main.ts`):
/// `PI_CODING_AGENT_SESSION_DIR`, else the global `settings.json` `sessionDir`, else
/// `<agent dir>/sessions`, the first two with `~` expanded. A project's own
/// `.pi/settings.json` can move its sessions too; no single root covers that.
fn session_root(agent_dir: &Path, env: Option<&std::ffi::OsStr>) -> PathBuf {
    if let Some(dir) = env {
        return expand_tilde(dir);
    }
    settings_session_dir(agent_dir).unwrap_or_else(|| agent_dir.join("sessions"))
}

/// The global `settings.json` `sessionDir` (pi-mono coding-agent `settings-manager.ts`
/// `getSessionDir`), read as pi reads the file: JSON, a leading BOM tolerated.
fn settings_session_dir(agent_dir: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(agent_dir.join("settings.json")).ok()?;
    let settings: serde_json::Value =
        serde_json::from_str(raw.strip_prefix('\u{feff}').unwrap_or(&raw)).ok()?;
    let dir = settings.get("sessionDir")?.as_str().filter(|d| !d.is_empty())?;
    // A relative `sessionDir` is relative to wherever pi was started: no one root.
    Some(expand_tilde(std::ffi::OsStr::new(dir))).filter(|dir| dir.is_absolute())
}

impl Sessions for PiSessions {
    type Listener = PiListener;

    fn listener(&self) -> Result<PiListener, RuntimeError> {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(PiListener {
            root,
            pool: self.pool.clone(),
        })
    }

    fn existing(
        &self,
    ) -> Result<impl Stream<Item = Result<PiSession, RuntimeError>> + Send + 'static, RuntimeError>
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
                    scan_sessions(root, |path, is_file| {
                        PiListener::open_session(path, is_file, &sessions_pool)
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

impl Observable for Pi {
    type Sessions = PiSessions;

    fn sessions(&self, pool: BlockingPool) -> PiSessions {
        PiSessions::builder().pool(pool).build()
    }
}

#[derive(Debug, Clone)]
pub struct PiListener {
    root: PathBuf,
    pool: BlockingPool,
}

impl PiListener {
    /// Whether `path` could be a pi session file.
    fn is_session_file(path: &Path) -> bool {
        path.extension().is_some_and(|ext| ext == "jsonl")
    }

    /// The session id a session file's `header` gives it; `None` for a file that is no session,
    /// or not yet one.
    fn session_id(header: Header) -> Option<SessionId> {
        match header {
            Header::Session { id } => Some(SessionId::from(id)),
            Header::Other | Header::Pending => None,
        }
    }

    /// Build a read-once session for an accepted `jsonl` file (no change signal), or `None`.
    ///
    /// Reads the file's first line for the session id. `None` too while that line is still
    /// being written: the listener waits for the file to change before trying again.
    fn open_session(path: &Path, is_file: bool, pool: &BlockingPool) -> Option<PiSession> {
        if !is_file || !Self::is_session_file(path) {
            return None;
        }
        let id = Self::session_id(read_header(path).ok()?)?;
        Some(PiSession::open(id, path.to_path_buf(), pool.clone()))
    }

    /// The live session over a watched file, once its header says which session it is.
    ///
    /// Pi creates a session file before (or while) writing its header, so a file with no complete
    /// header yet is read again on each change to it, as is one that could not be read: the
    /// failure may pass, and a file that is gone closes `stat`. `None` if the file turns out to be
    /// no session, or is gone before it becomes one.
    async fn await_session(
        path: PathBuf,
        mut stat: watch::Receiver<FileStat>,
        pool: BlockingPool,
    ) -> Option<PiSession> {
        loop {
            stat.borrow_and_update();
            let at = path.clone();
            match pool.run(move || read_header(&at)).await.ok()? {
                Ok(Header::Pending) => {}
                Ok(header) => {
                    return Some(PiSession {
                        id: Self::session_id(header)?,
                        path,
                        changes: Some(stat),
                        pool,
                    });
                }
                Err(err) => {
                    tracing::debug!(
                        ?err,
                        path = %path.display(),
                        "failed to read a session header; trying again once the file changes"
                    );
                }
            }
            stat.changed().await.ok()?;
        }
    }
}

impl Listener for PiListener {
    type Session = PiSession;

    fn watch(self) -> impl Stream<Item = Result<PiSession, WatchError>> + Send + 'static {
        let root = self.root;
        let pool = self.pool;
        async_stream::stream! {
            let files = TreeWatcher::builder().filter(Self::is_session_file).watch(&root);
            let mut files = match files {
                Ok(files) => files,
                Err(err) => {
                    yield Err(WatchError::from(err));
                    return;
                }
            };
            // Files whose header is not written yet wait here, without holding up the others.
            let mut pending = FuturesUnordered::new();
            let mut files_done = false;
            loop {
                tokio::select! {
                    file = files.next(), if !files_done => match file {
                        Some(file) => {
                            let (path, stat) = file.into_parts();
                            let session = Self::await_session(path.to_path_buf(), stat, pool.clone());
                            pending.push(session);
                        }
                        None => files_done = true,
                    },
                    Some(session) = pending.next(), if !pending.is_empty() => {
                        if let Some(session) = session {
                            yield Ok(session);
                        }
                    }
                    else => break,
                }
            }
        }
    }
}

/// What the first line of a would-be session file says.
#[derive(Debug, PartialEq, Eq)]
enum Header {
    /// A pi `session` header.
    Session {
        id: String,
    },
    /// The first entry is not a session header: no session, to pi.
    Other,
    /// Nothing complete yet: an empty file, or a first line still being written.
    Pending,
}

/// Read the header of the session file at `path`: its first entry, bounded by
/// [`MAX_HEADER_BYTES`]. As in pi's `readSessionHeader`, blank and malformed lines before it are
/// skipped, and a final line without a newline counts once it parses.
fn read_header(path: &Path) -> io::Result<Header> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file.take(MAX_HEADER_BYTES));
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(Header::Pending);
        }
        let terminated = line.last() == Some(&b'\n');
        if line.trim_ascii().is_empty() {
            if terminated {
                continue;
            }
            return Ok(Header::Pending);
        }
        let entry = match crate::json::js::from_slice::<serde_json::Value>(&line) {
            Ok(entry) => entry,
            Err(_) if terminated => continue,
            // A half-written line: wait for the rest.
            Err(_) => return Ok(Header::Pending),
        };
        return Ok(match (entry.get("type"), entry.get("id")) {
            (Some(kind), Some(serde_json::Value::String(id))) if *kind == "session" => {
                Header::Session { id: id.clone() }
            }
            _ => Header::Other,
        });
    }
}

/// The session id pi's own file naming (`<timestamp>_<id>.jsonl`, session-manager.ts
/// `newSession`) puts in `path`: everything after the first `_`, which the timestamp never
/// contains. The whole stem for any other name.
fn file_name_id(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    Some(stem.split_once('_').map_or(stem.as_ref(), |(_, id)| id).to_owned())
}

#[derive(Debug, Clone)]
pub struct PiSession {
    id: SessionId,
    path: PathBuf,
    /// Wakes [`messages`](Session::messages) on each change to the file; `None` reads it once.
    changes: Option<watch::Receiver<FileStat>>,
    pool: BlockingPool,
}

impl PiSession {
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

/// Resolve a header's `parentSession` file path to the parent's session id by reading the
/// parent's own header, so a custom-named parent (`pi --session <path>`) resolves too. Other
/// lines pass through untouched.
async fn resolve_parent(
    mut message: PiMessage,
    dir: Option<&Path>,
    pool: &BlockingPool,
) -> PiMessage {
    let Some(parent) = message.parent_session_path() else {
        return message;
    };
    let parent = match dir {
        Some(dir) if Path::new(parent).is_relative() => dir.join(parent),
        _ => PathBuf::from(parent),
    };
    if let Ok(Ok(Header::Session { id })) = pool.run(move || read_header(&parent)).await {
        message.resolved_parent = Some(SessionId::from(id));
    }
    message
}

impl Session for PiSession {
    type Message = PiMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages_from(
        self,
        from: Option<Checkpoint>,
    ) -> impl Stream<Item = Result<(Checkpoint, PiMessage), MessageError>> + Send + 'static {
        async_stream::stream! {
            let start = match from {
                Some(from) => self.start(from).await,
                None => 0,
            };
            let dir = self.path.parent().map(Path::to_path_buf);
            let lines = FollowLines::new(PooledReadLines::new(
                PathLineReader::at(&self.path, start),
                self.pool.clone(),
            ));
            let messages = match self.changes {
                Some(changes) => lines.follow(changes).left_stream(),
                None => lines.read_to_end().right_stream(),
            }
            .js_json::<PiMessage>();
            for await item in messages {
                match item {
                    Ok((line, message)) => {
                        let message = resolve_parent(message, dir.as_deref(), &self.pool).await;
                        yield Ok((Checkpoint::new(line.end, &line.bytes), message));
                    }
                    Err(err) => yield Err(MessageError::from(err)),
                }
            }
        }
    }

    fn read(&self) -> impl Stream<Item = Result<PiMessage, MessageError>> + Send + 'static {
        let dir = self.path.parent().map(Path::to_path_buf);
        let pool = self.pool.clone();
        FollowLines::new(PooledReadLines::new(PathLineReader::new(&self.path), self.pool.clone()))
            .read_to_end()
            .js_json::<PiMessage>()
            .map_err(MessageError::from)
            .and_then(move |(_, message)| {
                let (dir, pool) = (dir.clone(), pool.clone());
                async move { Ok(resolve_parent(message, dir.as_deref(), &pool).await) }
            })
    }
}

/// One line of a pi session file.
///
/// A line pi's v1→v2 migration gave an id (`migrateV1ToV2`, which rewrites the file in place)
/// is recognised by its key order: pi builds every entry as `type, id, parentId, timestamp, ..`
/// (or with extension fields before `id`), while the migration appends `id` and `parentId` to a
/// v1 entry that already had its `timestamp`. Such a line reports no [`id`](Message::id) or
/// [`parent_id`](Message::parent_id): the random ids did not exist when the line may first have
/// been captured, so it keeps the content-derived identity it had then.
#[derive(Debug, Clone)]
pub struct PiMessage {
    entry: PiEntry,
    /// `id` and `parentId` were assigned by pi's v1→v2 migration, not written with the entry.
    migrated: bool,
    /// On the header: the parent's session id, read from the parent file's own header.
    resolved_parent: Option<SessionId>,
}

impl<'de> Deserialize<'de> for PiMessage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // The workspace builds serde_json with `preserve_order`, so the map keeps the line's
        // key order.
        let fields = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        let position = |key: &str| fields.keys().position(|k| k == key);
        let migrated = matches!(
            (position("timestamp"), position("id")),
            (Some(timestamp), Some(id)) if timestamp < id
        );
        let entry = PiEntry::deserialize(serde_json::Value::Object(fields))
            .map_err(serde::de::Error::custom)?;
        Ok(Self {
            entry,
            migrated,
            resolved_parent: None,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PiEntry {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    parent_id: Option<String>,
    timestamp: Option<String>,
    message: Option<serde_json::Value>,
    cwd: Option<PathBuf>,
    /// On `model_change` (and the v1 header): the model switched to.
    model_id: Option<String>,
    /// On `usage`: the model the usage is attributed to.
    model: Option<String>,
    /// On `session_info`: a name the user gave the session.
    name: Option<String>,
    /// On the `session` header: the file of the session this one was forked from.
    parent_session: Option<String>,
    /// On a v1 `session` header: what `parentSession` was called then.
    branched_from: Option<String>,
    /// On `compaction` / `branch_summary`: the model-written summary.
    summary: Option<String>,
    /// On `compaction` / `branch_summary` / `usage`: the LLM call(s) the entry records.
    usage: Option<serde_json::Value>,
    /// On `custom_message`: what an extension injected into the model's context.
    content: Option<serde_json::Value>,
}

/// Entry types whose top-level `usage` records a model call of their own.
fn has_own_usage(kind: &str) -> bool {
    matches!(kind, "compaction" | "branch_summary" | "usage")
}

impl PiMessage {
    fn block(value: &serde_json::Value) -> Content {
        match value["type"].as_str() {
            Some("text") => Content::Text(value["text"].as_str().unwrap_or_default().to_owned()),
            Some("thinking") => Content::ReasoningSummary { tokens: None },
            Some("toolCall") => Content::ToolUse(ToolUse {
                id: ToolCallId::from(value["id"].as_str().unwrap_or_default().to_owned()),
                name: value["name"].as_str().unwrap_or_default().to_owned(),
                input: value["arguments"].clone(),
            }),
            // A pasted or attached image (pi-ai types.ts `ImageContent`): keep what it was, not
            // its base64 bytes.
            Some("image") => Content::Other(serde_json::json!({
                "type": "image",
                "mimeType": value["mimeType"],
            })),
            _ => Content::Other(value.clone()),
        }
    }

    /// A pi content value: a plain string or an array of blocks.
    fn blocks(content: &serde_json::Value) -> Vec<Content> {
        match content {
            serde_json::Value::String(text) => vec![Content::Text(text.clone())],
            serde_json::Value::Array(blocks) => blocks.iter().map(Self::block).collect(),
            _ => Vec::new(),
        }
    }

    /// A `message` entry's message. Only those: extensions keep agent event logs that embed
    /// messages (`message_end`, `turn_end`) beside sessions, and those calls are the ones a
    /// session already records.
    fn message(&self) -> Option<&serde_json::Value> {
        (self.entry.kind == "message").then_some(self.entry.message.as_ref()).flatten()
    }

    /// `message.role`, on a `message` entry.
    fn message_role(&self) -> Option<&str> {
        self.message()?.get("role")?.as_str()
    }

    fn message_str(&self, key: &str) -> Option<&str> {
        self.message()?.get(key)?.as_str()
    }

    /// The raw usage object this line reports as session usage: an assistant message's, or the
    /// top-level one of an entry that records its own model call. A tool result's `usage` is
    /// "not part of main LLM context accounting" (pi-ai types.ts `ToolResultMessage`).
    fn raw_usage(&self) -> Option<&serde_json::Value> {
        let usage = if has_own_usage(&self.entry.kind) {
            self.entry.usage.as_ref()
        } else if self.message_role() == Some("assistant") {
            self.message()?.get("usage")
        } else {
            None
        };
        usage.filter(|u| u.is_object())
    }

    /// The header's `parentSession` (v1: `branchedFrom`) file path.
    fn parent_session_path(&self) -> Option<&str> {
        if self.entry.kind != "session" {
            return None;
        }
        self.entry
            .parent_session
            .as_deref()
            .or(self.entry.branched_from.as_deref())
            .filter(|p| !p.trim().is_empty())
    }
}

impl Message for PiMessage {
    fn id(&self) -> Option<MessageId> {
        (!self.migrated).then(|| self.entry.id.clone().map(MessageId::from)).flatten()
    }

    fn role(&self) -> Role {
        let role = match self.entry.kind.as_str() {
            "message" => self.message_role().unwrap_or("message"),
            // Summaries pi feeds back to the model as context (session-manager.ts
            // `sessionEntryToContextMessages`); neither the user nor the model said them.
            "compaction" | "branch_summary" => return Role::System,
            "custom_message" => "custom",
            other => other,
        };
        match role {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "system" | "branchSummary" | "compactionSummary" => Role::System,
            "toolResult" | "tool" => Role::Tool,
            "bashExecution" => Role::User,
            // Extension-injected context (`custom_message` entries, `role: "custom"` messages,
            // v2's `hookMessage` before pi's v3 migration renamed it). Pi sends it to the model
            // as a user message (messages.ts `convertToLlm`), but the user did not write it;
            // `display` only picks how pi's TUI renders it.
            "custom" | "hookMessage" => Role::Other("custom".to_owned()),
            other => Role::Other(other.to_owned()),
        }
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.entry.timestamp.as_deref().and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
    }

    fn content(&self) -> Vec<Content> {
        match self.entry.kind.as_str() {
            "compaction" | "branch_summary" => {
                return self
                    .entry
                    .summary
                    .iter()
                    .filter(|s| !s.is_empty())
                    .map(|s| Content::Summary(s.clone()))
                    .collect();
            }
            "custom_message" => {
                return self.entry.content.as_ref().map(Self::blocks).unwrap_or_default();
            }
            "message" => {}
            _ => return Vec::new(),
        }
        let Some(message) = self.message() else {
            return Vec::new();
        };
        match self.message_role() {
            Some("toolResult") => vec![Content::ToolResult(ToolResult {
                call: ToolCallId::from(
                    message["toolCallId"].as_str().unwrap_or_default().to_owned(),
                ),
                output: message["content"].clone(),
                error: message["isError"].as_bool().unwrap_or(false),
            })],
            // A `!command` the user ran in pi's own shell, with what it printed; `!!command`
            // when the user kept it out of the model's context (messages.ts
            // `BashExecutionMessage.excludeFromContext`). It has no tool call of its own, so the
            // entry names it (a v1 entry by its time). A cancelled command has no exit code and
            // failed all the same.
            Some("bashExecution") => {
                let prefix = if message["excludeFromContext"].as_bool() == Some(true) {
                    "!!"
                } else {
                    "!"
                };
                vec![
                    Content::Text(format!(
                        "{prefix}{}",
                        message["command"].as_str().unwrap_or_default()
                    )),
                    Content::ToolResult(ToolResult {
                        call: ToolCallId::from(self.id().map_or_else(
                            || {
                                format!(
                                    "bash:{}",
                                    self.entry.timestamp.as_deref().unwrap_or_default()
                                )
                            },
                            String::from,
                        )),
                        output: message["output"].clone(),
                        error: message["cancelled"].as_bool() == Some(true)
                            || message["exitCode"].as_i64().is_some_and(|c| c != 0),
                    }),
                ]
            }
            Some("branchSummary" | "compactionSummary") => message["summary"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| vec![Content::Summary(s.to_owned())])
                .unwrap_or_default(),
            role => {
                let mut content = Self::blocks(&message["content"]);
                // A failed or aborted call usually has no content; `errorMessage` is what pi
                // shows for it (pi-ai types.ts `AssistantMessage.errorMessage`).
                if role == Some("assistant")
                    && let Some(error) = self.message_str("errorMessage").filter(|e| !e.is_empty())
                {
                    content.push(Content::Error(error.to_owned()));
                }
                content
            }
        }
    }

    fn model(&self) -> Option<String> {
        // Assistant turns name their model (the one the provider reports first); a
        // `model_change` line or v1 header names it at the top, a `usage` entry as `model`.
        self.message_str("responseModel")
            .or_else(|| self.message_str("model"))
            .or(self.entry.model_id.as_deref())
            .or(self.entry.model.as_deref())
            .map(str::to_owned)
    }

    /// `input` is the uncached prompt: pi-ai reports `cacheRead` separately (its context size is
    /// `input + cacheRead`, pi-ai utils/overflow.ts).
    fn usage(&self) -> Option<Usage> {
        let usage = self.raw_usage()?;
        let count = |key: &str| usage.get(key).and_then(serde_json::Value::as_u64);
        Some(Usage {
            input: count("input"),
            output: count("output"),
            cache_read: count("cacheRead"),
            cache_write: count("cacheWrite"),
            reasoning: count("reasoning"),
        })
    }

    fn stop_reason(&self) -> Option<StopReason> {
        let raw = self.message_str("stopReason")?;
        Some(match raw {
            "stop" => StopReason::EndTurn,
            "toolUse" => StopReason::ToolUse,
            "length" => StopReason::MaxTokens,
            "aborted" => StopReason::Aborted,
            "error" => StopReason::Error,
            other => StopReason::Other(other.to_owned()),
        })
    }

    fn parent_id(&self) -> Option<MessageId> {
        (!self.migrated).then(|| self.entry.parent_id.clone().map(MessageId::from)).flatten()
    }

    /// Only the `session` header carries the directory; it is the first line, so the session row
    /// gets it before any turn.
    fn cwd(&self) -> Option<PathBuf> {
        self.entry.cwd.clone()
    }

    /// Only the `session` header names a parent, as the parent's file path; this is the id that
    /// file's header carries when the session could read it, else the id its file name carries.
    fn parent_session(&self) -> Option<SessionId> {
        let path = self.parent_session_path()?;
        self.resolved_parent.clone().or_else(|| file_name_id(Path::new(path)).map(SessionId::from))
    }

    /// A `session_info` name, trimmed as pi reads it (session-manager.ts `getSessionName`). A
    /// blank name clears pi's title, which a line cannot express here: it assigns none.
    /// The session's name; a blank one clears it, as pi's `getSessionName` reads it.
    fn title(&self) -> Option<TitleChange> {
        if self.entry.kind != "session_info" {
            return None;
        }
        Some(TitleChange::new(TitleSource::Named, self.entry.name.as_deref().unwrap_or_default()))
    }

    /// One model call, identified by what a fork's verbatim copy keeps: the entry's id and
    /// timestamp, else (a v1 line, with no id or a migration-assigned one) its timestamps and
    /// token counts.
    ///
    /// Never the provider's `responseId` alone: it is only as unique as the provider makes it.
    /// Ollama's OpenAI-compatible endpoint names every response `chatcmpl-<0..998>`
    /// (middleware/openai.go), so unrelated calls would share a turn and count usage once.
    fn turn_id(&self) -> Option<String> {
        if !has_own_usage(&self.entry.kind) && self.message_role() != Some("assistant") {
            return None;
        }
        let timestamp = self.entry.timestamp.as_deref().unwrap_or_default();
        if let Some(id) = self.id() {
            return Some(format!("entry:{id}:{timestamp}"));
        }
        let sent = self
            .message()
            .and_then(|m| m.get("timestamp"))
            .map(ToString::to_string)
            .unwrap_or_default();
        let tokens = self.usage().unwrap_or_default();
        let count = |n: Option<u64>| n.map(|n| n.to_string()).unwrap_or_default();
        Some(format!(
            "line:{}:{timestamp}:{sent}:{}:{}:{}:{}",
            self.entry.kind,
            count(tokens.input),
            count(tokens.output),
            count(tokens.cache_read),
            count(tokens.cache_write),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use futures::{StreamExt, TryStreamExt};
    use rstest::rstest;

    use super::*;
    use crate::sync::BlockingPool;

    fn pool() -> BlockingPool {
        BlockingPool::new(std::num::NonZeroUsize::MIN)
    }
    use crate::harnesstools::session::model::{Content, Role};
    use crate::harnesstools::session::{Message, Session, Sessions};

    #[rstest]
    fn normalizes_a_pi_user_message() {
        let raw = serde_json::json!({
            "type": "message",
            "id": "m1",
            "parentId": null,
            "timestamp": "2026-09-18T10:00:00Z",
            "message": {"role": "user", "content": "hello pi"},
        })
        .to_string();
        let m: PiMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.role(), Role::User);
        assert_eq!(m.content(), vec![Content::Text("hello pi".into())]);
    }

    #[rstest]
    fn normalizes_pi_tool_call_and_result() {
        let call: PiMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "message",
                "id": "m1",
                "message": {"role": "assistant", "content": [
                    {"type": "toolCall", "id": "c1", "name": "bash", "arguments": {"cmd": "ls"}}
                ]},
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            matches!(call.content().as_slice(), [Content::ToolUse(u)] if u.name == "bash" && u.id.as_ref() == "c1")
        );

        let result: PiMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "message",
                "id": "m2",
                "message": {"role": "toolResult", "toolCallId": "c1", "isError": false, "content": "done"},
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(result.role(), Role::Tool);
        assert!(
            matches!(result.content().as_slice(), [Content::ToolResult(r)] if r.call.as_ref() == "c1" && !r.error)
        );
    }

    #[rstest]
    fn normalizes_pi_assistant_enrichment_fields() {
        let raw = serde_json::json!({
            "type": "message",
            "id": "m1",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "done"}],
                "model": "claude-opus-4-8",
                "stopReason": "stop",
                "usage": {"input": 10, "output": 20, "cacheRead": 5, "cacheWrite": 2},
            },
        })
        .to_string();
        let m: PiMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.model(), Some("claude-opus-4-8".to_owned()));
        assert_eq!(m.stop_reason(), Some(StopReason::EndTurn));
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
    #[case("toolUse", StopReason::ToolUse)]
    #[case("aborted", StopReason::Aborted)]
    #[case("length", StopReason::MaxTokens)]
    #[case("error", StopReason::Error)]
    #[case("weird", StopReason::Other("weird".to_owned()))]
    fn maps_pi_stop_reason_vocabulary(#[case] raw: &str, #[case] expected: StopReason) {
        let m: PiMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "message",
                "id": "m1",
                "message": {"role": "assistant", "content": [], "stopReason": raw},
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(m.stop_reason(), Some(expected));
    }

    #[rstest]
    fn enrichment_is_none_when_the_harness_did_not_provide_it() {
        let raw = serde_json::json!({
            "type": "message",
            "id": "m1",
            "message": {"role": "user", "content": "hi"},
        })
        .to_string();
        let m: PiMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.model(), None);
        assert_eq!(m.usage(), None);
        assert_eq!(m.stop_reason(), None);
        assert_eq!(m.cwd(), None);
        assert_eq!(m.git_branch(), None);
    }

    /// `!cmd` / `!!cmd` lines as pi 0.85.1 writes them (messages.ts `BashExecutionMessage`). An
    /// aborted command is written with `cancelled: true` and no `exitCode` at all.
    #[rstest]
    #[case::ok(
        r#"{"role":"bashExecution","command":"ls","output":"a\nb","exitCode":0,"cancelled":false,"truncated":false,"timestamp":1790217991905}"#,
        "!ls",
        false
    )]
    #[case::failed(
        r#"{"role":"bashExecution","command":"ls","output":"a\nb","exitCode":2,"cancelled":false,"truncated":false,"timestamp":1790217991905}"#,
        "!ls",
        true
    )]
    #[case::cancelled(
        r#"{"role":"bashExecution","command":"ls","output":"a\nb","cancelled":true,"truncated":false,"timestamp":1790218055302}"#,
        "!ls",
        true
    )]
    #[case::kept_out_of_context(
        r#"{"role":"bashExecution","command":"ls","output":"a\nb","exitCode":0,"cancelled":false,"truncated":false,"timestamp":1790217991916,"excludeFromContext":true}"#,
        "!!ls",
        false
    )]
    fn bash_execution_is_a_user_command_with_its_output(
        #[case] message: &str,
        #[case] typed: &str,
        #[case] error: bool,
    ) {
        let m: PiMessage = serde_json::from_str(&format!(
            r#"{{"type":"message","id":"m9","parentId":"m8","timestamp":"2026-09-24T02:46:31.905Z","message":{message}}}"#
        ))
        .unwrap();
        assert_eq!(m.role(), Role::User);
        assert_eq!(m.content(), vec![
            Content::Text(typed.into()),
            Content::ToolResult(ToolResult {
                call: ToolCallId::from("m9".to_owned()),
                output: serde_json::json!("a\nb"),
                error,
            }),
        ]);
    }

    /// Ollama's OpenAI-compatible endpoint names every response `chatcmpl-<0..998>`, so pi
    /// records the same `responseId` for unrelated calls; each still is its own turn.
    #[rstest]
    fn a_reused_response_id_does_not_merge_calls() {
        let call = |id: &str, ts: &str| {
            pi(&serde_json::json!({"type": "message", "id": id, "parentId": "u1", "timestamp": ts,
                "message": {"role": "assistant", "content": [{"type": "text", "text": "hi"}],
                    "api": "openai-completions", "provider": "ollama", "model": "qwen3:8b",
                    "usage": {"input": 215, "output": 40, "cacheRead": 0, "cacheWrite": 0,
                        "totalTokens": 255}, "stopReason": "stop", "timestamp": 1_790_217_992_640_u64,
                    "responseId": "chatcmpl-42"}}))
        };
        let first = call("a08ea294", "2026-09-24T02:46:32.649Z");
        let second = call("e28a7d96", "2026-09-24T02:47:12.652Z");
        assert_ne!(first.turn_id(), second.turn_id());
    }

    /// Images keep their kind, never their base64 bytes (pi-ai types.ts `ImageContent`).
    #[rstest]
    fn image_blocks_drop_their_bytes() {
        let m =
            pi(&serde_json::json!({"type": "message", "id": "1080a317", "parentId": "ca417218",
            "timestamp": "2026-09-24T02:47:34.550Z", "message": {"role": "user", "content": [
                {"type": "text", "text": "look at this image"},
                {"type": "image", "data": "PRIVATE_BYTES", "mimeType": "image/png"}],
            "timestamp": 1_790_218_054_548_u64}}));
        assert_eq!(m.content(), vec![
            Content::Text("look at this image".to_owned()),
            Content::Other(serde_json::json!({"type": "image", "mimeType": "image/png"})),
        ]);
    }

    #[rstest]
    fn exposes_parent_id_and_session_info_title() {
        let m: PiMessage = serde_json::from_str(
            &serde_json::json!({"type": "message", "id": "m2", "parentId": "m1",
                "message": {"role": "user", "content": "hi"}})
            .to_string(),
        )
        .unwrap();
        assert_eq!(m.parent_id(), Some(MessageId::from("m1".to_owned())));
        assert_eq!(m.title(), None);

        let info: PiMessage = serde_json::from_str(
            &serde_json::json!({"type": "session_info", "id": "m3", "parentId": "m2", "name": "my session"})
                .to_string(),
        )
        .unwrap();
        assert_eq!(info.title().and_then(|t| t.text).as_deref(), Some("my session"));
    }

    /// pi writes `parentSession` as the parent's *file path* (session-manager.ts
    /// `createBranchedSession`, `forkFrom`); a v1 header called it `branchedFrom`. Without the
    /// parent's header to hand, the id comes from pi's `<timestamp>_<id>.jsonl` file name.
    #[rstest]
    #[case::fork(
        serde_json::json!({"type": "session", "version": 3, "id": "s2", "cwd": "/w",
            "parentSession": "/home/u/.pi/agent/sessions/--w--/2026-09-18T10-00-00-000Z_0199aaaa-bbbb.jsonl"}),
        Some("0199aaaa-bbbb")
    )]
    #[case::bare_file_name(
        serde_json::json!({"type": "session", "id": "s2", "cwd": "/w",
            "parentSession": "2026-09-18T10-00-00-000Z_0199aaaa-bbbb.jsonl"}),
        Some("0199aaaa-bbbb")
    )]
    #[case::v1_branched_from(
        serde_json::json!({"type": "session", "id": "s2", "cwd": "/w",
            "branchedFrom": "/s/--w--/2025-12-09T00-52-54-397Z_d97339c6-6c10.jsonl"}),
        Some("d97339c6-6c10")
    )]
    #[case::root(serde_json::json!({"type": "session", "id": "s2", "cwd": "/w"}), None)]
    #[case::not_a_header(
        serde_json::json!({"type": "message", "id": "m1", "parentSession": "/x/1_p.jsonl",
            "message": {"role": "user", "content": "hi"}}),
        None
    )]
    fn the_header_names_the_parent_session(
        #[case] raw: serde_json::Value,
        #[case] parent: Option<&str>,
    ) {
        let m: PiMessage = serde_json::from_str(&raw.to_string()).unwrap();
        assert_eq!(m.parent_session(), parent.map(|p| SessionId::from(p.to_owned())));
    }

    /// Pi's own precedence (coding-agent `main.ts`): `PI_CODING_AGENT_SESSION_DIR`, then the
    /// global `settings.json` `sessionDir`, then `<agent dir>/sessions`, `~` expanded.
    #[rstest]
    #[case::default(None, None, "<agent>/sessions")]
    #[case::env(Some("/s/env"), Some(r#"{"sessionDir": "/s/settings"}"#), "/s/env")]
    #[case::env_tilde(Some("~/pi-sessions"), None, "<home>/pi-sessions")]
    #[case::settings(None, Some(r#"{"sessionDir": "<abs>/settings"}"#), "<abs>/settings")]
    #[case::settings_bom(
        None,
        Some("\u{feff}{\"sessionDir\": \"<abs>/settings\"}"),
        "<abs>/settings"
    )]
    #[case::settings_tilde(None, Some(r#"{"sessionDir": "~/x"}"#), "<home>/x")]
    #[case::settings_relative(None, Some(r#"{"sessionDir": "x"}"#), "<agent>/sessions")]
    #[case::settings_without(None, Some(r#"{"theme": "dark"}"#), "<agent>/sessions")]
    #[case::settings_malformed(None, Some("{nope"), "<agent>/sessions")]
    fn finds_the_session_root_as_pi_does(
        #[case] env: Option<&str>,
        #[case] settings: Option<&str>,
        #[case] expected: &str,
    ) {
        // `/s` has a root but no drive, so Windows does not count it as absolute.
        let abs = if cfg!(windows) {
            "C:/s"
        } else {
            "/s"
        };
        let agent = tempfile::tempdir().unwrap();
        if let Some(settings) = settings {
            std::fs::write(agent.path().join("settings.json"), settings.replace("<abs>", abs))
                .unwrap();
        }
        let expected = expected
            .replace("<abs>", abs)
            .replace("<agent>", &agent.path().to_string_lossy())
            .replace("<home>", &crate::utils::home_dir().to_string_lossy());
        assert_eq!(
            session_root(agent.path(), env.map(std::ffi::OsStr::new)),
            PathBuf::from(expected)
        );
    }

    #[rstest]
    fn listener_reports_not_found_for_a_missing_root() {
        let sessions =
            PiSessions::builder().root(PathBuf::from("/no/such/pi")).pool(pool()).build();
        assert!(matches!(sessions.listener(), Err(RuntimeError::NotFound(_))));
    }

    #[rstest]
    #[tokio::test]
    async fn messages_streams_turns_from_a_pi_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1700000000_s1.jsonl");
        let body = [
            serde_json::json!({"type": "session", "id": "s1", "cwd": "/w"}).to_string(),
            serde_json::json!({
                "type": "message",
                "id": "m1",
                "message": {"role": "assistant", "content": "hi"},
            })
            .to_string(),
        ]
        // Trailing newline required: messages() withholds an unterminated final line until a
        // later write completes it (a real session ends every record with a newline).
        .join("\n")
            + "\n";
        std::fs::write(&path, body).unwrap();

        let session = PiSession::open(SessionId::from("s1".to_owned()), path, pool());
        let roles: Vec<Role> =
            session.messages().take(2).map_ok(|m| m.role()).try_collect().await.unwrap();
        assert_eq!(roles.last(), Some(&Role::Assistant));
    }

    #[rstest]
    #[tokio::test]
    async fn watch_emits_sessions_as_files_appear() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("--proj--");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            sub.join("1700000000_s1.jsonl"),
            serde_json::json!({"type": "session", "id": "s1"}).to_string(),
        )
        .unwrap();

        let listener = PiSessions::builder()
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
        assert_eq!(seen, vec![SessionId::from("s1".to_owned())]);
    }

    fn session_file(dir: &Path) -> PathBuf {
        let path = dir.join("1700000000_s1.jsonl");
        std::fs::write(
            &path,
            serde_json::json!({"type": "session", "id": "s1", "cwd": "/w"}).to_string() + "\n",
        )
        .unwrap();
        path
    }

    #[rstest]
    #[tokio::test]
    async fn messages_yields_lines_appended_after_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = session_file(dir.path());

        let listener = PiSessions::builder()
            .root(dir.path().to_path_buf())
            .pool(pool())
            .build()
            .listener()
            .unwrap();
        // The watch stream owns the watcher: it must outlive the message stream.
        let mut sessions = std::pin::pin!(listener.watch());
        let session = sessions.next().await.unwrap().unwrap();
        let mut messages = std::pin::pin!(session.messages());
        assert!(messages.next().await.unwrap().is_ok());

        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(
            &mut file,
            (serde_json::json!({
                "type": "message",
                "id": "m1",
                "message": {"role": "assistant", "content": "hi"},
            })
            .to_string()
                + "\n")
                .as_bytes(),
        )
        .unwrap();
        drop(file);
        assert_eq!(messages.next().await.unwrap().unwrap().role(), Role::Assistant);
    }

    #[rstest]
    #[tokio::test]
    async fn messages_ends_when_the_session_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = session_file(dir.path());

        let listener = PiSessions::builder()
            .root(dir.path().to_path_buf())
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
    #[tokio::test]
    async fn read_yields_all_messages_and_terminates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1700000000_abc.jsonl");
        // Trailing newline required: read() reads complete lines only, like live capture, so an
        // unterminated final line is treated as still being written and left out.
        let body = [
            serde_json::json!({"type": "message", "id": "m1", "message": {"role": "user", "content": "hi"}})
                .to_string(),
            serde_json::json!({"type": "message", "id": "m2", "message": {"role": "assistant", "content": "yo"}})
                .to_string(),
        ]
        .join("\n")
            + "\n";
        std::fs::write(&path, body).unwrap();
        let session = PiSession::open(SessionId::from("abc".to_owned()), path, pool());
        let got: Vec<PiMessage> = session.read().try_collect().await.unwrap();
        assert_eq!(got.len(), 2);
    }

    #[rstest]
    #[tokio::test]
    async fn existing_finds_every_session_file_under_root() {
        let dir = tempfile::tempdir().unwrap();
        let header = |id: &str| serde_json::json!({"type": "session", "id": id}).to_string() + "\n";
        std::fs::write(dir.path().join("1_a.jsonl"), header("a")).unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("nested").join("2_b.jsonl"), header("b")).unwrap();
        std::fs::write(dir.path().join("ignore.txt"), header("c")).unwrap();
        // pi-subagents' default event log, beside the sessions: no header, no session.
        std::fs::create_dir(dir.path().join("subagent-artifacts")).unwrap();
        std::fs::write(
            dir.path().join("subagent-artifacts").join("run1_worker_0.jsonl"),
            b"{\"type\":\"agent_start\"}\n",
        )
        .unwrap();
        let sessions = PiSessions::builder().root(dir.path().to_path_buf()).pool(pool()).build();
        let mut ids: Vec<String> =
            sessions.existing().unwrap().map(|s| s.unwrap().id().into()).collect().await;
        ids.sort();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn existing_does_not_follow_symlinked_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("1_real.jsonl"),
            serde_json::json!({"type": "session", "id": "real"}).to_string() + "\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();
        let sessions = PiSessions::builder().root(dir.path().to_path_buf()).pool(pool()).build();
        let ids: Vec<String> =
            sessions.existing().unwrap().map(|s| s.unwrap().id().into()).collect().await;
        assert_eq!(ids, vec!["real".to_string()]);
    }

    #[rstest]
    #[case(include_str!("../../../tests/fixtures/pi/session1.jsonl"))]
    #[case(include_str!("../../../tests/fixtures/pi/session2.jsonl"))]
    fn normalizes_a_real_redacted_session(#[case] jsonl: &str) {
        let msgs: Vec<PiMessage> = jsonl
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<PiMessage>(l).expect("fixture record parses"))
            .collect();
        assert!(msgs.len() >= 10);

        let mut saw_user = false;
        let mut saw_assistant = false;
        let mut tool_uses = 0usize;
        let mut tool_results = 0usize;
        for m in &msgs {
            let _ = m.timestamp();
            saw_user |= m.role() == Role::User;
            saw_assistant |= m.role() == Role::Assistant;
            for c in m.content() {
                match c {
                    Content::ToolUse(u) => {
                        assert!(!u.name.is_empty(), "toolCall normalized to an empty name");
                        assert!(!u.id.to_string().is_empty(), "toolCall normalized to an empty id");
                        tool_uses += 1;
                    }
                    Content::ToolResult(r) => {
                        assert!(!r.call.to_string().is_empty(), "toolResult lost its call id");
                        tool_results += 1;
                    }
                    _ => {}
                }
            }
        }
        assert!(saw_user && saw_assistant, "expected both user and assistant turns");
        assert!(tool_uses >= 1, "expected at least one normalized toolCall");
        assert!(tool_results >= 1, "expected at least one normalized toolResult");
    }

    /// A session pi 0.85.1 wrote against a local mock provider (only the cwd rewritten): turns,
    /// a tool call, `!`/`!!` commands, a failed, a mid-stream failed, a truncated and an
    /// aborted call, a name, a label, extension entries, a compaction, a `/tree` branch summary
    /// and a switch to an OpenAI-compatible model.
    #[rstest]
    fn reads_a_session_pi_wrote() {
        let msgs: Vec<PiMessage> =
            include_str!("../../../tests/fixtures/pi/session-mock-0.85.jsonl")
                .lines()
                .map(|l| serde_json::from_str::<PiMessage>(l).expect("line parses"))
                .collect();
        let all: Vec<Content> = msgs.iter().flat_map(PiMessage::content).collect();
        let has = |c: &Content| all.contains(c);

        assert!(has(&Content::Text("!echo user-bang; exit 2".to_owned())));
        assert!(has(&Content::Text("!!echo hidden-bang".to_owned())));
        assert!(all.iter().any(|c| matches!(c, Content::ToolResult(r)
            if r.call.as_ref() == "1726195d" && r.error)));
        assert!(has(&Content::Error("This operation was aborted".to_owned())));
        assert!(has(&Content::Text("INJECTED CONTEXT".to_owned())));
        assert_eq!(all.iter().filter(|c| matches!(c, Content::Summary(_))).count(), 2);

        let stops: Vec<StopReason> = msgs.iter().filter_map(PiMessage::stop_reason).collect();
        for stop in
            [StopReason::Error, StopReason::Aborted, StopReason::MaxTokens, StopReason::ToolUse]
        {
            assert!(stops.contains(&stop), "{stop:?}");
        }
        // Every failed or aborted call says why.
        for m in msgs
            .iter()
            .filter(|m| matches!(m.stop_reason(), Some(StopReason::Error | StopReason::Aborted)))
        {
            assert!(m.content().iter().any(|c| matches!(c, Content::Error(_))), "{m:?}");
        }

        // One turn per usage-bearing line (13 assistant calls, the compaction, the summary),
        // none shared.
        let turns: Vec<String> =
            msgs.iter().filter(|m| m.usage().is_some()).filter_map(PiMessage::turn_id).collect();
        assert_eq!(turns.len(), msgs.iter().filter(|m| m.usage().is_some()).count());
        assert_eq!(turns.len(), 15);
        assert_eq!(turns.iter().collect::<std::collections::HashSet<_>>().len(), turns.len());

        let summaries: Vec<&PiMessage> =
            msgs.iter().filter(|m| m.entry.kind == "compaction").collect();
        assert_eq!(summaries[0].usage().and_then(|u| u.input), Some(221));
        assert_eq!(summaries[0].role(), Role::System);

        let titles: Vec<String> = msgs.iter().filter_map(|m| m.title()?.text).collect();
        assert_eq!(titles, vec!["sdk named session   x".to_owned()]);
        let models: std::collections::HashSet<String> =
            msgs.iter().filter_map(PiMessage::model).collect();
        assert_eq!(
            models,
            ["mock-claude", "mock-claude-resp", "mock-gpt", "mock-gpt-resp"]
                .map(str::to_owned)
                .into()
        );
        assert_eq!(msgs[0].cwd(), Some(PathBuf::from("/home/u/proj")));
    }

    fn pi(raw: &serde_json::Value) -> PiMessage {
        serde_json::from_str(&raw.to_string()).unwrap()
    }

    fn texts(m: &PiMessage) -> Vec<String> {
        m.content()
            .into_iter()
            .filter_map(|c| match c {
                Content::Text(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    fn tokens(input: u64, output: u64) -> Usage {
        Usage {
            input: Some(input),
            output: Some(output),
            cache_read: Some(0),
            cache_write: Some(0),
            reasoning: None,
        }
    }

    /// pi writes `parentSession` as the parent's *file path* (session-manager.ts:1674
    /// `createBranchedSession`, :1851 `forkFrom`), never its id.
    #[rstest]
    #[case("/home/u/.pi/agent/sessions/--w--/2026-09-18T10-00-00-000Z_0199aaaa-bbbb.jsonl")]
    #[case("2026-09-18T10-00-00-000Z_0199aaaa-bbbb.jsonl")]
    fn header_parent_session_path_resolves_to_the_parent_id(#[case] parent: &str) {
        let m = pi(&serde_json::json!({
            "type": "session", "version": 3, "id": "0199cccc", "timestamp": "2026-09-18T10:00:00Z",
            "cwd": "/w", "parentSession": parent,
        }));
        assert_eq!(m.parent_session(), Some(SessionId::from("0199aaaa-bbbb".to_owned())));
    }

    /// A failed assistant turn typically has empty content; the only user-visible text is
    /// `errorMessage` (pi-ai types.ts `AssistantMessage.errorMessage`, shown by the TUI's
    /// assistant-message.ts).
    #[rstest]
    #[case("error", StopReason::Error, "529 overloaded_error")]
    #[case("aborted", StopReason::Aborted, "Request was aborted")]
    fn a_failed_assistant_turn_keeps_its_error_message(
        #[case] stop: &str,
        #[case] reason: StopReason,
        #[case] error: &str,
    ) {
        let m = pi(&serde_json::json!({
            "type": "message", "id": "a1", "parentId": "u1", "timestamp": "2026-09-18T10:00:00Z",
            "message": {"role": "assistant", "content": [], "stopReason": stop, "errorMessage": error,
                "model": "m", "usage": {"input": 1, "output": 0, "cacheRead": 0, "cacheWrite": 0}},
        }));
        assert_eq!(m.content(), vec![Content::Error(error.to_owned())]);
        assert_eq!(m.stop_reason(), Some(reason));
    }

    #[rstest]
    fn a_successful_assistant_turn_has_no_error() {
        let m = pi(&serde_json::json!({"type": "message", "id": "a1",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "ok"}],
                "stopReason": "stop", "errorMessage": ""}}));
        assert_eq!(m.content(), vec![Content::Text("ok".to_owned())]);
    }

    /// `compaction` / `branch_summary` carry a top-level `summary` (session-manager.ts
    /// `CompactionEntry`, `BranchSummaryEntry`) that pi feeds back into context
    /// (`sessionEntryToContextMessages`).
    #[rstest]
    #[case(serde_json::json!({"type": "compaction", "id": "c1", "parentId": "a1",
        "timestamp": "2026-09-18T10:00:00Z", "summary": "SUMMARY-TEXT", "firstKeptEntryId": "u1",
        "tokensBefore": 1000}))]
    #[case(serde_json::json!({"type": "branch_summary", "id": "b1", "parentId": "a1",
        "timestamp": "2026-09-18T10:00:00Z", "summary": "SUMMARY-TEXT", "fromId": "a9"}))]
    fn summary_entries_keep_their_summary(#[case] raw: serde_json::Value) {
        let m = pi(&raw);
        assert_eq!(m.content(), vec![Content::Summary("SUMMARY-TEXT".to_owned())]);
        assert_eq!(m.role(), Role::System);
    }

    /// `compaction` / `branch_summary` record the usage of the LLM call that wrote the summary,
    /// and a `usage` entry that of a call outside the context (cache warming), all as a top-level
    /// `usage` (session-manager.ts `CompactionEntry`, `BranchSummaryEntry`, `UsageEntry`). Each
    /// is its own model call, named by the entry.
    #[rstest]
    #[case::compaction(serde_json::json!({"type": "compaction", "summary": "s",
        "firstKeptEntryId": "u1", "tokensBefore": 1000}), None)]
    #[case::branch_summary(serde_json::json!({"type": "branch_summary", "summary": "s",
        "fromId": "a9"}), None)]
    #[case::cache_warm(serde_json::json!({"type": "usage", "kind": "cache_warm",
        "provider": "anthropic", "model": "claude-opus-4-8"}), Some("claude-opus-4-8"))]
    fn entries_with_their_own_usage_report_it(
        #[case] mut raw: serde_json::Value,
        #[case] model: Option<&str>,
    ) {
        raw.as_object_mut().unwrap().extend(
            serde_json::json!({"id": "c1", "parentId": "a1", "timestamp": "2026-09-18T10:00:00Z",
                "usage": {"input": 900, "output": 100, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": 1000}})
            .as_object()
            .unwrap()
            .clone(),
        );
        let m = pi(&raw);
        assert_eq!(m.usage(), Some(tokens(900, 100)));
        assert_eq!(m.turn_id().as_deref(), Some("entry:c1:2026-09-18T10:00:00Z"));
        assert_eq!(m.model().as_deref(), model);
    }

    /// `custom_message` keeps its content at the top level (session-manager.ts
    /// `CustomMessageEntry`); `role: "custom"` messages (v2: `hookMessage`) under `message`.
    /// Either way an extension wrote it, not the user, whatever pi's TUI does with `display`.
    #[rstest]
    #[case::entry_shown(serde_json::json!({"type": "custom_message", "id": "x1", "parentId": "a1",
        "timestamp": "2026-09-18T10:00:00Z", "customType": "ext", "content": "INJECTED", "display": true}))]
    #[case::entry_hidden(serde_json::json!({"type": "custom_message", "customType": "ext",
        "content": [{"type": "text", "text": "INJECTED"}], "display": false, "id": "x1",
        "parentId": "a1", "timestamp": "2026-09-18T10:00:00Z"}))]
    #[case::message(serde_json::json!({"type": "message", "id": "x1", "timestamp": "2026-09-18T10:00:00Z",
        "message": {"role": "custom", "customType": "ext", "content": "INJECTED", "display": true}}))]
    #[case::v2_hook_message(serde_json::json!({"type": "message", "id": "x1",
        "timestamp": "2026-09-18T10:00:00Z",
        "message": {"role": "hookMessage", "customType": "ext", "content": "INJECTED", "display": true}}))]
    fn custom_messages_keep_their_content(#[case] raw: serde_json::Value) {
        let m = pi(&raw);
        assert_eq!(texts(&m), vec!["INJECTED".to_owned()]);
        assert_eq!(m.role(), Role::Other("custom".to_owned()));
        assert_eq!(m.id(), Some(MessageId::from("x1".to_owned())));
        assert_eq!(m.usage(), None);
    }

    /// A tool result's `usage` is "not part of main LLM context accounting" (pi-ai types.ts
    /// `ToolResultMessage`); tokscale counts assistant usage only (tokscale sessions/pi.rs).
    #[rstest]
    fn tool_result_usage_is_not_session_usage() {
        let m = pi(&serde_json::json!({
            "type": "message", "id": "t1", "parentId": "a1", "timestamp": "2026-09-18T10:00:00Z",
            "message": {"role": "toolResult", "toolCallId": "c1", "toolName": "subagent",
                "content": [], "isError": false,
                "usage": {"input": 5000, "output": 500, "cacheRead": 0, "cacheWrite": 0}},
        }));
        assert_eq!(m.usage(), None);
        assert_eq!(m.turn_id(), None);
    }

    /// Agent events embed the message they are about; pi-subagents logs them as JSONL beside the
    /// sessions (`subagent-artifacts/<run>_<agent>.jsonl`). Only a `message` entry is one, so an
    /// event never adds a call its session already records.
    #[rstest]
    #[case::message_start("message_start")]
    #[case::message_end("message_end")]
    #[case::turn_end("turn_end")]
    fn an_event_embedding_a_message_is_no_message(#[case] kind: &str) {
        let m = pi(&serde_json::json!({"type": kind, "message": {"role": "assistant",
            "content": [{"type": "text", "text": "hi"}], "model": "claude-opus-4-8",
            "usage": {"input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0},
            "stopReason": "stop", "timestamp": 1_790_217_992_640_u64}}));
        assert_eq!(m.role(), Role::Other(kind.to_owned()));
        assert_eq!(m.content(), vec![]);
        assert_eq!(m.usage(), None);
        assert_eq!(m.turn_id(), None);
        assert_eq!(m.model(), None);
        assert_eq!(m.stop_reason(), None);
    }

    /// pi trims a `session_info` name, and a blank one clears the title (session-manager.ts
    /// `getSessionName`). Clearing is not expressible per line, so a blank name assigns none.
    #[rstest]
    #[case("", None)]
    #[case("   ", None)]
    #[case("  my session ", Some("my session"))]
    fn session_info_names_are_trimmed_titles(#[case] name: &str, #[case] title: Option<&str>) {
        let m = pi(&serde_json::json!({"type": "session_info", "id": "i1", "parentId": "a1",
            "timestamp": "2026-09-18T10:00:00Z", "name": name}));
        assert_eq!(m.title().and_then(|t| t.text).as_deref(), title);
    }

    /// `pi --session <path>` keeps any explicit file name (session-manager.ts `_setSessionFile`,
    /// "preserve explicit path"), so the file stem is not the session id; the header's `id` is.
    #[rstest]
    #[tokio::test]
    async fn session_id_comes_from_the_header_not_the_file_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("my_notes.jsonl"),
            serde_json::json!({"type": "session", "version": 3, "id": "0199dddd", "cwd": "/w",
                "timestamp": "2026-09-18T10:00:00Z"})
            .to_string()
                + "\n",
        )
        .unwrap();
        let sessions = PiSessions::builder().root(dir.path().to_path_buf()).pool(pool()).build();
        let ids: Vec<String> =
            sessions.existing().unwrap().map(|s| s.unwrap().id().into()).collect().await;
        assert_eq!(ids, vec!["0199dddd".to_owned()]);
    }

    /// Pi creates a session file before (or while) writing its header; until the header line is
    /// there the file is no session yet, and the listener waits for it to grow.
    #[rstest]
    #[tokio::test]
    async fn existing_skips_a_file_whose_header_is_not_written_yet() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("2026-09-18T10-00-00-000Z_s1.jsonl"), b"").unwrap();
        std::fs::write(dir.path().join("2026-09-18T10-00-00-000Z_s2.jsonl"), br#"{"type":"sess"#)
            .unwrap();
        let sessions = PiSessions::builder().root(dir.path().to_path_buf()).pool(pool()).build();
        let found: Vec<_> = sessions.existing().unwrap().collect().await;
        assert!(found.is_empty(), "{found:?}");
    }

    /// A file found before its header is written becomes a session, under the header's id, once
    /// the header lands; a file that already has one is not held up by it meanwhile.
    #[rstest]
    #[tokio::test]
    async fn watch_waits_for_a_header_being_written() {
        let dir = tempfile::tempdir().unwrap();
        let pending = dir.path().join("2026-09-18T10-00-00-000Z_s1.jsonl");
        std::fs::write(&pending, br#"{"type":"sess"#).unwrap();
        let listener = PiSessions::builder()
            .root(dir.path().to_path_buf())
            .pool(pool())
            .build()
            .listener()
            .unwrap();
        let mut sessions = std::pin::pin!(listener.watch());
        let timeout = std::time::Duration::from_secs(10);

        std::fs::write(
            dir.path().join("my_notes.jsonl"),
            serde_json::json!({"type": "session", "id": "0199aaaa"}).to_string() + "\n",
        )
        .unwrap();
        let first = tokio::time::timeout(timeout, sessions.next()).await.unwrap().unwrap().unwrap();
        assert_eq!(first.id(), SessionId::from("0199aaaa".to_owned()));

        std::fs::write(
            &pending,
            serde_json::json!({"type": "session", "id": "0199bbbb"}).to_string() + "\n",
        )
        .unwrap();
        let second =
            tokio::time::timeout(timeout, sessions.next()).await.unwrap().unwrap().unwrap();
        assert_eq!(second.id(), SessionId::from("0199bbbb".to_owned()));
    }

    #[rstest]
    #[case::empty(b"".as_slice(), Header::Pending)]
    #[case::half_written(br#"{"type":"session","id":"#.as_slice(), Header::Pending)]
    #[case::unterminated_but_whole(
        br#"{"type":"session","id":"s1"}"#.as_slice(),
        Header::Session { id: "s1".to_owned() }
    )]
    #[case::after_blank_lines(
        b"\n  \n{\"type\":\"session\",\"id\":\"s1\"}\n{}\n".as_slice(),
        Header::Session { id: "s1".to_owned() }
    )]
    #[case::not_a_header(b"{\"type\":\"message\",\"id\":\"m1\"}\n".as_slice(), Header::Other)]
    #[case::id_not_a_string(b"{\"type\":\"session\",\"id\":1}\n".as_slice(), Header::Other)]
    #[case::garbage(b"nope\n".as_slice(), Header::Pending)]
    #[case::after_garbage(
        b"nope\n{\"type\":\"session\",\"id\":\"s1\"}\n".as_slice(),
        Header::Session { id: "s1".to_owned() }
    )]
    #[case::bom(
        b"\xef\xbb\xbf{\"type\":\"session\",\"id\":\"s1\"}\n{\"type\":\"model_change\"}\n"
            .as_slice(),
        Header::Other
    )]
    #[case::event_log(b"{\"type\":\"agent_start\"}\n".as_slice(), Header::Other)]
    fn reads_the_header_line(#[case] body: &[u8], #[case] expected: Header) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, body).unwrap();
        assert_eq!(read_header(&path).unwrap(), expected);
    }

    /// The session reading a fork resolves its parent through the parent file's own header, so a
    /// parent kept under an explicit name (`pi --session <path>`) resolves to its id too, the
    /// same id [`Session::id`] gives the parent.
    #[rstest]
    #[tokio::test]
    async fn a_fork_resolves_its_parent_through_the_parents_header() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("my_notes.jsonl");
        std::fs::write(
            &parent,
            serde_json::json!({"type": "session", "version": 3, "id": "0199aaaa", "cwd": "/w"})
                .to_string()
                + "\n",
        )
        .unwrap();
        let fork = dir.path().join("2026-09-18T11-00-00-000Z_0199bbbb.jsonl");
        std::fs::write(
            &fork,
            serde_json::json!({"type": "session", "version": 3, "id": "0199bbbb", "cwd": "/w",
                "parentSession": parent})
            .to_string()
                + "\n",
        )
        .unwrap();

        let sessions = PiSessions::builder().root(dir.path().to_path_buf()).pool(pool()).build();
        let mut found: Vec<PiSession> = sessions.existing().unwrap().try_collect().await.unwrap();
        found.sort_by_key(|s| String::from(s.id()));
        let ids: Vec<String> = found.iter().map(|s| s.id().into()).collect();
        assert_eq!(ids, vec!["0199aaaa".to_owned(), "0199bbbb".to_owned()]);

        let fork = found.pop().unwrap();
        let expected = Some(SessionId::from("0199aaaa".to_owned()));
        let read: Vec<PiMessage> = fork.read().try_collect().await.unwrap();
        assert_eq!(read[0].parent_session(), expected);
        let followed: Vec<(Checkpoint, PiMessage)> =
            fork.clone().messages_from(None).try_collect().await.unwrap();
        assert_eq!(followed[0].1.parent_session(), expected);
        let messages: Vec<PiMessage> = fork.messages().try_collect().await.unwrap();
        assert_eq!(messages[0].parent_session(), expected);
    }

    /// `/fork` and `/tree` copy entries verbatim, ids and timestamps included (session-manager.ts
    /// `forkFrom`, `createBranchedSession`), so a call is named by what the copy keeps: the entry
    /// id with its timestamp (8 random hex digits alone are not unique across sessions), whatever
    /// `responseId` the provider gave.
    #[rstest]
    #[case::response_id(serde_json::json!({"responseId": "msg_01ABC"}))]
    #[case::blank_response_id(serde_json::json!({"responseId": ""}))]
    #[case::no_response_id(serde_json::json!({}))]
    fn an_assistant_turn_is_named_by_what_a_fork_copies(#[case] extra: serde_json::Value) {
        let turn = "entry:a1b2c3d4:2026-09-18T10:00:05Z";
        let mut message = serde_json::json!({"role": "assistant", "content": [],
            "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0}});
        message.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let original =
            pi(&serde_json::json!({"type": "message", "id": "a1b2c3d4", "parentId": "u1",
            "timestamp": "2026-09-18T10:00:05Z", "message": message}));
        // `createBranchedSession` re-parents the copy; nothing else changes.
        let copy = pi(&serde_json::json!({"type": "message", "id": "a1b2c3d4", "parentId": null,
            "timestamp": "2026-09-18T10:00:05Z", "message": message}));
        assert_eq!(original.turn_id().as_deref(), Some(turn));
        assert_eq!(copy.turn_id(), original.turn_id());
    }

    /// Mimic pi's `migrateV1ToV2` on one line: `id` and `parentId` appended to the entry (a JS
    /// property assignment), `firstKeptEntryIndex` swapped for `firstKeptEntryId`.
    fn migrate_v1(line: &str, id: &str, parent: Option<&str>) -> String {
        let mut entry: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(line).unwrap();
        entry.insert("id".to_owned(), id.into());
        entry.insert("parentId".to_owned(), parent.into());
        if entry.shift_remove("firstKeptEntryIndex").is_some() {
            entry.insert("firstKeptEntryId".to_owned(), "0badf00d".into());
        }
        serde_json::Value::Object(entry).to_string()
    }

    /// pi migrates a v1 file in place when it opens it (session-manager.ts `_loadEntries` ->
    /// `_rewriteFile`), giving every entry a random id. Each line reads the same before and
    /// after, so a re-read after the rewrite resolves to the rows captured before it.
    #[rstest]
    fn a_v1_session_reads_the_same_after_pi_migrates_it() {
        let lines: Vec<&str> =
            include_str!("../../../tests/fixtures/pi/session-v1.jsonl").lines().skip(1).collect();
        let mut parent: Option<String> = None;
        for (n, line) in lines.iter().enumerate() {
            let id = format!("{n:08x}");
            let before = pi(&serde_json::from_str(line).unwrap());
            let after =
                pi(&serde_json::from_str(&migrate_v1(line, &id, parent.as_deref())).unwrap());
            parent = Some(id);
            assert_eq!(after.id(), None, "line {n}");
            assert_eq!(after.parent_id(), None, "line {n}");
            assert_eq!(after.role(), before.role(), "line {n}");
            assert_eq!(after.content(), before.content(), "line {n}");
            assert_eq!(after.usage(), before.usage(), "line {n}");
            assert_eq!(after.turn_id(), before.turn_id(), "line {n}");
            assert_eq!(after.timestamp(), before.timestamp(), "line {n}");
            if before.usage().is_some() {
                assert!(before.turn_id().is_some(), "line {n} has usage but no turn");
            }
        }
    }

    /// An assistant turn whose tool call arguments hold a lone surrogate, as pi writes one
    /// (earendil-works/pi#2551), is still read, usage and all.
    #[rstest]
    #[case::lone_surrogate(
        br#"{"type":"message","id":"a1","parentId":"u1","timestamp":"2026-09-24T02:46:32.649Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"c1","name":"ask_user","arguments":{"question":"ok \udfe1?"}}],"usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0},"stopReason":"toolUse","timestamp":1790217992640}}"#.as_slice(),
        "ok \u{fffd}?"
    )]
    #[case::invalid_utf8(
        b"{\"type\":\"message\",\"id\":\"a1\",\"parentId\":\"u1\",\"timestamp\":\"2026-09-24T02:46:32.649Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"c1\",\"name\":\"ask_user\",\"arguments\":{\"question\":\"ok \xff?\"}}],\"usage\":{\"input\":10,\"output\":5,\"cacheRead\":0,\"cacheWrite\":0},\"stopReason\":\"toolUse\",\"timestamp\":1790217992640}}".as_slice(),
        "ok \u{fffd}?"
    )]
    #[tokio::test]
    async fn a_line_json_parse_reads_is_read(#[case] line: &[u8], #[case] question: &str) {
        assert!(serde_json::from_slice::<serde_json::Value>(line).is_err());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("2026-09-24T02-46-31-698Z_s1.jsonl");
        let mut body = br#"{"type":"session","version":3,"id":"s1","timestamp":"2026-09-24T02:46:31.698Z","cwd":"/w"}"#.to_vec();
        body.push(b'\n');
        body.extend_from_slice(line);
        body.push(b'\n');
        std::fs::write(&path, body).unwrap();

        let session = PiSession::open(SessionId::from("s1".to_owned()), path, pool());
        let got: Vec<PiMessage> = session.read().try_collect().await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].usage(), Some(tokens(10, 5)));
        assert!(matches!(got[1].content().as_slice(), [Content::ToolUse(u)]
            if u.input == serde_json::json!({"question": question})));
    }

    /// A session file that cannot be read when the watcher finds it is read again once it changes,
    /// rather than dropped for the rest of the watch.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn watch_reads_a_file_again_after_a_failed_read() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("2026-09-18T10-00-00-000Z_s1.jsonl");
        std::fs::write(
            &locked,
            serde_json::json!({"type": "session", "id": "0199aaaa"}).to_string() + "\n",
        )
        .unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if File::open(&locked).is_ok() {
            // Permissions do not stop this user (root): nothing to fail.
            return;
        }
        let listener = PiSessions::builder()
            .root(dir.path().to_path_buf())
            .pool(pool())
            .build()
            .listener()
            .unwrap();
        let mut sessions = std::pin::pin!(listener.watch());
        let timeout = std::time::Duration::from_secs(10);

        // Found after the locked file, so read after it: the pool runs one read at a time, in
        // turn.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(500), sessions.next())
                .await
                .is_err()
        );
        std::fs::write(
            dir.path().join("my_notes.jsonl"),
            serde_json::json!({"type": "session", "id": "0199bbbb"}).to_string() + "\n",
        )
        .unwrap();
        let first = tokio::time::timeout(timeout, sessions.next()).await.unwrap().unwrap().unwrap();
        assert_eq!(first.id(), SessionId::from("0199bbbb".to_owned()));

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&locked)
            .unwrap()
            .write_all(b"{\"type\":\"model_change\",\"id\":\"m1\"}\n")
            .unwrap();
        let second =
            tokio::time::timeout(timeout, sessions.next()).await.unwrap().unwrap().unwrap();
        assert_eq!(second.id(), SessionId::from("0199aaaa".to_owned()));
    }
}
