use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use futures::{Stream, StreamExt};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::watch;
use typed_builder::TypedBuilder;

use crate::fs::tree_watcher::{FileStat, TreeWatcher};
use crate::harnesstools::codex::Codex;
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
pub struct CodexSessions {
    #[builder(default, setter(strip_option, into))]
    root: Option<PathBuf>,
    /// Runs every file read of the sessions this finds.
    pool: BlockingPool,
}

impl CodexSessions {
    fn resolve_root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(|| {
            env_nonempty("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home_dir().join(".codex"))
                .join("sessions")
        })
    }
}

/// Where Codex moves the rollouts of a thread the user archives: `archived_sessions/` beside
/// `sessions/` (codex-rs `ARCHIVED_SESSIONS_SUBDIR`, `archive_thread.rs`), flat, each keeping
/// its name. `None` for a root not named `sessions`, which has no such sibling.
fn archive_of(root: &Path) -> Option<PathBuf> {
    (root.file_name()? == "sessions").then(|| root.with_file_name("archived_sessions"))
}

impl Sessions for CodexSessions {
    type Listener = CodexListener;

    fn listener(&self) -> Result<CodexListener, RuntimeError> {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(CodexListener {
            root,
            pool: self.pool.clone(),
        })
    }

    /// The rollouts under `sessions/`, then those of archived threads (`archived_sessions/`
    /// beside it), which Codex no longer lists but which are still the user's history.
    ///
    /// Only `sessions/` is watched ([`CodexListener`]): what Codex appends to an archived
    /// thread's rollout (it may, codex-rs `include_archived`) is read here, or once the thread
    /// is unarchived and its rollout moves back.
    fn existing(
        &self,
    ) -> Result<impl Stream<Item = Result<CodexSession, RuntimeError>> + Send + 'static, RuntimeError>
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
                    let open = |path: &Path, is_file| {
                        CodexListener::open_session(path, is_file, &sessions_pool)
                    };
                    let archive = archive_of(&root).filter(|archive| archive.is_dir());
                    let mut items = scan_sessions(root, open);
                    if let Some(archive) = archive {
                        // A rollout archived mid-scan may have been found in both places.
                        let live: HashSet<SessionId> =
                            items.iter().flatten().map(|session| session.id.clone()).collect();
                        items.extend(
                            scan_sessions(archive, open)
                                .into_iter()
                                .filter(|item| !item.as_ref().is_ok_and(|s| live.contains(&s.id))),
                        );
                    }
                    items
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

impl Observable for Codex {
    type Sessions = CodexSessions;

    fn sessions(&self, pool: BlockingPool) -> CodexSessions {
        CodexSessions::builder().pool(pool).build()
    }
}

/// The session a rollout file holds, from its name (`rollout-<timestamp>-<id>.jsonl`, the
/// timestamp `YYYY-MM-DD` in the earliest rollouts and `YYYY-MM-DDThh-mm-ss` since).
///
/// The id is the thread's, except for a rollout a thread was reverted into (codex-rs
/// `rollout_file_name.rs`: `rollout-<timestamp>-<thread>_<rollout>.jsonl`), which holds only the
/// thread's history since the revert and names the rollout before it as its `history_base`.
/// Such a file keeps both ids as its session, so it never shares one with the thread's earlier
/// rollout (see [`thread_of`]).
fn session_id_of(stem: &str) -> SessionId {
    const UUID_LEN: usize = 36;
    if let Some((head, rollout)) = stem.rsplit_once('_')
        && let Some(thread) = head.len().checked_sub(UUID_LEN).and_then(|at| head.get(at..))
    {
        return SessionId::from(format!("{thread}_{rollout}"));
    }
    let mut groups: Vec<&str> = stem.rsplitn(6, '-').collect();
    groups.truncate(5);
    groups.reverse();
    SessionId::from(groups.join("-"))
}

/// The thread a session belongs to: its id, or for a reverted thread's rollout the thread
/// part of it (see [`session_id_of`]).
fn thread_of(session: &SessionId) -> &str {
    let id: &str = session.as_ref();
    id.split_once('_').map_or(id, |(thread, _)| thread)
}

#[derive(Debug, Clone)]
pub struct CodexListener {
    root: PathBuf,
    pool: BlockingPool,
}

impl CodexListener {
    /// The session a codex rollout file holds (see [`session_id_of`]), or `None` if `path` is
    /// not one.
    fn session_id(path: &Path) -> Option<SessionId> {
        let name = path.file_name()?.to_string_lossy();
        if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
            return None;
        }
        Some(session_id_of(&path.file_stem()?.to_string_lossy()))
    }

    /// Build a read-once session for an accepted codex rollout file (no change signal), or `None`.
    fn open_session(path: &Path, is_file: bool, pool: &BlockingPool) -> Option<CodexSession> {
        if !is_file {
            return None;
        }
        Some(CodexSession::open(Self::session_id(path)?, path.to_path_buf(), pool.clone()))
    }
}

impl Listener for CodexListener {
    type Session = CodexSession;

    fn watch(self) -> impl Stream<Item = Result<CodexSession, WatchError>> + Send + 'static {
        let root = self.root;
        let pool = self.pool;
        async_stream::stream! {
            let files = TreeWatcher::builder()
                .filter(|path| Self::session_id(path).is_some())
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
                yield Ok(CodexSession {
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
pub struct CodexSession {
    id: SessionId,
    path: PathBuf,
    /// Wakes [`messages`](Session::messages) on each change to the file; `None` reads it once.
    changes: Option<watch::Receiver<FileStat>>,
    pool: BlockingPool,
}

impl CodexSession {
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

    /// The rollout's lines from byte `start`: followed on each change while `changes` lives, or
    /// read once to the end without it.
    fn lines(
        path: &Path,
        start: u64,
        changes: Option<watch::Receiver<FileStat>>,
        pool: BlockingPool,
    ) -> impl Stream<Item = io::Result<Line>> + Send + 'static {
        let lines = FollowLines::new(PooledReadLines::new(PathLineReader::at(path, start), pool));
        match changes {
            Some(changes) => lines.follow(changes).left_stream(),
            None => lines.read_to_end().right_stream(),
        }
    }

    fn stamper(&self, start: u64) -> Stamper {
        Stamper {
            session: self.id.clone(),
            path: self.path.clone(),
            pool: self.pool.clone(),
            before: start,
            usage_recorded: (start == 0).then_some(false),
            history_start: (start == 0).then_some(None),
        }
    }
}

impl Session for CodexSession {
    type Message = CodexMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages_from(
        self,
        from: Option<Checkpoint>,
    ) -> impl Stream<Item = Result<(Checkpoint, CodexMessage), MessageError>> + Send + 'static {
        async_stream::stream! {
            let start = match from {
                Some(from) => self.start(from).await,
                None => 0,
            };
            let mut stamper = self.stamper(start);
            let messages =
                Self::lines(&self.path, start, self.changes, self.pool).json::<CodexMessage>();
            for await item in messages {
                yield match item {
                    Ok((line, mut message)) => {
                        stamper.stamp(&line, &mut message).await;
                        Ok((Checkpoint::new(line.end, &line.bytes), message))
                    }
                    Err(err) => Err(MessageError::from(err)),
                };
            }
        }
    }

    fn read(&self) -> impl Stream<Item = Result<CodexMessage, MessageError>> + Send + 'static {
        let mut stamper = self.stamper(0);
        let messages = Self::lines(&self.path, 0, None, self.pool.clone()).json::<CodexMessage>();
        async_stream::stream! {
            for await item in messages {
                yield match item {
                    Ok((line, mut message)) => {
                        stamper.stamp(&line, &mut message).await;
                        Ok(message)
                    }
                    Err(err) => Err(MessageError::from(err)),
                };
            }
        }
    }
}

/// Tells each line of one rollout what only its reader knows (see [`LineContext`]).
struct Stamper {
    session: SessionId,
    path: PathBuf,
    pool: BlockingPool,
    /// Where reading started: lines before it were never seen, so whether one of them was a
    /// `token_usage_record` is looked up in the file, once, when a `token_count` needs to know.
    before: u64,
    /// Whether this rollout records usage per model call by now: a line so far was a
    /// `token_usage_record`, or the Codex that created it writes them (see [`records_usage`]);
    /// `None` until known.
    usage_recorded: Option<bool>,
    /// The ordinal this rollout's own history starts at, when it begins with history inherited
    /// from its parent (its `session_meta.subagent_history_start_ordinal`); `None` until known.
    history_start: Option<Option<u64>>,
}

impl Stamper {
    async fn stamp(&mut self, line: &Line, message: &mut CodexMessage) {
        // The rollout's first line: reading began at its start, or restarted there after the
        // file was truncated or replaced, so nothing was skipped and nothing before it counts.
        if line.end == line.bytes.len() as u64 + 1 {
            self.before = 0;
            self.usage_recorded = Some(false);
            self.history_start = Some(None);
        }
        if message.kind == TOKEN_USAGE_RECORD {
            self.usage_recorded = Some(true);
        } else if self.usage_recorded.is_none() && message.is_token_count() {
            let (path, before) = (self.path.clone(), self.before);
            let session = self.session.clone();
            let found = self
                .pool
                .run(move || {
                    first_own_meta(&path, &session).as_ref().is_some_and(records_usage)
                        || prefix_contains(&path, before, TOKEN_USAGE_RECORD_MARKER)
                            .unwrap_or(false)
                })
                .await
                .unwrap_or(false);
            self.usage_recorded = Some(found);
        }
        message.context = LineContext {
            session: Some(self.session.clone()),
            usage_recorded: self.usage_recorded == Some(true),
        };
        if let Some(meta) = message.own_meta() {
            self.history_start = Some(history_start_of(meta));
            if records_usage(meta) {
                self.usage_recorded = Some(true);
            }
            return;
        }
        let Some(ordinal) = message.ordinal else {
            return;
        };
        if self.history_start.is_none() {
            // Read past its start: the history start is on the first line, never seen here.
            let path = self.path.clone();
            let session = self.session.clone();
            let found = self.pool.run(move || first_own_meta(&path, &session)).await;
            self.history_start = Some(found.ok().flatten().as_ref().and_then(history_start_of));
        }
        if self.history_start.flatten().is_some_and(|start| ordinal < start) {
            message.inherit();
        }
    }
}

/// Where a rollout's own history starts after the lines it inherited (codex-rs
/// `SessionMeta::subagent_history_start_ordinal`, set on a subagent forked with its parent's
/// context: "earlier rollout records are inherited model context and stay out of child
/// turn/item projection").
fn history_start_of(meta: &serde_json::Value) -> Option<u64> {
    meta["subagent_history_start_ordinal"].as_u64()
}

/// Whether the Codex that created a rollout (its own `session_meta.cli_version`) records usage
/// per model call: every call it makes then has a `token_usage_record` (codex-rs 5f79a92e39,
/// first released in 0.153.0), and each `token_count` only repeats one.
///
/// Known up front, because such a rollout can begin with `token_count` lines and no record: a
/// subagent forked from a thread in legacy history mode starts with a copy of the parent's
/// history, from which Codex drops the records but keeps the snapshots (codex-rs
/// `agent/control/spawn.rs`, `keep_forked_rollout_item`). Those snapshots are the parent's
/// calls, which the parent's own records already count.
///
/// A development build (`0.0.0`) or an unreadable version is taken for an older Codex.
fn records_usage(meta: &serde_json::Value) -> bool {
    meta["cli_version"].as_str().is_some_and(|version| {
        let mut parts = version.split(['.', '-']).map(str::parse::<u64>);
        matches!(
            (parts.next(), parts.next()),
            (Some(Ok(major)), Some(Ok(minor))) if (major, minor) >= (0, 153)
        )
    })
}

/// The own `session_meta` payload of the rollout at `path`, which is its first line.
fn first_own_meta(path: &Path, session: &SessionId) -> Option<serde_json::Value> {
    let mut first = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(std::fs::File::open(path).ok()?),
        &mut first,
    )
    .ok()?;
    let mut line: CodexMessage = serde_json::from_str(&first).ok()?;
    line.context.session = Some(session.clone());
    line.own_meta().cloned()
}

/// Whether the first `end` bytes of the file at `path` contain `needle`, read in chunks so a
/// long rollout is never held in memory whole.
fn prefix_contains(path: &Path, end: u64, needle: &[u8]) -> std::io::Result<bool> {
    const CHUNK: usize = 64 * 1024;
    let mut file = std::fs::File::open(path)?.take(end);
    let finder = memchr::memmem::Finder::new(needle);
    let keep = needle.len().saturating_sub(1);
    let mut window: Vec<u8> = Vec::with_capacity(CHUNK + keep);
    let mut chunk = vec![0; CHUNK];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            return Ok(false);
        }
        window.extend_from_slice(&chunk[..read]);
        if finder.find(&window).is_some() {
            return Ok(true);
        }
        // Keep a needle's length less one, so a match straddling two chunks is still found.
        window.drain(..window.len().saturating_sub(keep));
    }
}

const TOKEN_USAGE_RECORD: &str = "token_usage_record";

/// How a `token_usage_record` line names its type. Unescaped quotes cannot occur inside a JSON
/// string, so a transcript merely quoting the name does not match.
const TOKEN_USAGE_RECORD_MARKER: &[u8] = b"\"type\":\"token_usage_record\"";

/// What a line cannot tell about itself, which the reader of its rollout knows.
#[derive(Debug, Clone, Default)]
struct LineContext {
    /// The rollout's own session. A `session_meta` naming another was copied in from the
    /// rollout a fork or subagent started from (codex-rs `ForkPersistence::Copied`).
    session: Option<SessionId>,
    /// An earlier line was a `token_usage_record`, or the rollout was created by a Codex that
    /// writes them (see [`records_usage`]): this rollout records usage per model call, so its
    /// `token_count` lines only repeat it.
    usage_recorded: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(from = "RawLine")]
pub struct CodexMessage {
    kind: String,
    timestamp: Option<String>,
    /// The line's position in its rollout (codex-rs `RolloutLine.ordinal`), in rollouts written
    /// since ordinals were introduced.
    ordinal: Option<u64>,
    payload: Option<serde_json::Value>,
    context: LineContext,
}

/// The kind a line takes once it is known to hold nothing of this session's own: a line a
/// subagent inherited from its parent, or the output of a compaction call (both carried again
/// elsewhere). Every accessor finds nothing in it.
const HIDDEN: &str = "hidden";

/// One rollout line as written, in any of the shapes Codex has written:
/// - `{"timestamp", "type", "payload"}` (codex-rs `RolloutLine`, since 0.32.0), with an
///   `ordinal` in newer rollouts and a `metadata` beside a `response_item` payload
///   (`CodexHarnessMetadata`);
/// - before 0.32.0 (codex-rs 43809a454e, "Introduce rollout items"), a first line holding the
///   session meta itself (`{"id", "timestamp", "instructions", "git"}`), then each response item
///   bare (`{"type": "message", ...}`), with `{"record_type": "state"}` snapshots between.
#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    ordinal: Option<u64>,
    payload: Option<serde_json::Value>,
    metadata: Option<serde_json::Value>,
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

impl From<RawLine> for CodexMessage {
    fn from(raw: RawLine) -> Self {
        let RawLine {
            kind,
            timestamp,
            ordinal,
            payload,
            metadata,
            mut rest,
        } = raw;
        let (kind, payload) = match (kind, payload) {
            // Output Codex generated for a compaction, not a message to the user (codex-rs
            // `compact.rs`); the `compacted` line after it carries the summary.
            (Some(_), Some(_))
                if metadata
                    .as_ref()
                    .is_some_and(|m| m["compaction_output"].as_bool() == Some(true)) =>
            {
                (HIDDEN.to_owned(), None)
            }
            (Some(kind), Some(payload)) => (kind, Some(payload)),
            (Some(kind), None) if rest.is_empty() => (kind, None),
            (Some(kind), None) => {
                rest.insert("type".to_owned(), serde_json::Value::String(kind));
                ("response_item".to_owned(), Some(serde_json::Value::Object(rest)))
            }
            (None, _) if rest.contains_key("record_type") => ("state".to_owned(), None),
            (None, _) if rest.contains_key("id") => {
                if let Some(ts) = &timestamp {
                    rest.insert("timestamp".to_owned(), serde_json::Value::String(ts.clone()));
                }
                ("session_meta".to_owned(), Some(serde_json::Value::Object(rest)))
            }
            (None, payload) => (String::new(), payload),
        };
        Self {
            kind,
            timestamp,
            ordinal,
            payload,
            context: LineContext::default(),
        }
    }
}

/// Whether a command output reports a non-zero exit, in any shape Codex writes one:
/// - a JSON envelope with `exit_code`, top-level or (older rollouts) under `metadata`;
/// - a text header ahead of the `Output:` line (codex-rs `tools/mod.rs`
///   `format_exec_output_for_model`: `Exit code: N`; `tools/context.rs` `response_header`:
///   `Process exited with code N`).
///
/// Only the header is read, so a command whose output quotes such a line cannot flip it.
fn codex_output_failed(output: &serde_json::Value) -> bool {
    let texts: Vec<&str> = match output {
        serde_json::Value::String(s) => vec![s],
        serde_json::Value::Array(blocks) => {
            blocks.iter().filter_map(|b| b["text"].as_str()).collect()
        }
        _ => Vec::new(),
    };
    texts.iter().any(|text| exit_code(text).is_some_and(|code| code != 0))
}

/// The exit code one command output reports, if it reports one.
fn exit_code(text: &str) -> Option<i64> {
    if let Ok(serde_json::Value::Object(envelope)) = serde_json::from_str(text) {
        return envelope
            .get("exit_code")
            .or_else(|| envelope.get("metadata").and_then(|m| m.get("exit_code")))
            .and_then(serde_json::Value::as_i64);
    }
    text.lines().take_while(|line| line.trim_end() != "Output:").find_map(|line| {
        ["Exit code: ", "Process exited with code "]
            .iter()
            .find_map(|marker| line.strip_prefix(marker))
            .and_then(|code| code.trim().parse().ok())
    })
}

/// Start and end markers of the fragments Codex injects into the conversation as `user`
/// messages, per codex-rs `core/src/context/contextual_user_message.rs`
/// (`CONTEXTUAL_USER_FRAGMENT_MATCHERS`) and the frozen legacy list in
/// `thread-store/src/local/rollout_migration/rollback.rs` (`is_known_contextual_user_text`).
const CONTEXTUAL_USER_MARKERS: &[(&str, &str)] = &[
    ("# AGENTS.md instructions", "</INSTRUCTIONS>"),
    // Older rollouts wrapped AGENTS.md in `USER_INSTRUCTIONS_OPEN_TAG`.
    ("<user_instructions>", "</user_instructions>"),
    ("<environment_context>", "</environment_context>"),
    ("<agent_message_board_notification>", "</agent_message_board_notification>"),
    ("<skill>", "</skill>"),
    ("<user_shell_command>", "</user_shell_command>"),
    ("<turn_aborted>", "</turn_aborted>"),
    ("<subagent_notification>", "</subagent_notification>"),
    ("<codex_internal_context", "</codex_internal_context>"),
    ("<goal_context>", "</goal_context>"),
    ("<recommended_plugins>", "</recommended_plugins>"),
    // codex-rs `parse_hook_prompt_fragment`: `<hook_prompt hook_run_id="...">`.
    ("<hook_prompt ", "</hook_prompt>"),
    // A review's findings handed back to the thread that asked for it; Codex never counts it as
    // user input (codex-rs `history_user_authorization.rs`, `legacy_user_messages`).
    ("<user_action>", "</user_action>"),
];

/// Warnings Codex once injected as `user` messages (the `Legacy*Warning` fragments).
const CONTEXTUAL_USER_PREFIXES: &[&str] = &[
    "Warning: The maximum number of unified exec processes you can keep open is",
    "Warning: apply_patch was requested via ",
    "Warning: Your account was flagged for potentially high-risk cyber activity",
];

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn ends_with_ignore_case(text: &str, suffix: &str) -> bool {
    text.len() >= suffix.len()
        && text
            .get(text.len() - suffix.len()..)
            .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
}

/// Whether `text` is a fragment of harness-injected context rather than something the user
/// typed (codex-rs `is_contextual_user_fragment`).
fn is_contextual_user_text(text: &str) -> bool {
    let text = text.trim();
    CONTEXTUAL_USER_MARKERS.iter().any(|(start, end)| {
        starts_with_ignore_case(text, start) && ends_with_ignore_case(text, end)
    }) || CONTEXTUAL_USER_PREFIXES.iter().any(|prefix| text.starts_with(prefix))
        || text
            .strip_prefix("<external_")
            .and_then(|rest| rest.split_once('>'))
            .is_some_and(|(key, _)| text.ends_with(&format!("</external_{key}>")))
}

/// The command a user ran from Codex's prompt, when `text` is the fragment Codex records for it
/// (codex-rs `context/user_shell_command.rs`: `<user_shell_command>` wrapping `<command>` and a
/// `<result>` holding the output). The command is what the user typed; the result is execution
/// output, which is not kept.
fn user_shell_command(text: &str) -> Option<&str> {
    let body =
        text.trim().strip_prefix("<user_shell_command>")?.strip_suffix("</user_shell_command>")?;
    let (_, rest) = body.split_once("<command>")?;
    let (command, _) = rest.split_once("</command>")?;
    Some(command.trim())
}

/// Response items that hold nothing readable (codex-rs `ResponseItem`): an encrypted compaction
/// checkpoint (`compaction`, once `compaction_summary`, and `context_compaction`), a reasoning
/// configuration change, and items older rollouts kept (`ghost_snapshot`, a git snapshot for
/// undo) or never should have.
fn is_opaque_item(kind: &str) -> bool {
    matches!(
        kind,
        "compaction"
            | "compaction_summary"
            | "context_compaction"
            | "compaction_trigger"
            | "configuration_update"
            | "additional_tools"
            | "ghost_snapshot"
            | "other"
    )
}

/// What Codex puts ahead of a compaction summary it hands the next model call (codex-rs
/// `prompts/templates/compact/summary_prefix.md`, `SUMMARY_PREFIX`): the summary is what follows.
const SUMMARY_PREFIX: &str = "Another language model started to solve this problem and produced a \
                              summary of its thinking process.";
const SUMMARY_PREFIX_END: &str =
    "use the information in this summary to assist with your own analysis:";

/// The summary a `compacted` line's `message` holds, without the preamble Codex wraps it in.
fn compaction_summary(message: &str) -> &str {
    message
        .strip_prefix(SUMMARY_PREFIX)
        .and_then(|rest| rest.split_once(SUMMARY_PREFIX_END))
        .map_or(message, |(_, summary)| summary)
        .trim()
}

/// A stable key for one `TokenUsage` object, for lines that carry no id of their own.
fn usage_key(usage: &serde_json::Value) -> Option<String> {
    let field = |name: &str| usage.get(name).and_then(serde_json::Value::as_u64).unwrap_or(0);
    usage.is_object().then(|| {
        format!(
            "{}.{}.{}.{}.{}",
            field("input_tokens"),
            field("cached_input_tokens"),
            field("output_tokens"),
            field("reasoning_output_tokens"),
            field("total_tokens"),
        )
    })
}

/// Our [`Usage`] for a Codex `TokenUsage`. Codex (and the Responses API) count cached input
/// and cache writes inside `input_tokens`, as its `input_tokens_details` breakdown (codex-rs
/// `sse/responses.rs`, `parses_cache_write_token_usage`: 100 input of which 40 cached and 60
/// written), where [`Usage::input`] is the input that is neither. `output_tokens` already
/// includes `reasoning_output_tokens`.
fn usage_of(usage: &serde_json::Value) -> Option<Usage> {
    if !usage.is_object() {
        return None;
    }
    let field = |name: &str| usage.get(name).and_then(serde_json::Value::as_u64);
    let cache_read = field("cached_input_tokens");
    let cache_write = field("cache_write_input_tokens");
    Some(Usage {
        input: field("input_tokens").map(|input| {
            input.saturating_sub(cache_read.unwrap_or(0)).saturating_sub(cache_write.unwrap_or(0))
        }),
        output: field("output_tokens"),
        cache_read,
        cache_write,
        reasoning: field("reasoning_output_tokens"),
    })
}

impl CodexMessage {
    fn block(value: &serde_json::Value) -> Option<Content> {
        match value["type"].as_str() {
            Some("input_text" | "output_text" | "text") => {
                let text = value["text"].as_str().unwrap_or_default();
                // Recorded the way Claude Code replays one of its `!` commands.
                Some(user_shell_command(text).map_or_else(
                    || Content::Text(text.to_owned()),
                    |cmd| Content::Text(format!("! {cmd}")),
                ))
            }
            // Ciphertext only the model can read (an agent message's payload).
            Some("encrypted_content") => None,
            _ => Some(Content::Other(value.clone())),
        }
    }

    fn payload_type(&self) -> Option<&str> {
        self.payload.as_ref()?["type"].as_str()
    }

    fn is_event(&self, kind: &str) -> bool {
        self.kind == "event_msg" && self.payload_type() == Some(kind)
    }

    fn is_token_count(&self) -> bool {
        self.is_event("token_count")
    }

    /// A `session_meta` another rollout's: one a fork or subagent copied in from its parent,
    /// which describes the parent, not this session.
    fn is_copied_meta(&self) -> bool {
        self.kind == "session_meta"
            && self.context.session.as_ref().is_some_and(|own| {
                self.payload
                    .as_ref()
                    .and_then(|p| p["id"].as_str())
                    .is_some_and(|id| id != thread_of(own))
            })
    }

    /// Mark this line as history this rollout inherited from its parent (see [`HIDDEN`]).
    fn inherit(&mut self) {
        HIDDEN.clone_into(&mut self.kind);
        self.payload = None;
    }

    /// Whether this line is conversation or its bookkeeping: an item of model history
    /// (`response_item`, or before 0.32.0 a bare item), a compaction, or a message between agents.
    /// Everything else (session, turn and world state, usage, events) names no item.
    fn is_item(&self) -> bool {
        !matches!(
            self.kind.as_str(),
            "event_msg"
                | "session_meta"
                | "turn_context"
                | "world_state"
                | "retained_context"
                | "security_risk_score"
                | "realtime_item"
                | "inter_agent_communication_metadata"
                | "compacted"
                | "state"
                | HIDDEN
                | TOKEN_USAGE_RECORD
        )
    }

    /// This rollout's own `session_meta` payload.
    fn own_meta(&self) -> Option<&serde_json::Value> {
        (self.kind == "session_meta" && !self.is_copied_meta()).then_some(self.payload.as_ref()?)
    }

    /// The `info` of a `token_count` that reports usage of its own: only in a rollout that
    /// records none per model call, and not for a snapshot of no new tokens (a rate-limit
    /// refresh, an estimate after compaction, the window filled after an overflow).
    fn token_count_info(&self) -> Option<&serde_json::Value> {
        if !self.is_token_count() || self.context.usage_recorded {
            return None;
        }
        let info = self.payload.as_ref()?.get("info").filter(|info| !info.is_null())?;
        let last = info.get("last_token_usage")?;
        let tokens = |name: &str| last.get(name).and_then(serde_json::Value::as_u64).unwrap_or(0);
        (tokens("input_tokens") + tokens("output_tokens") > 0).then_some(info)
    }

    /// The model call a usage line accounts for. A `token_usage_record` names its response.
    /// A `token_count` names none, so it is keyed on the thread's running total, which is
    /// unique per call and survives being copied into a fork (whose copies are re-stamped with
    /// the child's timestamps), and which repeats verbatim when Codex re-sends a snapshot.
    fn usage_turn(&self) -> Option<String> {
        if self.kind == TOKEN_USAGE_RECORD {
            let payload = self.payload.as_ref()?;
            return match payload["response_id"].as_str().filter(|id| !id.is_empty()) {
                Some(response) => Some(response.to_owned()),
                None => {
                    usage_key(&payload["thread_token_usage"]).map(|key| format!("thread:{key}"))
                }
            };
        }
        let info = self.token_count_info()?;
        match usage_key(&info["total_token_usage"]) {
            Some(total) => Some(format!("token_count:{total}")),
            None => Some(format!(
                "token_count:{}:{}",
                self.timestamp.as_deref().unwrap_or_default(),
                usage_key(&info["last_token_usage"])?
            )),
        }
    }

    /// Why the turn this event ends failed or stopped, when it did.
    fn failure(&self) -> Option<(StopReason, String)> {
        let payload = self.payload.as_ref()?;
        if self.is_event("turn_aborted") {
            let reason = payload["reason"].as_str().unwrap_or("aborted");
            return Some((StopReason::Aborted, reason.to_owned()));
        }
        // Written as `task_complete` (codex-rs `EventMsg::TurnComplete`,
        // `#[serde(rename = "task_complete", alias = "turn_complete")]`).
        if self.is_event("task_complete") || self.is_event("turn_complete") {
            let error = payload.get("error").filter(|e| !e.is_null())?;
            let message =
                error["message"].as_str().map_or_else(|| error.to_string(), str::to_owned);
            return Some((StopReason::Error, message));
        }
        None
    }

    /// Whether this `user` message is context the harness injected (AGENTS.md, the environment,
    /// skills, notifications) rather than a user turn: any of its texts is a contextual fragment
    /// (codex-rs `event_mapping::is_contextual_user_message_content`).
    fn is_contextual(&self) -> bool {
        self.payload.as_ref().and_then(|p| p["content"].as_array()).is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block["type"] == "input_text"
                    && block["text"].as_str().is_some_and(|t| {
                        is_contextual_user_text(t) && user_shell_command(t).is_none()
                    })
            })
        })
    }
}

impl Message for CodexMessage {
    fn id(&self) -> Option<MessageId> {
        // Usage lines carry no id, but name the model call they account for; keyed on nothing,
        // two in one millisecond would share a synthetic id and one would be dropped.
        if self.kind == TOKEN_USAGE_RECORD || self.is_token_count() {
            return self.usage_turn().map(|turn| MessageId::from(format!("{turn}#usage")));
        }
        if self.kind == "session_meta" {
            return self.own_meta()?["id"].as_str().map(|id| MessageId::from(id.to_owned()));
        }
        // Events carry no item id; some name a tool call (`patch_apply_end`, `mcp_tool_call_end`),
        // an id the call's own item already holds. Items that are no conversation (a compaction's
        // opaque checkpoint, a configuration update) would only keep an empty row.
        if !self.is_item() || self.payload_type().is_some_and(is_opaque_item) {
            return None;
        }
        // Prefer the per-record `id` (ctc_/ctco_/msg_...) over `call_id`: a tool call and its
        // output share one `call_id`, so keying identity on it would collide the two records and
        // the dedup gate would drop the output. Older rollouts have no per-record id at all, so
        // an output falling back to `call_id` is suffixed to keep it distinct from its call.
        // `call_id` linkage lives in the content, not here.
        let p = self.payload.as_ref()?;
        if let Some(id) = p["id"].as_str() {
            return Some(MessageId::from(id.to_owned()));
        }
        let call_id = p["call_id"].as_str()?;
        let is_output = p["type"].as_str().is_some_and(|t| t.ends_with("_output"));
        Some(MessageId::from(if is_output {
            format!("{call_id}#out")
        } else {
            call_id.to_owned()
        }))
    }

    fn role(&self) -> Role {
        if self.kind == "compacted" {
            return Role::System;
        }
        if self.failure().is_some() {
            return Role::Assistant;
        }
        // Messages between agents (codex-rs `ResponseItem::AgentMessage`, `RolloutItem::
        // InterAgentCommunication`): the harness delivered them, no user typed them.
        if self.kind == "inter_agent_communication" {
            return Role::System;
        }
        let payload = self.payload.as_ref();
        match self.payload_type() {
            Some(
                "function_call"
                | "custom_tool_call"
                | "local_shell_call"
                | "web_search_call"
                | "tool_search_call"
                | "image_generation_call"
                | "reasoning",
            ) => Role::Assistant,
            Some("agent_message") => Role::System,
            Some(kind) if self.is_item() && is_opaque_item(kind) => Role::System,
            Some("function_call_output" | "custom_tool_call_output" | "tool_search_output") => {
                Role::Tool
            }
            _ => match payload.and_then(|p| p["role"].as_str()).unwrap_or(self.kind.as_str()) {
                "user" if self.is_contextual() => Role::System,
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "system" | "developer" => Role::System,
                "tool" => Role::Tool,
                other => Role::Other(other.to_owned()),
            },
        }
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.timestamp.as_deref().and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
    }

    fn content(&self) -> Vec<Content> {
        let Some(payload) = self.payload.as_ref() else {
            return Vec::new();
        };
        if self.kind == "compacted" {
            return payload["message"]
                .as_str()
                .map(compaction_summary)
                .filter(|summary| !summary.is_empty())
                .map(|summary| vec![Content::Summary(summary.to_owned())])
                .unwrap_or_default();
        }
        if let Some((_, why)) = self.failure() {
            return vec![Content::Error(why)];
        }
        if !self.is_item() {
            return Vec::new();
        }
        if self.kind == "inter_agent_communication" {
            return payload["content"]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(|text| vec![Content::Text(text.to_owned())])
                .unwrap_or_default();
        }
        let call_id =
            |key: &str| ToolCallId::from(payload[key].as_str().unwrap_or_default().to_owned());
        let tool_use = |id: ToolCallId, name: &str, input: &serde_json::Value| {
            vec![Content::ToolUse(ToolUse {
                id,
                name: name.to_owned(),
                input: input.clone(),
            })]
        };
        match payload["type"].as_str() {
            // Do not retain summaries, encrypted reasoning, or other reasoning payloads.
            Some("reasoning") => vec![Content::ReasoningSummary { tokens: None }],
            Some("function_call") => tool_use(
                call_id("call_id"),
                payload["name"].as_str().unwrap_or_default(),
                &payload["arguments"],
            ),
            Some("custom_tool_call") => tool_use(
                call_id("call_id"),
                payload["name"].as_str().unwrap_or_default(),
                &payload["input"],
            ),
            // Its output comes back as a `function_call_output` under the same `call_id`.
            Some("local_shell_call") => tool_use(
                call_id(if payload["call_id"].is_string() {
                    "call_id"
                } else {
                    "id"
                }),
                "local_shell",
                &payload["action"],
            ),
            // Run by the API: the results feed the model directly and no output item follows.
            Some("web_search_call") => tool_use(call_id("id"), "web_search", &payload["action"]),
            // Also run by the API; its `result` is the image itself, which is not kept.
            Some("image_generation_call") => {
                tool_use(call_id("id"), "image_generation", &payload["revised_prompt"])
            }
            Some(kind) if is_opaque_item(kind) => Vec::new(),
            Some("tool_search_call") => tool_use(
                call_id(if payload["call_id"].is_string() {
                    "call_id"
                } else {
                    "id"
                }),
                "tool_search",
                &payload["arguments"],
            ),
            Some("function_call_output" | "custom_tool_call_output") => {
                vec![Content::ToolResult(ToolResult {
                    call: call_id("call_id"),
                    output: payload["output"].clone(),
                    error: codex_output_failed(&payload["output"]),
                })]
            }
            Some("tool_search_output") => vec![Content::ToolResult(ToolResult {
                call: call_id(if payload["call_id"].is_string() {
                    "call_id"
                } else {
                    "id"
                }),
                output: payload["tools"].clone(),
                error: payload["status"].as_str().is_some_and(|s| s != "completed"),
            })],
            _ => match &payload["content"] {
                serde_json::Value::Array(blocks) => blocks.iter().filter_map(Self::block).collect(),
                serde_json::Value::String(text) => vec![Content::Text(text.clone())],
                _ => Vec::new(),
            },
        }
    }

    fn model(&self) -> Option<String> {
        if self.is_copied_meta() {
            return None;
        }
        self.payload.as_ref()?.get("model")?.as_str().map(str::to_owned)
    }

    fn usage(&self) -> Option<Usage> {
        if self.kind == TOKEN_USAGE_RECORD {
            return usage_of(self.payload.as_ref()?.get("usage")?);
        }
        usage_of(&self.token_count_info()?["last_token_usage"])
    }

    fn stop_reason(&self) -> Option<StopReason> {
        self.failure().map(|(reason, _)| reason)
    }

    fn cwd(&self) -> Option<PathBuf> {
        if self.is_copied_meta() {
            return None;
        }
        self.payload.as_ref()?.get("cwd")?.as_str().map(PathBuf::from)
    }

    fn git_branch(&self) -> Option<String> {
        self.own_meta()?["git"]["branch"].as_str().map(str::to_owned)
    }

    /// The thread a fork or subagent started from (codex-rs `SessionMeta::forked_from_id`,
    /// `parent_thread_id`, and before those `source.subagent.thread_spawn.parent_thread_id`).
    ///
    /// A rollout a thread was reverted into continues the rollout its `history_base` names
    /// (codex-rs `revert_thread.rs`), which is its parent here (see `session_id_of`).
    fn parent_session(&self) -> Option<SessionId> {
        let meta = self.own_meta()?;
        if let Some(own) = self.context.session.as_ref()
            && thread_of(own) != own.as_ref()
            && let Some(base) = meta["history_base"]["thread_id"].as_str()
        {
            let thread = thread_of(own);
            return Some(SessionId::from(if base == thread {
                thread.to_owned()
            } else {
                format!("{thread}_{base}")
            }));
        }
        [
            &meta["forked_from_id"],
            &meta["parent_thread_id"],
            &meta["source"]["subagent"]["thread_spawn"]["parent_thread_id"],
        ]
        .into_iter()
        .find_map(serde_json::Value::as_str)
        .map(|parent| SessionId::from(parent.to_owned()))
    }

    /// Only usage lines name their model call (see `usage_turn`): Codex items carry no
    /// response id.
    fn turn_id(&self) -> Option<String> {
        self.usage_turn()
    }

    /// A name given to the thread, as rollouts recorded it from 0.93.0 until names moved to
    /// `session_index.jsonl` (codex-rs `EventMsg::ThreadNameUpdated`, retired in 2c1a361a2e and
    /// since skipped by the rollout migration's `should_skip_retired_record`). A subagent's events
    /// can land in its parent's rollout, so only a name for this rollout's own thread counts.
    fn title(&self) -> Option<String> {
        if !self.is_event("thread_name_updated") {
            return None;
        }
        let payload = self.payload.as_ref()?;
        let own = self.context.session.as_ref().map(thread_of);
        if let (Some(thread), Some(own)) = (payload["thread_id"].as_str(), own)
            && thread != own
        {
            return None;
        }
        payload["thread_name"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
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
    fn normalizes_a_codex_assistant_message() {
        let raw = serde_json::json!({
            "timestamp": "2026-09-18T10:00:00Z",
            "type": "message",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "done"}],
            },
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.role(), Role::Assistant);
        assert_eq!(m.content(), vec![Content::Text("done".into())]);
    }

    #[rstest]
    fn turn_context_exposes_model_and_cwd() {
        let raw = serde_json::json!({
            "type": "turn_context",
            "payload": {"model": "gpt-5.6-terra", "effort": "medium", "cwd": "/work/atuin"},
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.model(), Some("gpt-5.6-terra".to_owned()));
        assert_eq!(m.cwd(), Some(PathBuf::from("/work/atuin")));
    }

    #[rstest]
    fn token_usage_record_exposes_usage() {
        let raw = serde_json::json!({
            "type": "token_usage_record",
            "payload": {
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 50,
                    "cached_input_tokens": 20,
                    "cache_write_input_tokens": 0,
                },
            },
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            m.usage(),
            Some(Usage {
                input: Some(80),
                output: Some(50),
                cache_read: Some(20),
                cache_write: Some(0),
                reasoning: None,
            })
        );
    }

    #[rstest]
    fn enrichment_is_none_when_the_harness_did_not_provide_it() {
        let raw = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "hi"}]},
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&raw).unwrap();
        assert_eq!(m.model(), None);
        assert_eq!(m.usage(), None);
        assert_eq!(m.stop_reason(), None);
        assert_eq!(m.git_branch(), None);
    }

    #[rstest]
    fn normalizes_a_codex_function_call() {
        let raw = serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "function_call",
                "name": "shell",
                "arguments": "{\"cmd\":\"ls\"}",
                "call_id": "c1",
            },
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&raw).unwrap();
        assert!(matches!(m.content().as_slice(), [Content::ToolUse(_)]));
    }

    #[rstest]
    fn normalizes_a_codex_custom_tool_call() {
        let call = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "custom_tool_call", "name": "shell", "input": "ls", "call_id": "c1"},
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&call).unwrap();
        assert_eq!(m.role(), Role::Assistant);
        assert!(
            matches!(m.content().as_slice(), [Content::ToolUse(u)] if u.id.to_string() == "c1")
        );

        let output = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "custom_tool_call_output", "call_id": "c1", "output": "files"},
        })
        .to_string();
        let m: CodexMessage = serde_json::from_str(&output).unwrap();
        assert_eq!(m.role(), Role::Tool);
        assert!(
            matches!(m.content().as_slice(), [Content::ToolResult(r)] if r.call.to_string() == "c1")
        );
    }

    #[rstest]
    fn tool_call_and_output_have_distinct_ids_despite_shared_call_id() {
        let call = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_x", "name": "sh", "input": "ls"},
        })
        .to_string();
        let output = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "custom_tool_call_output", "id": "ctco_1", "call_id": "call_x", "output": "files"},
        })
        .to_string();
        let call: CodexMessage = serde_json::from_str(&call).unwrap();
        let output: CodexMessage = serde_json::from_str(&output).unwrap();
        assert_ne!(call.id(), output.id(), "call and its output must not share a source id");
        assert_eq!(call.id(), Some(MessageId::from("ctc_1".to_owned())));
        assert_eq!(output.id(), Some(MessageId::from("ctco_1".to_owned())));
    }

    /// Older rollouts carry no per-record id: the call keys on `call_id` and its output must
    /// still get a distinct id, or the dedup gate drops every tool result.
    #[rstest]
    #[case("function_call", "function_call_output")]
    #[case("custom_tool_call", "custom_tool_call_output")]
    fn id_less_tool_output_does_not_collide_with_its_call(
        #[case] call_kind: &str,
        #[case] output_kind: &str,
    ) {
        let call: CodexMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "response_item",
                "payload": {"type": call_kind, "call_id": "call_x", "name": "sh", "arguments": "ls"},
            })
            .to_string(),
        )
        .unwrap();
        let output: CodexMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "response_item",
                "payload": {"type": output_kind, "call_id": "call_x", "output": "files"},
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(call.id(), Some(MessageId::from("call_x".to_owned())));
        assert_eq!(output.id(), Some(MessageId::from("call_x#out".to_owned())));
    }

    #[rstest]
    #[case::exec_ok(serde_json::json!("Exit code: 0\nWall time: 0.1 seconds\nOutput:\nok"), false)]
    #[case::exec_failed(serde_json::json!("Exit code: 2\nWall time: 0.1 seconds\nOutput:\nboom"), true)]
    #[case::unified_ok(
        serde_json::json!("Chunk ID: c1\nWall time: 0.2000 seconds\nProcess exited with code 0\nOutput:\nok"),
        false
    )]
    #[case::unified_killed(
        serde_json::json!("Wall time: 0.2000 seconds\nProcess exited with code -9\nOutput:\n"),
        true
    )]
    #[case::unified_still_running(
        serde_json::json!("Wall time: 1.0000 seconds\nProcess running with session ID 7\nOutput:\npartial"),
        false
    )]
    #[case::output_quotes_a_failure(
        serde_json::json!("Exit code: 0\nWall time: 0.1 seconds\nOutput:\nExit code: 1\nProcess exited with code 1"),
        false
    )]
    #[case::output_quotes_a_success(
        serde_json::json!("Exit code: 1\nWall time: 0.1 seconds\nOutput:\nExit code: 0"),
        true
    )]
    #[case::json_ok(serde_json::json!([{"type": "output_text", "text": "{\"output\":\"x\",\"exit_code\":0}"}]), false)]
    #[case::json_failed(serde_json::json!([{"type": "output_text", "text": "{\"output\":\"x\",\"exit_code\":1}"}]), true)]
    #[case::json_output_quotes_a_failure(
        serde_json::json!("{\"output\":\"\\\"exit_code\\\":1\",\"exit_code\":0}"),
        false
    )]
    #[case::legacy_metadata(
        serde_json::json!("{\"output\":\"x\",\"metadata\":{\"exit_code\":1,\"duration_seconds\":0.1}}"),
        true
    )]
    #[case::plain_text(serde_json::json!("plain text"), false)]
    fn tool_output_error_is_derived_from_exit_code(
        #[case] output: serde_json::Value,
        #[case] error: bool,
    ) {
        let m: CodexMessage = serde_json::from_str(
            &serde_json::json!({
                "type": "response_item",
                "payload": {"type": "function_call_output", "call_id": "c1", "output": output},
            })
            .to_string(),
        )
        .unwrap();
        assert!(matches!(m.content().as_slice(), [Content::ToolResult(r)] if r.error == error));
    }

    #[rstest]
    #[case("turn_aborted", Some(StopReason::Aborted))]
    #[case("token_count", None)]
    fn turn_aborted_events_end_the_turn(#[case] kind: &str, #[case] expected: Option<StopReason>) {
        let m: CodexMessage = serde_json::from_str(
            &serde_json::json!({"type": "event_msg", "payload": {"type": kind}}).to_string(),
        )
        .unwrap();
        assert_eq!(m.stop_reason(), expected);
    }

    #[rstest]
    fn listener_reports_not_found_for_a_missing_root() {
        let sessions =
            CodexSessions::builder().root(PathBuf::from("/no/such/codex")).pool(pool()).build();
        assert!(matches!(sessions.listener(), Err(RuntimeError::NotFound(_))));
    }

    #[rstest]
    #[tokio::test]
    async fn messages_streams_turns_from_a_rollout_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-2026-09-18-th1.jsonl");
        let body = [
            serde_json::json!({"type": "session_meta", "payload": {"id": "th1"}}).to_string(),
            serde_json::json!({
                "type": "message",
                "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
            })
            .to_string(),
        ]
        // Trailing newline required: messages() withholds an unterminated final line until a
        // later write completes it (a real session ends every record with a newline).
        .join("\n")
            + "\n";
        std::fs::write(&path, body).unwrap();

        let session = CodexSession::open(SessionId::from("th1".to_owned()), path, pool());
        let roles: Vec<Role> =
            session.messages().take(2).map_ok(|m| m.role()).try_collect().await.unwrap();
        assert_eq!(roles.last(), Some(&Role::User));
    }

    #[rstest]
    #[tokio::test]
    async fn watch_emits_sessions_as_files_appear() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("2026").join("09").join("19");
        std::fs::create_dir_all(&sub).unwrap();
        let sid = "0a1b2c3d-4e5f-6789-abcd-ef0123456789";
        std::fs::write(
            sub.join(format!("rollout-2026-09-19T00-00-00-{sid}.jsonl")),
            serde_json::json!({"type": "session_meta", "payload": {"id": sid}}).to_string(),
        )
        .unwrap();

        let listener = CodexSessions::builder()
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
        assert_eq!(seen, vec![SessionId::from(sid.to_owned())]);
    }

    /// Archiving moves a thread's rollout from `sessions/` to `archived_sessions/`: it is still
    /// found, and once only if it was caught in both places mid-move.
    #[rstest]
    #[tokio::test]
    async fn existing_includes_archived_threads() {
        let home = tempfile::tempdir().unwrap();
        let (live, archived) =
            ("0a1b2c3d-4e5f-6789-abcd-ef0123456789", "1a1b2c3d-4e5f-6789-abcd-ef0123456789");
        let name = |id: &str| format!("rollout-2026-09-19T00-00-00-{id}.jsonl");
        let day = home.path().join("sessions/2026/09/19");
        let archive = home.path().join("archived_sessions");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        for path in [day.join(name(live)), archive.join(name(live)), archive.join(name(archived))] {
            std::fs::write(path, "").unwrap();
        }

        let sessions =
            CodexSessions::builder().root(home.path().join("sessions")).pool(pool()).build();
        let mut found: Vec<(String, bool)> = sessions
            .existing()
            .unwrap()
            .map_ok(|s| (s.id().to_string(), s.path.starts_with(&day)))
            .try_collect()
            .await
            .unwrap();
        found.sort();
        assert_eq!(found, [(live.to_owned(), true), (archived.to_owned(), false)]);
    }

    fn rollout(dir: &Path) -> PathBuf {
        let path =
            dir.join("rollout-2026-09-19T00-00-00-0a1b2c3d-4e5f-6789-abcd-ef0123456789.jsonl");
        std::fs::write(
            &path,
            serde_json::json!({"type": "session_meta", "payload": {"id": "th1"}}).to_string()
                + "\n",
        )
        .unwrap();
        path
    }

    #[rstest]
    #[tokio::test]
    async fn messages_yields_lines_appended_after_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = rollout(dir.path());

        let listener = CodexSessions::builder()
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
                "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
            })
            .to_string()
                + "\n")
                .as_bytes(),
        )
        .unwrap();
        drop(file);
        assert_eq!(messages.next().await.unwrap().unwrap().role(), Role::User);
    }

    #[rstest]
    #[tokio::test]
    async fn messages_ends_when_the_session_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = rollout(dir.path());

        let listener = CodexSessions::builder()
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

    /// The fixture records usage per model call and snapshots it after each: only the nine
    /// records count.
    #[rstest]
    #[tokio::test]
    async fn a_real_rollout_counts_each_recorded_call_once() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/session1.jsonl");
        let session = CodexSession::open(SessionId::from("s".to_owned()), path, pool());
        let lines: Vec<CodexMessage> = session.read().try_collect().await.unwrap();
        let usage: Vec<&CodexMessage> = lines.iter().filter(|m| m.usage().is_some()).collect();
        assert_eq!(usage.len(), 9);
        assert!(usage.iter().all(|m| m.kind == TOKEN_USAGE_RECORD && m.turn_id().is_some()));
    }

    #[rstest]
    #[case(include_str!("../../../tests/fixtures/codex/session1.jsonl"))]
    fn normalizes_a_real_redacted_session(#[case] jsonl: &str) {
        let msgs: Vec<CodexMessage> = jsonl
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<CodexMessage>(l).expect("fixture record parses"))
            .collect();
        assert!(msgs.len() >= 15);

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
                        assert!(!u.name.is_empty(), "custom_tool_call normalized to an empty name");
                        assert!(!u.id.to_string().is_empty(), "tool call lost its id");
                        tool_uses += 1;
                    }
                    Content::ToolResult(r) => {
                        assert!(!r.call.to_string().is_empty(), "tool result lost its call id");
                        tool_results += 1;
                    }
                    _ => {}
                }
            }
        }
        assert!(saw_user && saw_assistant, "expected both user and assistant turns");
        assert!(tool_uses >= 1, "expected at least one normalized tool call");
        assert!(tool_results >= 1, "expected at least one normalized tool result");
    }

    fn line(raw: &serde_json::Value) -> CodexMessage {
        serde_json::from_str(&raw.to_string()).unwrap()
    }

    fn token_usage(input: u64, cached: u64, output: u64) -> serde_json::Value {
        serde_json::json!({
            "input_tokens": input, "cached_input_tokens": cached, "cache_write_input_tokens": 0,
            "output_tokens": output, "reasoning_output_tokens": 0, "total_tokens": input + output,
        })
    }

    fn token_count(total: &serde_json::Value, last: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "timestamp": "2025-09-18T10:00:00.000Z", "type": "event_msg",
            "payload": {"type": "token_count", "info": {
                "total_token_usage": total, "last_token_usage": last,
                "model_context_window": 258_400,
            }, "rate_limits": null},
        })
    }

    fn usage_record(response: &str, usage: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "timestamp": "2026-09-18T10:00:00.000Z", "type": "token_usage_record",
            "payload": {"turn_id": "t1", "response_id": response, "usage": usage,
                "turn_token_usage": usage, "thread_token_usage": usage},
        })
    }

    /// Rollouts written before `token_usage_record` existed report usage only on
    /// `event_msg`/`token_count`: `info.last_token_usage` is the per-call delta.
    #[rstest]
    fn legacy_token_count_usage_is_captured() {
        let m = line(&token_count(
            &token_usage(30_000, 20_000, 300),
            &token_usage(15_000, 10_000, 100),
        ));
        assert_eq!(
            m.usage(),
            Some(Usage {
                input: Some(5_000),
                output: Some(100),
                cache_read: Some(10_000),
                cache_write: Some(0),
                reasoning: Some(0),
            })
        );
        assert!(m.turn_id().is_some(), "usage-bearing line without a turn id");
        assert!(m.id().is_some());
    }

    /// Codex re-sends the same snapshot (a rate-limit refresh); it names the same model call,
    /// so the pipeline counts it once. Another call's snapshot names a different one.
    #[rstest]
    fn repeated_token_count_snapshots_name_one_turn() {
        let first = line(&token_count(&token_usage(30, 20, 3), &token_usage(15, 10, 1)));
        let again = line(&token_count(&token_usage(30, 20, 3), &token_usage(15, 10, 1)));
        let next = line(&token_count(&token_usage(45, 30, 4), &token_usage(15, 10, 1)));
        assert_eq!(first.turn_id(), again.turn_id());
        assert_eq!(first.id(), again.id());
        assert_ne!(first.turn_id(), next.turn_id());
        assert_ne!(first.id(), next.id());
    }

    /// Snapshots that report no new tokens: rate limits only, an estimate after compaction
    /// (`recompute_token_usage`), the window filled after an overflow (`fill_to_context_window`).
    #[rstest]
    #[case(serde_json::json!({"timestamp": "2025-09-18T10:00:00.000Z", "type": "event_msg",
        "payload": {"type": "token_count", "info": null, "rate_limits": {}}}))]
    #[case(token_count(&token_usage(30, 20, 3), &serde_json::json!({"input_tokens": 0,
        "cached_input_tokens": 0, "output_tokens": 0, "reasoning_output_tokens": 0, "total_tokens": 900})))]
    fn token_count_without_new_tokens_has_no_usage(#[case] raw: serde_json::Value) {
        let m = line(&raw);
        assert_eq!(m.usage(), None);
        assert_eq!(m.turn_id(), None);
        assert_eq!(m.id(), None, "an empty snapshot would keep a row");
    }

    /// Modern rollouts write a `token_usage_record` per model call before the `token_count`
    /// snapshot of the same call (codex-rs `turn.rs`: `record_observed_response_completed`,
    /// then `send_token_count_event`); once one is seen, snapshots repeat usage already counted.
    #[rstest]
    fn token_count_after_a_usage_record_has_no_usage() {
        let mut m = line(&token_count(&token_usage(30, 20, 3), &token_usage(15, 10, 1)));
        m.context.usage_recorded = true;
        assert_eq!(m.usage(), None);
        assert_eq!(m.turn_id(), None);
        assert_eq!(m.id(), None);
    }

    /// Codex/OpenAI `input_tokens` already includes `cached_input_tokens`
    /// (codex-rs `TokenUsage::non_cached_input`), where `Usage::input` is fresh input only.
    #[rstest]
    fn usage_input_excludes_cached_input() {
        let m = line(&usage_record("resp_1", &token_usage(14_161, 9_984, 123)));
        let usage = m.usage().unwrap();
        assert_eq!(usage.cache_read, Some(9_984));
        assert_eq!(usage.input, Some(14_161 - 9_984), "cached input counted as fresh input too");
    }

    /// Cache writes are part of `input_tokens` too (the usage codex-rs
    /// `parses_cache_write_token_usage` builds from a Responses API reply).
    #[rstest]
    fn usage_input_excludes_cache_writes() {
        let usage = serde_json::json!({
            "input_tokens": 100, "cached_input_tokens": 40, "cache_write_input_tokens": 60,
            "output_tokens": 10, "reasoning_output_tokens": 5, "total_tokens": 110,
        });
        assert_eq!(
            line(&usage_record("resp_1", &usage)).usage(),
            Some(Usage {
                input: Some(0),
                output: Some(10),
                cache_read: Some(40),
                cache_write: Some(60),
                reasoning: Some(5),
            })
        );
    }

    /// A usage record names its model call by `response_id`, which also keys its row: the line
    /// has no id, and id-less lines in one millisecond would otherwise collide.
    #[rstest]
    fn usage_record_is_keyed_by_its_response() {
        let a = line(&usage_record("resp_a", &token_usage(10, 0, 5)));
        let b = line(&usage_record("resp_b", &token_usage(10, 0, 5)));
        assert_eq!(a.turn_id().as_deref(), Some("resp_a"));
        assert_ne!(a.id(), b.id());
        assert!(a.id().is_some());
    }

    /// `session_meta` carries `git: {commit_hash, branch, repository_url}`
    /// (codex-rs `SessionMetaLine.git`).
    #[rstest]
    fn session_meta_git_branch_is_read() {
        let m = line(&serde_json::json!({
            "timestamp": "2026-09-18T10:00:00.000Z", "type": "session_meta",
            "payload": {"id": "child", "cwd": "/work", "git": {"branch": "main", "commit_hash": "abc"}},
        }));
        assert_eq!(m.git_branch().as_deref(), Some("main"));
    }

    /// A forked or subagent rollout names its origin in `session_meta.forked_from_id` /
    /// `parent_thread_id` (codex-rs `SessionMeta`), or, in older rollouts, under
    /// `source.subagent.thread_spawn`.
    #[rstest]
    #[case(serde_json::json!({"id": "child", "cwd": "/work", "forked_from_id": "parent"}))]
    #[case(serde_json::json!({"id": "child", "cwd": "/work", "parent_thread_id": "parent"}))]
    #[case(serde_json::json!({"id": "child", "cwd": "/work",
        "source": {"subagent": {"thread_spawn": {"parent_thread_id": "parent", "depth": 1}}}}))]
    fn session_meta_names_its_parent(#[case] payload: serde_json::Value) {
        let m = line(&serde_json::json!({
            "timestamp": "2026-09-18T10:00:00.000Z", "type": "session_meta", "payload": payload,
        }));
        assert_eq!(m.parent_session(), Some(SessionId::from("parent".to_owned())));
    }

    /// A fork copies its parent's rollout, `session_meta` included, after its own
    /// (codex-rs `ForkPersistence::Copied`). The copy describes the parent: it must not
    /// replace the child's parent link (with the grandparent) or its session context.
    #[rstest]
    #[tokio::test]
    async fn a_fork_keeps_its_own_session_meta() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-child.jsonl");
        let body = [
            serde_json::json!({"type": "session_meta",
                "payload": {"id": "child", "forked_from_id": "parent", "cwd": "/work"}}),
            serde_json::json!({"type": "session_meta", "payload": {"id": "parent",
                "forked_from_id": "grandparent", "cwd": "/elsewhere", "git": {"branch": "old"}}}),
        ]
        .map(|l| l.to_string() + "\n")
        .concat();
        std::fs::write(&path, body).unwrap();

        let session = CodexSession::open(SessionId::from("child".to_owned()), path, pool());
        let lines: Vec<CodexMessage> = session.read().try_collect().await.unwrap();
        let parents: Vec<_> = lines.iter().map(Message::parent_session).collect();
        assert_eq!(parents, vec![Some(SessionId::from("parent".to_owned())), None]);
        assert_eq!(lines[1].cwd(), None);
        assert_eq!(lines[1].git_branch(), None);
        assert_eq!(lines[1].id(), None);
    }

    fn write_lines(path: &Path, lines: &[serde_json::Value]) {
        let body: String = lines.iter().map(|l| l.to_string() + "\n").collect();
        std::fs::write(path, body).unwrap();
    }

    /// Usage over a whole rollout: a legacy one counts its snapshots, a modern one only its
    /// records, and one written by both (a legacy session resumed by a newer Codex) its
    /// snapshots until records begin.
    #[rstest]
    #[case::legacy(vec![
        token_count(&token_usage(10, 0, 1), &token_usage(10, 0, 1)),
        token_count(&token_usage(30, 0, 3), &token_usage(20, 0, 2)),
    ], vec![1, 2])]
    #[case::modern(vec![
        usage_record("resp_a", &token_usage(10, 0, 1)),
        token_count(&token_usage(10, 0, 1), &token_usage(10, 0, 1)),
        usage_record("resp_b", &token_usage(20, 0, 2)),
        token_count(&token_usage(30, 0, 3), &token_usage(20, 0, 2)),
    ], vec![1, 2])]
    #[case::resumed(vec![
        token_count(&token_usage(10, 0, 1), &token_usage(10, 0, 1)),
        usage_record("resp_b", &token_usage(20, 0, 2)),
        token_count(&token_usage(30, 0, 3), &token_usage(20, 0, 2)),
    ], vec![1, 2])]
    #[tokio::test]
    async fn a_rollout_reports_each_call_once(
        #[case] lines: Vec<serde_json::Value>,
        #[case] expected: Vec<u64>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-s.jsonl");
        write_lines(&path, &lines);
        let session = CodexSession::open(SessionId::from("s".to_owned()), path, pool());
        let outputs: Vec<u64> = session
            .messages()
            .try_filter_map(|m| async move { Ok(m.usage().and_then(|u| u.output)) })
            .try_collect()
            .await
            .unwrap();
        assert_eq!(outputs, expected);
    }

    /// Resumed past a usage record, a snapshot still knows the rollout records usage: the
    /// reader looks the lines it skipped up in the file. A checkpoint that no longer names its
    /// line reads from the start, where the record is seen.
    #[rstest]
    #[case::from_its_checkpoint(true)]
    #[case::from_a_stale_checkpoint(false)]
    #[tokio::test]
    async fn a_resumed_reader_still_knows_usage_is_recorded(#[case] valid: bool) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-s.jsonl");
        let record = usage_record("resp_a", &token_usage(10, 0, 1)).to_string();
        write_lines(&path, &[
            serde_json::from_str(&record).unwrap(),
            token_count(&token_usage(10, 0, 1), &token_usage(10, 0, 1)),
        ]);
        let past_record = u64::try_from(record.len() + 1).unwrap();
        let named = if valid {
            record.as_str()
        } else {
            "another line"
        };
        let from = Checkpoint::new(past_record, named.as_bytes());

        let session = CodexSession::open(SessionId::from("s".to_owned()), path, pool());
        let lines: Vec<CodexMessage> =
            session.messages_from(Some(from)).map_ok(|(_, m)| m).try_collect().await.unwrap();
        assert_eq!(
            lines.len(),
            if valid {
                1
            } else {
                2
            }
        );
        let snapshot = lines.last().unwrap();
        assert!(snapshot.is_token_count());
        assert_eq!(snapshot.usage(), None, "a recorded call counted twice after a resume");
    }

    #[rstest]
    #[case::released("0.153.0", true)]
    #[case::newer("0.156.1", true)]
    #[case::prerelease("0.158.0-alpha.6", true)]
    #[case::first_prerelease("0.153.0-alpha.1", true)]
    #[case::major("1.0.0", true)]
    #[case::before_records("0.152.1", false)]
    #[case::old("0.46.0", false)]
    #[case::development_build("0.0.0", false)]
    #[case::unreadable("dev", false)]
    fn a_codex_version_tells_whether_it_records_usage(
        #[case] version: &str,
        #[case] records: bool,
    ) {
        assert_eq!(records_usage(&serde_json::json!({"cli_version": version})), records);
    }

    fn own_meta(version: &str) -> serde_json::Value {
        serde_json::json!({"timestamp": "2026-09-24T05:43:07.582Z", "type": "session_meta",
            "payload": {"id": "s", "cli_version": version, "parent_thread_id": "p"}})
    }

    /// A subagent forked from a thread in legacy history mode begins with a copy of the
    /// parent's history: its snapshots but not its records. A Codex that records usage counts
    /// those calls in the parent; an older one wrote no records anywhere, so the copies are
    /// keyed as the parent's own snapshots are and the parent's claim wins.
    #[rstest]
    #[case::recording_codex("0.158.0-alpha.6", vec![3])]
    #[case::older_codex("0.152.1", vec![1, 2, 3])]
    #[tokio::test]
    async fn copied_snapshots_count_only_where_nothing_recorded_them(
        #[case] version: &str,
        #[case] expected: Vec<u64>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-s.jsonl");
        write_lines(&path, &[
            own_meta(version),
            serde_json::json!({"type": "session_meta", "payload": {"id": "p"}}),
            token_count(&token_usage(10, 0, 1), &token_usage(10, 0, 1)),
            token_count(&token_usage(30, 0, 3), &token_usage(20, 0, 2)),
            usage_record("resp_c", &token_usage(40, 0, 3)),
            token_count(&token_usage(70, 0, 6), &token_usage(40, 0, 3)),
        ]);
        let session = CodexSession::open(SessionId::from("s".to_owned()), path, pool());
        let outputs: Vec<u64> = session
            .messages()
            .try_filter_map(|m| async move { Ok(m.usage().and_then(|u| u.output)) })
            .try_collect()
            .await
            .unwrap();
        assert_eq!(outputs, expected);
    }

    /// Resumed inside the copied history, a reader still knows the rollout records usage.
    #[rstest]
    #[tokio::test]
    async fn a_resumed_reader_knows_its_codex_records_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-s.jsonl");
        let first = token_count(&token_usage(10, 0, 1), &token_usage(10, 0, 1));
        write_lines(&path, &[
            own_meta("0.156.1"),
            first.clone(),
            token_count(&token_usage(30, 0, 3), &token_usage(20, 0, 2)),
        ]);
        let body = std::fs::read(&path).unwrap();
        let end = memchr::memchr_iter(b'\n', &body).nth(1).unwrap();
        let start = memchr::memchr(b'\n', &body).unwrap() + 1;
        let from = Checkpoint::new(u64::try_from(end + 1).unwrap(), &body[start..end]);

        let session = CodexSession::open(SessionId::from("s".to_owned()), path, pool());
        let lines: Vec<CodexMessage> =
            session.messages_from(Some(from)).map_ok(|(_, m)| m).try_collect().await.unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].usage(), None, "a copied snapshot counted after a resume");
    }

    /// As Codex 0.158.0-alpha.6 writes it: a subagent forked from a thread in legacy history
    /// mode starts with the parent's history, the parent's two calls among it as snapshots
    /// only; the subagent's own call is its one record.
    #[rstest]
    #[tokio::test]
    async fn a_legacy_forked_subagent_counts_only_its_own_call() {
        let id = "01a0d1f0-6420-7493-8d85-56f7921317d5";
        let lines = read_session(id, fixture("legacy-forked-subagent.jsonl")).await;
        let usage: Vec<(Option<String>, Option<u64>)> =
            lines.iter().filter_map(|m| m.usage().map(|u| (m.turn_id(), u.output))).collect();
        assert_eq!(usage, [(Some("resp_0003".to_owned()), Some(53))]);
    }

    #[rstest]
    #[case(b"abc", 3, b"abc".as_slice(), true)]
    #[case(b"abc", 2, b"abc".as_slice(), false)]
    #[case(b"xyz", 3, b"abc".as_slice(), false)]
    fn prefix_contains_stops_at_the_end(
        #[case] body: &[u8],
        #[case] end: u64,
        #[case] needle: &[u8],
        #[case] expected: bool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, body).unwrap();
        assert_eq!(prefix_contains(&path, end, needle).unwrap(), expected);
    }

    /// A marker split across two read chunks is still found.
    #[rstest]
    fn prefix_contains_finds_a_needle_across_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        let mut body = vec![b'x'; 64 * 1024 - 5];
        body.extend_from_slice(TOKEN_USAGE_RECORD_MARKER);
        std::fs::write(&path, &body).unwrap();
        let len = u64::try_from(body.len()).unwrap();
        assert!(prefix_contains(&path, len, TOKEN_USAGE_RECORD_MARKER).unwrap());
    }

    /// A `compacted` line carries the compaction summary in `payload.message` (codex-rs
    /// `CompactedItemWire.message`).
    #[rstest]
    fn compacted_summary_is_kept() {
        let m = line(&serde_json::json!({
            "timestamp": "2026-09-18T10:00:00.000Z", "type": "compacted",
            "payload": {"message": "Summary of the conversation so far", "replacement_history": []},
        }));
        assert_eq!(m.content(), vec![Content::Summary(
            "Summary of the conversation so far".to_owned()
        )]);
        assert_eq!(m.role(), Role::System);
    }

    /// `turn_complete` persists terminal error details in `payload.error` (codex-rs
    /// `TurnCompleteEvent.error`), `turn_aborted` its reason (`TurnAbortedEvent.reason`).
    #[rstest]
    #[case(serde_json::json!({"type": "turn_complete", "turn_id": "t1", "last_agent_message": null,
        "error": {"message": "stream disconnected before completion", "codex_error_info": null}}),
        Some((StopReason::Error, "stream disconnected before completion")))]
    #[case(serde_json::json!({"type": "turn_aborted", "turn_id": "t1", "reason": "interrupted"}),
        Some((StopReason::Aborted, "interrupted")))]
    #[case(serde_json::json!({"type": "turn_complete", "turn_id": "t1", "last_agent_message": "ok"}),
        None)]
    // As codex 0.156.1 writes them (`EventMsg::TurnComplete` serializes as `task_complete`),
    // from runs against a mock Responses API that failed the call.
    #[case::task_complete_failed(serde_json::json!({"type": "task_complete",
        "turn_id": "01a0d148-8dec-7e81-910a-4c5609413f17", "last_agent_message": null,
        "error": {"message": "stream disconnected before completion: mock upstream exploded",
            "codex_error_info": "other"},
        "started_at": 1_790_217_588, "completed_at": 1_790_217_588, "duration_ms": 113}),
        Some((StopReason::Error, "stream disconnected before completion: mock upstream exploded")))]
    #[case::task_complete_context_window(serde_json::json!({"type": "task_complete",
        "turn_id": "01a0d148-8f94-77f0-a86b-f1a057cedbcc", "last_agent_message": null,
        "error": {"message": "Codex ran out of room in the model's context window. Start a new \
            thread or clear earlier history before retrying.",
            "codex_error_info": "context_window_exceeded"},
        "started_at": 1_790_217_588, "completed_at": 1_790_217_588, "duration_ms": 112}),
        Some((StopReason::Error, "Codex ran out of room in the model's context window. Start a \
            new thread or clear earlier history before retrying.")))]
    #[case::task_complete_ok(serde_json::json!({"type": "task_complete",
        "turn_id": "01a0d147-e769-7640-98cf-19953c6fb04b", "last_agent_message": "Echo: hi",
        "started_at": 1_790_217_545, "completed_at": 1_790_217_545, "duration_ms": 129}), None)]
    fn turn_failures_report_an_error(
        #[case] payload: serde_json::Value,
        #[case] expected: Option<(StopReason, &str)>,
    ) {
        let m = line(&serde_json::json!({
            "timestamp": "2026-09-18T10:00:00.000Z", "type": "event_msg", "payload": payload,
        }));
        assert_eq!(m.stop_reason(), expected.as_ref().map(|(reason, _)| reason.clone()));
        let errors: Vec<Content> =
            expected.iter().map(|(_, why)| Content::Error((*why).to_owned())).collect();
        assert_eq!(m.content(), errors);
    }

    /// Codex injects AGENTS.md, `<environment_context>` and other context as `role: "user"`
    /// messages (codex-rs `core/src/context/contextual_user_message.rs`,
    /// `is_contextual_user_fragment`); they are not user turns.
    #[rstest]
    #[case("# AGENTS.md instructions for /work\n\n<INSTRUCTIONS>\nbe nice\n</INSTRUCTIONS>")]
    #[case("<environment_context>\n  <cwd>/work</cwd>\n</environment_context>")]
    #[case("<user_instructions>\nbe nice\n</user_instructions>")]
    #[case("<skill>\n<name>demo</name>\n</skill>")]
    #[case("<external_ide_context>open: a.rs</external_ide_context>")]
    #[case("<codex_internal_context source=\"goal\">\nx\n</codex_internal_context>")]
    #[case("<hook_prompt hook_run_id=\"h1\">do it</hook_prompt>")]
    // What a review hands back to the thread that asked for it (from a `codex exec review` run).
    #[case(
        "<user_action>\n  <context>User initiated a review task. Here's the full review output \
         from reviewer model. User may select one or more comments to resolve.</context>\n  \
         <action>review</action>\n  <results>\n  Looks fine.\n  </results>\n  </user_action>\n"
    )]
    #[case(
        "Warning: apply_patch was requested via exec_command. Use the apply_patch tool instead of \
         exec_command."
    )]
    fn contextual_user_messages_are_system(#[case] text: &str) {
        let m = line(&serde_json::json!({
            "timestamp": "2026-09-18T10:00:00.000Z", "type": "response_item",
            "payload": {"type": "message", "id": "msg_1", "role": "user",
                "content": [{"type": "input_text", "text": text}]},
        }));
        assert_eq!(m.role(), Role::System, "harness-injected context captured as a user turn");
    }

    #[rstest]
    #[case("please read AGENTS.md")]
    #[case("<environment_context> is what codex sends")]
    #[case("<external_a>mismatched</external_b>")]
    fn typed_user_messages_stay_user_turns(#[case] text: &str) {
        let m = line(&serde_json::json!({
            "type": "response_item",
            "payload": {"type": "message", "id": "msg_1", "role": "user",
                "content": [{"type": "input_text", "text": text}]},
        }));
        assert_eq!(m.role(), Role::User);
    }

    /// A command the user ran from the prompt keeps what they typed, never its output.
    #[rstest]
    fn a_user_shell_command_keeps_the_command_not_its_output() {
        let text = "<user_shell_command>\n<command>\ncat .env\n</command>\n<result>\nExit code: \
                    0\nDuration: 0.0100 \
                    seconds\nOutput:\nDB_PASSWORD=hunter2\n</result>\n</user_shell_command>";
        let m = line(&serde_json::json!({
            "type": "response_item",
            "payload": {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": text}]},
        }));
        assert_eq!(m.role(), Role::User);
        assert_eq!(m.content(), vec![Content::Text("! cat .env".to_owned())]);
    }

    /// `developer` is the Responses API system role.
    #[rstest]
    fn developer_role_is_system() {
        let m = line(&serde_json::json!({
            "type": "response_item",
            "payload": {"type": "message", "id": "msg_1", "role": "developer",
                "content": [{"type": "input_text", "text": "<permissions instructions>"}]},
        }));
        assert_eq!(m.role(), Role::System);
    }

    /// Tool calls other than function/custom calls (codex-rs `ResponseItem::LocalShellCall`,
    /// `WebSearchCall`, `ToolSearchCall`) are tool uses too.
    #[rstest]
    #[case(serde_json::json!({"type": "local_shell_call", "id": "lsh_1", "call_id": "c1",
        "status": "completed", "action": {"type": "exec", "command": ["ls"]}}), "c1", "local_shell")]
    #[case(serde_json::json!({"type": "web_search_call", "id": "ws_1", "status": "completed",
        "action": {"type": "search", "query": "rust"}}), "ws_1", "web_search")]
    #[case(serde_json::json!({"type": "tool_search_call", "id": "ts_1", "call_id": "c2",
        "execution": "server", "arguments": {}}), "c2", "tool_search")]
    #[case(serde_json::json!({"type": "image_generation_call", "id": "ig_1", "status": "completed",
        "revised_prompt": "a cat", "result": "iVBORw0KGgo="}), "ig_1", "image_generation")]
    fn other_tool_calls_normalize_to_tool_use(
        #[case] payload: serde_json::Value,
        #[case] call: &str,
        #[case] name: &str,
    ) {
        let m = line(&serde_json::json!({"type": "response_item", "payload": payload}));
        assert_eq!(m.role(), Role::Assistant);
        assert!(
            matches!(m.content().as_slice(), [Content::ToolUse(u)] if u.id.as_ref() == call && u.name == name),
            "tool call lost: {:?}",
            m.content()
        );
    }

    /// A tool search's results come back as `tool_search_output` under its `call_id`; a local
    /// shell call's as a `function_call_output`.
    #[rstest]
    #[case(serde_json::json!({"type": "tool_search_output", "call_id": "c2", "status": "completed",
        "execution": "server", "tools": []}), "c2#out")]
    #[case(serde_json::json!({"type": "function_call_output", "call_id": "c1",
        "output": "files\nProcess exited with code 0"}), "c1#out")]
    fn other_tool_outputs_answer_their_call(#[case] payload: serde_json::Value, #[case] id: &str) {
        let m = line(&serde_json::json!({"type": "response_item", "payload": payload}));
        assert_eq!(m.role(), Role::Tool);
        assert_eq!(m.id(), Some(MessageId::from(id.to_owned())));
        assert!(matches!(m.content().as_slice(), [Content::ToolResult(r)] if !r.error));
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex").join(name)
    }

    async fn read_session(id: &str, path: PathBuf) -> Vec<CodexMessage> {
        let session = CodexSession::open(SessionId::from(id.to_owned()), path, pool());
        session.read().try_collect().await.unwrap()
    }

    /// What a line would become a row of (see the daemon's `message_enricher::build`).
    fn kept(m: &CodexMessage) -> bool {
        m.id().is_some()
            || !m.content().is_empty()
            || m.usage().is_some()
            || m.stop_reason().is_some()
            || m.model().is_some()
            || m.cwd().is_some()
            || m.title().is_some()
    }

    fn texts(lines: &[CodexMessage]) -> Vec<(Role, String)> {
        lines
            .iter()
            .flat_map(|m| {
                m.content().into_iter().filter_map(move |c| match c {
                    Content::Text(t) | Content::Summary(t) => Some((m.role(), t)),
                    _ => None,
                })
            })
            .collect()
    }

    /// The rollout name carries the session: the thread id, or for a thread reverted into a
    /// new rollout (codex-rs `rollout_file_name.rs`) both ids, so the two files never share one.
    #[rstest]
    #[case::current(
        "rollout-2026-09-24T02-39-05-01a0d147-e745-7ae2-a941-ce48e7888470",
        "01a0d147-e745-7ae2-a941-ce48e7888470"
    )]
    #[case::date_only(
        "rollout-2025-05-07-5973b6c0-94b8-487b-a530-2aeb6098ae0e",
        "5973b6c0-94b8-487b-a530-2aeb6098ae0e"
    )]
    #[case::reverted(
        "rollout-2026-09-24T03-00-00-01a0d147-e745-7ae2-a941-ce48e7888470_01a0d160-0000-7000-8000-000000000001",
        "01a0d147-e745-7ae2-a941-ce48e7888470_01a0d160-0000-7000-8000-000000000001"
    )]
    fn a_rollout_name_carries_its_session(#[case] stem: &str, #[case] id: &str) {
        assert_eq!(session_id_of(stem), SessionId::from(id.to_owned()));
    }

    /// A reverted thread's rollout keeps the thread's own `session_meta` (its `id` is the thread
    /// id), and continues the rollout its `history_base` names.
    #[rstest]
    #[case::from_the_first_rollout(
        "01a0d147-e745-7ae2-a941-ce48e7888470",
        "01a0d147-e745-7ae2-a941-ce48e7888470"
    )]
    #[case::from_an_earlier_revert(
        "01a0d150-0000-7000-8000-000000000009",
        "01a0d147-e745-7ae2-a941-ce48e7888470_01a0d150-0000-7000-8000-000000000009"
    )]
    fn a_reverted_rollout_continues_its_history_base(#[case] base: &str, #[case] parent: &str) {
        let thread = "01a0d147-e745-7ae2-a941-ce48e7888470";
        let mut m = line(&serde_json::json!({
            "timestamp": "2026-09-24T03:00:00.000Z", "ordinal": 0, "type": "session_meta",
            "payload": {"session_id": thread, "id": thread, "cwd": "/work",
                "history_mode": "paginated", "git": {"branch": "main"},
                "history_base": {"thread_id": base, "end_ordinal_exclusive": 12,
                    "end_byte_offset": 4096}},
        }));
        m.context.session =
            Some(SessionId::from(format!("{thread}_01a0d160-0000-7000-8000-000000000001")));
        assert!(!m.is_copied_meta());
        assert_eq!(m.cwd(), Some(PathBuf::from("/work")));
        assert_eq!(m.git_branch().as_deref(), Some("main"));
        assert_eq!(m.parent_session(), Some(SessionId::from(parent.to_owned())));
    }

    /// Before 0.32.0 (codex-rs 43809a454e) a rollout had no `type`/`payload` envelope: its first
    /// line is the session meta itself, items are written bare, `{"record_type": "state"}` lines
    /// sit between them. The fixture follows the writer at codex-rs 2437a8d17a.
    #[rstest]
    #[tokio::test]
    async fn a_pre_envelope_rollout_is_read() {
        let lines =
            read_session("5973b6c0-94b8-487b-a530-2aeb6098ae0e", fixture("legacy-bare.jsonl"))
                .await;
        assert_eq!(lines.len(), 10, "a line failed to parse");
        assert_eq!(lines[0].git_branch().as_deref(), Some("main"));
        assert!(lines[0].timestamp().is_some());
        assert_eq!(texts(&lines), vec![
            (
                Role::System,
                "<environment_context>\n  <cwd>/work</cwd>\n</environment_context>".to_owned()
            ),
            (Role::User, "list the files".to_owned()),
            (Role::Assistant, "There is one file: a.txt".to_owned()),
        ]);
        let tools = lines
            .iter()
            .flat_map(Message::content)
            .filter(|c| matches!(c, Content::ToolUse(_) | Content::ToolResult(_)))
            .count();
        assert_eq!(tools, 2);
        assert_eq!(lines.iter().filter(|m| kept(m)).count(), 7, "state snapshots keep no row");
    }

    /// A subagent spawned with its parent's context (`fork_context`) starts its rollout with a
    /// copy of the parent's (`subagent_history_start_ordinal`); only what follows is its own
    /// (codex-rs `SessionMeta`). Generated by codex 0.156.1 against a mock Responses API.
    #[rstest]
    #[tokio::test]
    async fn a_forked_subagent_keeps_only_its_own_history() {
        let id = "01a0d14f-ec4f-7ec1-abae-b9c7d4951878";
        let lines = read_session(id, fixture("forked-subagent.jsonl")).await;
        assert_eq!(texts(&lines), vec![
            (Role::User, "forked subagent task: say hi".to_owned()),
            (Role::Assistant, "Echo: forked subagent task: say hi".to_owned()),
        ]);
        assert_eq!(
            lines[0].parent_session(),
            Some(SessionId::from("01a0d14f-eb8f-7433-9ed4-80fd3b6921fa".to_owned()))
        );
        assert_eq!(lines.iter().filter(|m| m.usage().is_some()).count(), 1);
    }

    /// Resumed past its start, a reader still knows which lines are inherited.
    #[rstest]
    #[tokio::test]
    async fn a_resumed_reader_still_hides_inherited_history() {
        let path = fixture("forked-subagent.jsonl");
        let body = std::fs::read_to_string(&path).unwrap();
        let newline = body.find('\n').unwrap();
        let first =
            Checkpoint::new(u64::try_from(newline + 1).unwrap(), &body.as_bytes()[..newline]);
        let session = CodexSession::open(
            SessionId::from("01a0d14f-ec4f-7ec1-abae-b9c7d4951878".to_owned()),
            path,
            pool(),
        );
        let lines: Vec<CodexMessage> =
            session.messages_from(Some(first)).map_ok(|(_, m)| m).try_collect().await.unwrap();
        assert_eq!(
            texts(&lines).first().map(|(_, t)| t.as_str()),
            Some("forked subagent task: say hi")
        );
    }

    /// A followed rollout that is truncated or replaced is read again from its start, and what
    /// its earlier lines told the reader (a usage record seen, where inherited history ends) no
    /// longer holds: the new lines are read as a rollout of their own.
    #[rstest]
    #[case::truncated(false)]
    #[case::replaced(true)]
    #[tokio::test]
    async fn a_rewritten_rollout_forgets_what_its_old_lines_said(#[case] replace: bool) {
        let dir = tempfile::tempdir().unwrap();
        let sid = "0a1b2c3d-4e5f-6789-abcd-ef0123456789";
        let path = dir.path().join(format!("rollout-2026-09-19T00-00-00-{sid}.jsonl"));
        let user = |ordinal: u64, text: &str| {
            serde_json::json!({"timestamp": "2026-09-19T00:00:00.000Z", "type": "response_item",
                "ordinal": ordinal, "payload": {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": text}]}})
        };
        write_lines(&path, &[
            serde_json::json!({"timestamp": "2026-09-19T00:00:00.000Z", "type": "session_meta",
                "ordinal": 0, "payload": {"id": sid, "subagent_history_start_ordinal": 2}}),
            user(1, "inherited from the parent"),
            user(2, "the subagent's own task"),
            usage_record("resp_a", &token_usage(10, 0, 1)),
            token_count(&token_usage(10, 0, 1), &token_usage(10, 0, 1)),
        ]);

        let listener = CodexSessions::builder()
            .root(dir.path().to_path_buf())
            .pool(pool())
            .build()
            .listener()
            .unwrap();
        let mut sessions = std::pin::pin!(listener.watch());
        let watched = sessions.next().await.unwrap().unwrap();
        // Signalled by hand: a watcher may instead end the stream of a file replaced at its
        // path and start a new one, which would not show this reader carrying on.
        let (changed, changes) = watch::channel(*watched.changes.as_ref().unwrap().borrow());
        let session = CodexSession {
            changes: Some(changes),
            ..watched
        };
        let mut messages = std::pin::pin!(session.messages());
        let mut before = Vec::new();
        for _ in 0..5 {
            before.push(messages.next().await.unwrap().unwrap());
        }
        assert_eq!(texts(&before), [(Role::User, "the subagent's own task".to_owned())]);
        assert_eq!(before[4].usage(), None, "the record already counted this call");

        // Shorter than what was read, so a truncation in place is seen as one.
        let after = [user(1, "new"), token_count(&token_usage(2, 0, 2), &token_usage(2, 0, 2))];
        if replace {
            let staged = dir.path().join("staged");
            write_lines(&staged, &after);
            std::fs::rename(&staged, &path).unwrap();
        } else {
            write_lines(&path, &after);
        }
        changed.send_modify(|_| {});
        let mut reread = Vec::new();
        for _ in 0..2 {
            let next = tokio::time::timeout(std::time::Duration::from_secs(10), messages.next())
                .await
                .expect("the rewritten rollout was not read again within 10s");
            reread.push(next.unwrap().unwrap());
        }
        assert_eq!(texts(&reread), [(Role::User, "new".to_owned())]);
        assert_eq!(reread[1].usage().and_then(|u| u.output), Some(2));
    }

    /// Compaction as codex 0.156.1 writes it: the `compacted` line's `message` is the summary
    /// behind Codex's preamble, and the summary is all that is kept of it.
    #[rstest]
    #[tokio::test]
    async fn a_compaction_keeps_its_summary() {
        let id = "01a0d14f-e276-77d3-b955-89d5b0151306";
        let lines = read_session(id, fixture("paginated-compacted.jsonl")).await;
        let summaries: Vec<String> = lines
            .iter()
            .flat_map(Message::content)
            .filter_map(|c| match c {
                Content::Summary(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(summaries, vec!["SUMMARY: the user asked for things and we did them."; 2]);
        let users: Vec<String> = texts(&lines)
            .into_iter()
            .filter(|(role, _)| *role == Role::User)
            .map(|(_, t)| t)
            .collect();
        assert_eq!(users, ["compact turn one", "compact turn two", "compact turn three"]);
        // Five model calls, two of them the compactions'.
        assert_eq!(lines.iter().filter(|m| m.usage().is_some()).count(), 5);
    }

    /// Newer Codex marks the model output of a compaction call (codex-rs 29aa1df62e,
    /// `CodexHarnessMetadata::compaction_output`): not a reply to the user; the `compacted` line
    /// after it carries the summary.
    #[rstest]
    fn compaction_output_is_not_a_reply() {
        let m = line(&serde_json::json!({
            "timestamp": "2026-09-24T02:47:49.222Z", "ordinal": 19, "type": "response_item",
            "payload": {"type": "message", "id": "msg_resp_0002", "role": "assistant",
                "content": [{"type": "output_text", "text": "SUMMARY: done."}]},
            "metadata": {"compaction_output": true},
        }));
        assert!(!kept(&m));
    }

    /// Events hold no item of their own: one naming a tool call (`patch_apply_end`, written by
    /// legacy rollouts) must not take the call's id. Nor do opaque items keep a row.
    #[rstest]
    #[case(serde_json::json!({"type": "event_msg", "payload": {"type": "patch_apply_end",
        "call_id": "call_1", "stdout": "", "stderr": "", "success": true}}))]
    #[case(serde_json::json!({"type": "event_msg", "payload": {"type": "item_completed",
        "thread_id": "t", "turn_id": "u", "item": {"type": "UserMessage", "id": "m1", "content": []}}}))]
    #[case(serde_json::json!({"type": "response_item", "payload": {"type": "compaction",
        "id": "cmp_1", "encrypted_content": "Y29tcGFjdA=="}}))]
    #[case(serde_json::json!({"type": "world_state", "payload": {"full": true, "state": {}}}))]
    fn bookkeeping_keeps_no_row(#[case] raw: serde_json::Value) {
        let m = line(&raw);
        assert_eq!(m.id(), None);
        assert!(!kept(&m));
    }

    /// A message from another agent (multi-agent v2, codex-rs `ResponseItem::AgentMessage`) was
    /// delivered by the harness; its encrypted payload is not kept. From a codex 0.156.1 run.
    #[rstest]
    fn an_agent_message_is_system_without_its_ciphertext() {
        let m = line(&serde_json::json!({
            "timestamp": "2026-09-24T02:51:10.389Z", "ordinal": 9, "type": "response_item",
            "payload": {"type": "agent_message", "id": "amsg_01a0d152-f6b5-7200-a223-914e6099d827",
                "author": "/root", "recipient": "/root/helper_task", "content": [
                    {"type": "input_text", "text": "Message Type: NEW_TASK\nTask name: \
                        /root/helper_task\nSender: /root\nPayload:\n"},
                    {"type": "encrypted_content", "encrypted_content": "gAAAAB..."}]},
        }));
        assert_eq!(m.role(), Role::System);
        assert_eq!(m.content(), vec![Content::Text(
            "Message Type: NEW_TASK\nTask name: /root/helper_task\nSender: /root\nPayload:\n"
                .to_owned()
        )]);
    }

    /// An image generation's result is the image itself, never kept.
    #[rstest]
    fn an_image_generation_keeps_its_prompt_not_its_image() {
        let m = line(&serde_json::json!({"type": "response_item", "payload": {
            "type": "image_generation_call", "id": "ig_1", "status": "completed",
            "revised_prompt": "a cat", "result": "iVBORw0KGgo="}}));
        assert!(
            matches!(m.content().as_slice(), [Content::ToolUse(u)] if u.input == serde_json::json!("a cat"))
        );
    }

    /// Rollouts from 0.93.0 until names moved out of them recorded a thread's name as an event
    /// (codex-rs `ThreadNameUpdatedEvent`); a name for another thread (a subagent's event in its
    /// parent's rollout) is not this one's.
    #[rstest]
    #[case("th1", Some("Fix the flaky test"))]
    #[case("other", None)]
    fn a_thread_name_titles_the_session(#[case] thread: &str, #[case] expected: Option<&str>) {
        let mut m = line(&serde_json::json!({
            "timestamp": "2026-02-01T10:00:00.000Z", "type": "event_msg",
            "payload": {"type": "thread_name_updated", "thread_id": thread,
                "thread_name": "Fix the flaky test"},
        }));
        m.context.session = Some(SessionId::from("th1".to_owned()));
        assert_eq!(m.title().as_deref(), expected);
    }
}
