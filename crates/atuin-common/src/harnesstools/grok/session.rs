//! Grok Build sessions: `$GROK_HOME/sessions/<encoded cwd>/<session>/chat_history.jsonl`, paired
//! with `summary.json`, `updates.jsonl`, and `.atuin.json` in the same session directory.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::watch;
use typed_builder::TypedBuilder;

use super::Grok;
use crate::fs::tree_watcher::{FileStat, TreeWatcher};
use crate::harnesstools::resume;
use crate::harnesstools::session::model::{
    Content, MessageId, ParentKind, Role, StopReason, TitleChange, TitleSource, ToolCallId,
    ToolResult, ToolUse, Usage,
};
use crate::harnesstools::session::{
    Checkpoint, FilePatch, Listener, Message, MessageError, Observable, Patch, RuntimeError,
    Session, SessionId, Sessions, WatchError, checkpoint_offset, modified_after,
};
use crate::io::{FollowLines, PathLineReader, PooledReadLines};
use crate::json::jsonl::JsonlExt;
use crate::path::{AbsPathBuf, PathBufExt as _};
use crate::sync::BlockingPool;

/// Transcript filename inside each Grok Build session directory.
pub(crate) const TRANSCRIPT_FILE: &str = "chat_history.jsonl";

/// Summary metadata filename beside [`TRANSCRIPT_FILE`].
pub(crate) const SUMMARY_FILE: &str = "summary.json";

/// ACP update log beside [`TRANSCRIPT_FILE`], carrying per-event timestamps and TUI scrollback.
pub(crate) const UPDATES_FILE: &str = "updates.jsonl";

/// Grok Build's rewind points beside [`TRANSCRIPT_FILE`]: per turn, what each file it changed held
/// before the turn first touched it.
const REWINDS_FILE: &str = "rewind_points.jsonl";

/// Directory beside [`TRANSCRIPT_FILE`] where Grok Build keeps each compaction's request, holding
/// the whole transcript as it stood before the compaction replaced it.
const COMPACTIONS_DIR: &str = "compaction_requests";

/// `synthetic_reason` of the lines a compaction writes in place of the conversation it replaced:
/// the `<user_info>` preamble, then the summary.
const COMPACTION: &str = "compaction_meta";

/// Atuin sidecar file inside a session directory, preserving source-id mappings and merged-row
/// records across Grok Build's in-place rewrites of `chat_history.jsonl`.
pub(crate) const SIDECAR_FILE: &str = ".atuin.json";

/// Maximum byte length of an encoded workspace directory name before shortening with a hash
/// suffix, staying safely under the 255-byte component limit on APFS/ext4.
const MAX_WORKSPACE_SLUG_LEN: usize = 200;

/// Sidecar bookkeeping stored in `<session dir>/.atuin.json` for sessions Atuin wrote or caught
/// up. Grok Build rewrites `chat_history.jsonl` on `--resume` and strips unknown JSON keys on
/// transcript lines, while leaving sibling files in the session directory untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AtuinSidecar {
    /// Deterministic `gk-...` line key -> synced `source_id`, when they differ.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub ids: HashMap<String, String>,
    /// Synced `source_id` -> `source_id` of the written line it was merged into.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub merged: HashMap<String, String>,
}

impl AtuinSidecar {
    pub(crate) fn read_beside(transcript_path: &Path) -> Self {
        let path = transcript_path.with_file_name(SIDECAR_FILE);
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ids.is_empty() && self.merged.is_empty()
    }
}

#[derive(Debug, Clone, TypedBuilder)]
pub struct GrokSessions {
    #[builder(default, setter(strip_option, into))]
    root: Option<PathBuf>,
    /// Runs every file read of the sessions this finds.
    pool: BlockingPool,
}

impl GrokSessions {
    fn resolve_root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(default_root)
    }
}

/// Grok Build's sessions directory: `$GROK_HOME/sessions`, else `~/.grok/sessions`.
pub(crate) fn default_root() -> PathBuf {
    super::home().joined("sessions")
}

/// Encode `cwd` into the workspace directory name under `sessions/`.
///
/// Uses RFC 3986 unreserved byte encoding (`%2FUsers%2F...`). Paths whose encoded form exceeds
/// [`MAX_WORKSPACE_SLUG_LEN`] are shortened without splitting a `%XX` escape and suffixed with a
/// 64-bit hash so directory creation never fails with `ENAMETOOLONG`.
#[must_use]
pub(crate) fn encode_cwd(cwd: &Path) -> String {
    let normalized: PathBuf = cwd.components().collect();
    let raw = normalized.to_string_lossy();
    let mut encoded = String::with_capacity(raw.len() + 16);
    for byte in raw.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    if encoded.len() <= MAX_WORKSPACE_SLUG_LEN {
        return encoded;
    }
    let mut cut = MAX_WORKSPACE_SLUG_LEN - 17;
    if encoded.as_bytes().get(cut.saturating_sub(1)) == Some(&b'%') {
        cut = cut.saturating_sub(1);
    } else if encoded.as_bytes().get(cut.saturating_sub(2)) == Some(&b'%') {
        cut = cut.saturating_sub(2);
    }
    let digest = xxhash_rust::xxh3::xxh3_64(raw.as_bytes());
    format!("{}-{digest:016x}", &encoded[..cut])
}

/// Locate session `id`'s `chat_history.jsonl` under `root`.
pub(crate) fn locate(root: &Path, id: &str) -> Option<PathBuf> {
    if !resume::is_plain_name(id) {
        return None;
    }
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|ft| ft.is_dir()))
        .map(|workspace| workspace.path().joined(id).joined(TRANSCRIPT_FILE))
        .find(|path| path.is_file())
}

/// Scan `<root>/<workspace>/<session>/chat_history.jsonl` without descending into per-session
/// artifact subdirectories such as `terminal/` or `images/`.
fn scan_grok_sessions(root: &Path, pool: &BlockingPool) -> Vec<Result<GrokSession, RuntimeError>> {
    let mut out = Vec::new();
    let workspaces = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) => {
            out.push(Err(RuntimeError::Io(err)));
            return out;
        }
    };
    for workspace in workspaces {
        let workspace = match workspace {
            Ok(entry) => entry,
            Err(err) => {
                out.push(Err(RuntimeError::Io(err)));
                continue;
            }
        };
        let ws_type = match workspace.file_type() {
            Ok(ft) => ft,
            Err(err) => {
                out.push(Err(RuntimeError::Io(err)));
                continue;
            }
        };
        if !ws_type.is_dir() {
            continue;
        }
        let sessions = match std::fs::read_dir(workspace.path()) {
            Ok(entries) => entries,
            Err(err) => {
                out.push(Err(RuntimeError::Io(err)));
                continue;
            }
        };
        for entry in sessions {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    out.push(Err(RuntimeError::Io(err)));
                    continue;
                }
            };
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(err) => {
                    out.push(Err(RuntimeError::Io(err)));
                    continue;
                }
            };
            if !ft.is_dir() {
                continue;
            }
            let transcript = entry.path().joined(TRANSCRIPT_FILE);
            if let Some(session) =
                GrokListener::open_session(root, &transcript, transcript.is_file(), pool)
            {
                out.push(Ok(session));
            }
        }
    }
    out
}

impl Sessions for GrokSessions {
    type Listener = GrokListener;

    fn listener(&self) -> Result<GrokListener, RuntimeError> {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        Ok(GrokListener {
            root,
            pool: self.pool.clone(),
        })
    }

    fn existing(
        &self,
    ) -> Result<impl Stream<Item = Result<GrokSession, RuntimeError>> + Send + 'static, RuntimeError>
    {
        let root = self.resolve_root();
        if !root.is_dir() {
            return Err(RuntimeError::NotFound(root));
        }
        let pool = self.pool.clone();
        Ok(async_stream::stream! {
            let sessions_pool = pool.clone();
            let scan = pool
                .run(move || scan_grok_sessions(&root, &sessions_pool))
                .await;
            match scan {
                Ok(items) => {
                    for item in items {
                        yield item;
                    }
                }
                Err(cancelled) => yield Err(RuntimeError::Io(io::Error::other(cancelled))),
            }
        })
    }
}

impl Observable for Grok {
    type Sessions = GrokSessions;

    fn sessions(&self, pool: BlockingPool) -> GrokSessions {
        GrokSessions::builder().pool(pool).build()
    }
}

#[derive(Debug, Clone)]
pub struct GrokListener {
    root: PathBuf,
    pool: BlockingPool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WatchedKind {
    Transcript,
    Summary,
}

/// Classify `path` under `root` as `<workspace>/<session>/{chat_history.jsonl,summary.json}`.
fn watched_session_file(root: &Path, path: &Path) -> Option<WatchedKind> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts = relative.components();
    let (Some(workspace), Some(session), Some(file), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let workspace = workspace.as_os_str().to_str()?;
    let session = session.as_os_str().to_str()?;
    if workspace.is_empty() || !resume::is_plain_name(session) {
        return None;
    }
    if file.as_os_str() == TRANSCRIPT_FILE {
        Some(WatchedKind::Transcript)
    } else if file.as_os_str() == SUMMARY_FILE {
        Some(WatchedKind::Summary)
    } else {
        None
    }
}

impl GrokListener {
    fn session_id(path: &Path) -> Option<SessionId> {
        let dir = path.parent()?.file_name()?.to_str()?;
        resume::is_plain_name(dir).then(|| SessionId::from(dir.to_owned()))
    }

    fn open_session(
        root: &Path,
        path: &Path,
        is_file: bool,
        pool: &BlockingPool,
    ) -> Option<GrokSession> {
        if !is_file || watched_session_file(root, path) != Some(WatchedKind::Transcript) {
            return None;
        }
        Some(GrokSession::open(Self::session_id(path)?, path.to_path_buf(), pool.clone()))
    }
}

#[derive(Default)]
struct WatchedPair {
    transcript_path: Option<PathBuf>,
    transcript_stat: Option<watch::Receiver<FileStat>>,
    summary_stat: Option<watch::Receiver<FileStat>>,
}

impl Listener for GrokListener {
    type Session = GrokSession;

    fn watch(self) -> impl Stream<Item = Result<GrokSession, WatchError>> + Send + 'static {
        let root = self.root;
        let pool = self.pool;
        async_stream::stream! {
            let sessions_root =
                AbsPathBuf::canonicalize(&root).map_or_else(|_| root.clone(), PathBuf::from);
            let filter_root = sessions_root.clone();
            let files = TreeWatcher::builder()
                .filter(move |path| watched_session_file(&filter_root, path).is_some())
                .watch(&root);
            let files = match files {
                Ok(files) => files,
                Err(err) => {
                    yield Err(WatchError::from(err));
                    return;
                }
            };
            let mut pairs: HashMap<SessionId, WatchedPair> = HashMap::new();
            for await file in files {
                let (path, stat) = file.into_parts();
                let Some(kind) = watched_session_file(&sessions_root, &path) else { continue };
                let Some(id) = Self::session_id(&path) else { continue };
                let entry = pairs.entry(id.clone()).or_default();
                match kind {
                    WatchedKind::Transcript => {
                        entry.transcript_path = Some(path.to_path_buf());
                        entry.transcript_stat = Some(stat);
                    }
                    WatchedKind::Summary => {
                        entry.summary_stat = Some(stat);
                    }
                }
                if let (Some(transcript_path), Some(changes)) =
                    (entry.transcript_path.clone(), entry.transcript_stat.clone())
                {
                    yield Ok(GrokSession {
                        id,
                        path: transcript_path,
                        changes: Some(changes),
                        summary_changes: entry.summary_stat.clone(),
                        pool: pool.clone(),
                    });
                }
            }
        }
    }
}

/// Session-level metadata read from `summary.json` beside `chat_history.jsonl`.
#[derive(Debug, Clone, Default)]
pub(crate) struct GrokSummary {
    pub present: bool,
    pub cwd: Option<PathBuf>,
    pub created_at: Option<OffsetDateTime>,
    pub updated_at: Option<OffsetDateTime>,
    pub title: Option<TitleChange>,
    pub parent: Option<(SessionId, ParentKind)>,
}

/// Per-event timestamps extracted from `updates.jsonl` beside `chat_history.jsonl`.
#[derive(Debug, Clone, Default)]
pub(crate) struct UpdateTimes {
    pub prompts_by_index: HashMap<u64, OffsetDateTime>,
    pub prompts_in_order: Vec<OffsetDateTime>,
    pub call_starts: HashMap<String, OffsetDateTime>,
    pub call_ends: HashMap<String, OffsetDateTime>,
    pub turn_replies: Vec<OffsetDateTime>,
    pub latest: Option<OffsetDateTime>,
    /// Each turn's usage (`turn_completed`), by the prompt index that started it.
    pub usage: HashMap<u64, Usage>,
    /// Each turn's last reply, by the prompt index that started it.
    pub replies: HashMap<u64, OffsetDateTime>,
}

fn parse_unix_timestamp(raw: &Value) -> Option<OffsetDateTime> {
    let n = raw.as_i64()?;
    if n >= 10_000_000_000 {
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(n) * 1_000_000).ok()
    } else {
        OffsetDateTime::from_unix_timestamp(n).ok()
    }
}

/// When an `updates.jsonl` event happened: Grok Build's milliseconds (`params._meta`), else its
/// whole seconds.
fn event_timestamp(entry: &Value) -> Option<OffsetDateTime> {
    parse_unix_timestamp(&entry["params"]["_meta"]["agentTimestampMs"])
        .or_else(|| parse_unix_timestamp(&entry["timestamp"]))
}

type FileSignature = Option<(std::time::SystemTime, u64)>;

fn file_signature(path: &Path) -> FileSignature {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Read `updates.jsonl` beside `transcript_path` for per-message timestamps.
pub(crate) fn read_update_times_beside(transcript_path: &Path) -> UpdateTimes {
    let updates_path = transcript_path.with_file_name(UPDATES_FILE);
    let Ok(file) = File::open(updates_path) else {
        return UpdateTimes::default();
    };
    let mut times = UpdateTimes::default();
    let mut current_turn: Option<usize> = None;
    let mut current_prompt: Option<u64> = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = crate::json::js::from_slice::<Value>(line.as_bytes()) else {
            continue;
        };
        let Some(ts) = event_timestamp(&entry) else {
            continue;
        };
        times.latest = Some(times.latest.map_or(ts, |prev| prev.max(ts)));
        let update = &entry["params"]["update"];
        match update["sessionUpdate"].as_str().unwrap_or_default() {
            "user_message_chunk" => {
                if let Some(idx) = update["_meta"]["promptIndex"].as_u64() {
                    times.prompts_by_index.entry(idx).or_insert(ts);
                    current_prompt = Some(idx);
                }
                times.prompts_in_order.push(ts);
                let turn_idx = times.prompts_in_order.len() - 1;
                if times.turn_replies.len() <= turn_idx {
                    times.turn_replies.resize(turn_idx + 1, ts);
                }
                current_turn = Some(turn_idx);
            }
            "tool_call" => {
                if let Some(call_id) = update["toolCallId"].as_str().filter(|s| !s.is_empty()) {
                    times.call_starts.entry(call_id.to_owned()).or_insert(ts);
                }
            }
            "tool_call_update" => {
                if let Some(call_id) = update["toolCallId"].as_str().filter(|s| !s.is_empty()) {
                    times.call_starts.entry(call_id.to_owned()).or_insert(ts);
                    times.call_ends.insert(call_id.to_owned(), ts);
                }
            }
            "turn_completed" if update["usage"].is_object() => {
                if let Some(idx) = current_prompt {
                    times.usage.insert(idx, usage_of(&update["usage"]));
                }
                if let Some(idx) = current_prompt {
                    times.replies.insert(idx, ts);
                }
            }
            "agent_message_chunk" | "turn_completed" => {
                if let Some(idx) = current_prompt {
                    times.replies.insert(idx, ts);
                }
                if let Some(turn_idx) = current_turn {
                    if times.turn_replies.len() <= turn_idx {
                        times.turn_replies.resize(turn_idx + 1, ts);
                    } else {
                        times.turn_replies[turn_idx] = ts;
                    }
                } else if times.turn_replies.is_empty() {
                    times.turn_replies.push(ts);
                } else if let Some(last) = times.turn_replies.last_mut() {
                    *last = ts;
                }
            }
            _ => {}
        }
    }
    times
}

/// A turn's usage as Grok Build reports it, whose `inputTokens` include the cached ones.
fn usage_of(usage: &Value) -> Usage {
    let field = |name: &str| usage[name].as_u64();
    let (cache_read, cache_write) = (field("cachedReadTokens"), field("cacheCreationTokens"));
    Usage {
        input: field("inputTokens").map(|input| {
            input.saturating_sub(cache_read.unwrap_or(0)).saturating_sub(cache_write.unwrap_or(0))
        }),
        output: field("outputTokens"),
        cache_read,
        cache_write,
        reasoning: field("reasoningTokens"),
    }
}

/// What each turn's files held before the turn first touched them (`rewind_points.jsonl`): by the
/// turn's prompt index, then the file's path as Grok Build names it (relative to the session's
/// directory); `None` for a file that didn't exist.
type Snapshots = HashMap<u64, HashMap<String, Option<String>>>;

fn read_snapshots_beside(transcript_path: &Path) -> Snapshots {
    let Ok(text) = std::fs::read_to_string(transcript_path.with_file_name(REWINDS_FILE)) else {
        return Snapshots::default();
    };
    text.lines()
        .filter_map(|line| crate::json::js::from_slice::<Value>(line.as_bytes()).ok())
        .filter_map(|point| {
            let files = point["file_snapshots"].as_object()?.iter().map(|(path, snapshot)| {
                (path.clone(), snapshot["content"].as_str().map(str::to_owned))
            });
            Some((point["prompt_index"].as_u64()?, files.collect()))
        })
        .collect()
}

/// The transcripts Grok Build replaced with each compaction of the session at `transcript_path`
/// (`compaction_requests/*.json`), oldest first. Read with the transcript, they give back what a
/// session compacted before it was captured said.
pub(crate) fn compacted_beside(transcript_path: &Path) -> Vec<Vec<Value>> {
    let Ok(entries) = std::fs::read_dir(transcript_path.with_file_name(COMPACTIONS_DIR)) else {
        return Vec::new();
    };
    let mut requests: Vec<(String, Vec<Value>)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let bytes = std::fs::read(entry.path()).ok()?;
            let mut request = crate::json::js::from_slice::<Value>(&bytes).ok()?;
            let at = request["created_at"].as_str()?.to_owned();
            let Value::Array(mut lines) = request["chat_history"].take() else {
                return None;
            };
            // The request ends with the instruction to summarise, no line of the conversation.
            if lines
                .last()
                .is_some_and(|line| line["type"] == "user" && line["prompt_index"].is_null())
            {
                lines.pop();
            }
            Some((at, lines))
        })
        .collect();
    requests.sort_by(|a, b| a.0.cmp(&b.0));
    requests.into_iter().map(|(_, lines)| lines).collect()
}

/// Read `summary.json` beside `transcript_path`, returning default metadata when absent or
/// unreadable.
pub(crate) fn read_summary_beside(transcript_path: &Path) -> GrokSummary {
    let summary_path = transcript_path.with_file_name(SUMMARY_FILE);
    let Ok(bytes) = std::fs::read(summary_path) else {
        return GrokSummary::default();
    };
    let Ok(raw) = serde_json::from_slice::<Value>(&bytes) else {
        return GrokSummary::default();
    };
    let cwd = raw["info"]["cwd"].as_str().filter(|s| !s.is_empty()).map(PathBuf::from);
    let created_at =
        raw["created_at"].as_str().and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());
    let updated_at = raw["last_active_at"]
        .as_str()
        .or_else(|| raw["updated_at"].as_str())
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .or(created_at);
    let title = raw["generated_title"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|t| TitleChange::new(TitleSource::Generated, t))
        .or_else(|| {
            raw["session_summary"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|t| TitleChange::new(TitleSource::Summary, t))
        });
    let parent = raw["parent_session_id"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(
        |parent_id| {
            let kind = if raw["session_kind"].as_str().is_some_and(|k| k.starts_with("subagent")) {
                ParentKind::Subagent
            } else {
                ParentKind::Fork
            };
            (SessionId::from(parent_id.to_owned()), kind)
        },
    );
    GrokSummary {
        present: true,
        cwd,
        created_at,
        updated_at,
        title,
        parent,
    }
}

/// Deterministic id for session `session_id`'s metadata header row (`atuin-meta-<session>`).
#[must_use]
pub(crate) fn meta_id(session_id: &str) -> String {
    format!("atuin-meta-{session_id}")
}

/// Deterministic id for a title-change info row (`atuin-title-<digest>`), never embedding the
/// unredacted title text itself in the synced `source_id`.
#[must_use]
pub(crate) fn title_id(session_id: &str, change: &TitleChange) -> String {
    let payload = json!([session_id, change.source, change.text]);
    let digest = xxhash_rust::xxh3::xxh3_64(payload.to_string().as_bytes());
    format!("atuin-title-{digest:016x}")
}

/// Deterministic prefix-chained content key (`gk-<digest>`) for a conversational line, derived
/// from `prev_gk` (the previous conversational line's `gk-` key) plus the fields Grok Build
/// preserves verbatim when rewriting `chat_history.jsonl` on resume.
///
/// Chaining `prev_gk` gives repeated prompts (such as `"continue"` or `"ok"`) distinct keys both
/// within a single transcript and across diverged branches on different machines, while staying
/// invariant when Grok prepends its `"system"` prompt on `--resume`.
pub(crate) fn line_key(prev_gk: Option<&str>, role: &Role, content: &[Content]) -> Option<String> {
    if content.is_empty() {
        return None;
    }
    let canonical = json!([prev_gk, role, content]);
    let hash = xxhash_rust::xxh3::xxh3_64(canonical.to_string().as_bytes());
    Some(format!("gk-{hash:016x}"))
}

/// Keys a transcript's conversational lines in order (see [`line_key`]). It also marks the prompts
/// a compaction repeats between its `<user_info>` preamble and its summary as replayed: context
/// the model is given again, not prompts sent twice.
#[derive(Debug, Default, Clone)]
pub(crate) struct Chain {
    last: Option<String>,
    compacting: bool,
}

impl Chain {
    /// The key of `msg`, the transcript's next line; `None` for a line with no content.
    pub(crate) fn key(&mut self, msg: &mut GrokMessage) -> Option<String> {
        if msg.synthetic_reason.as_deref() == Some(COMPACTION) {
            self.compacting = !msg.is_compaction_summary();
        } else if self.compacting && msg.kind == "user" && msg.prompt_index.is_none() {
            msg.replayed = true;
        }
        // What an edit changed is read from beside the transcript, when it's there: not the line.
        let content: Vec<_> =
            msg.content().into_iter().filter(|c| !matches!(c, Content::Patch(_))).collect();
        let key = line_key(self.last.as_deref(), &msg.role(), &content)?;
        self.last = Some(key.clone());
        Some(key)
    }

    /// The key of the last conversational line keyed.
    pub(crate) fn last(&self) -> Option<&str> {
        self.last.as_deref()
    }
}

/// How a transcript read again from its start joins what was read before. Grok Build rewrites it
/// on `--resume`, `atuin threads resume --switch` replaces it, and a compaction replaces it with a
/// summary. The capture of a read starts over ([`Checkpoint::generation`]), so its first line not
/// read before would follow nothing: it follows the last line of the new read that was read
/// before, or else, when a compaction kept none, the line the read before ended on.
#[derive(Debug, Default)]
struct Rejoin {
    read: std::collections::HashSet<String>,
    last: Option<String>,
    from: Option<String>,
    rejoining: bool,
}

impl Rejoin {
    /// The transcript is read from its start again.
    fn again(&mut self) {
        if !self.rejoining {
            self.from.clone_from(&self.last);
        }
        self.rejoining = true;
    }

    fn line(&mut self, msg: &mut GrokMessage) {
        let Some(id) = msg.computed_id.clone() else {
            return;
        };
        if !self.read.insert(id.clone()) {
            if !self.rejoining {
                self.last = Some(id);
            // A compaction's `<user_info>` is read before when the session's first was the same,
            // and the prompt it repeats when an earlier compaction repeated it too.
            } else if msg.synthetic_reason.as_deref() != Some(COMPACTION) && !msg.replayed {
                self.from = Some(id);
            }
            return;
        }
        if std::mem::take(&mut self.rejoining) {
            msg.follows_id.clone_from(&self.from);
        }
        self.last = Some(id);
    }
}

/// Per-session state carried across lines without cloning growing maps on each line.
#[derive(Debug, Default)]
struct ReaderState {
    summary: GrokSummary,
    times: UpdateTimes,
    sidecar: AtuinSidecar,
    summary_sig: FileSignature,
    updates_sig: FileSignature,
    sidecar_sig: FileSignature,
    snapshots: Snapshots,
    snapshots_sig: FileSignature,
    /// What the turn's edits so far left each file holding, by its snapshot's path, and the
    /// edits not yet answered, by call id.
    edited: HashMap<String, Option<String>>,
    edits: HashMap<String, GrokToolCall>,
    chain: Chain,
    prompt_seq: usize,
    current_turn: Option<usize>,
    /// The prompt index of the turn being read, and the turns whose usage a reply carries.
    prompt: Option<u64>,
    usage_given: std::collections::HashSet<u64>,
    /// The ids of the lines before the checkpoint a read starts from.
    replayed: Vec<String>,
    last_ts: Option<OffsetDateTime>,
    /// The time of the line before, which no line is timed before.
    previous_ts: Option<OffsetDateTime>,
    emitted_title: Option<TitleChange>,
    last_chain_id: Option<String>,
}

impl ReaderState {
    fn load(session_id: &str, transcript_path: &Path, start_offset: u64) -> Self {
        let summary_sig = file_signature(&transcript_path.with_file_name(SUMMARY_FILE));
        let updates_sig = file_signature(&transcript_path.with_file_name(UPDATES_FILE));
        let sidecar_sig = file_signature(&transcript_path.with_file_name(SIDECAR_FILE));
        let snapshots_sig = file_signature(&transcript_path.with_file_name(REWINDS_FILE));
        let snapshots = read_snapshots_beside(transcript_path);
        let summary = read_summary_beside(transcript_path);
        let times = read_update_times_beside(transcript_path);
        let sidecar = AtuinSidecar::read_beside(transcript_path);
        let fallback_mtime = std::fs::metadata(transcript_path)
            .and_then(|m| m.modified())
            .ok()
            .map(OffsetDateTime::from);
        let initial_chain = summary.present.then(|| meta_id(session_id));
        // On a checkpointed restart (`start_offset > 0`) `emitted_header` is skipped, so leave
        // `emitted_title` unset here and let `take_pending_title()` emit any title generated while
        // capture was stopped (SQLite deduplicates an unchanged `atuin-title-*` idempotently).
        let emitted_title = (start_offset == 0).then(|| summary.title.clone()).flatten();
        let mut state = Self {
            last_ts: summary.created_at.or(times.latest).or(fallback_mtime),
            previous_ts: None,
            emitted_title,
            summary,
            times,
            sidecar,
            summary_sig,
            updates_sig,
            sidecar_sig,
            snapshots,
            snapshots_sig,
            edited: HashMap::new(),
            edits: HashMap::new(),
            chain: Chain::default(),
            prompt_seq: 0,
            current_turn: None,
            prompt: None,
            usage_given: std::collections::HashSet::new(),
            replayed: Vec::new(),
            last_chain_id: initial_chain,
        };
        if start_offset > 0
            && let Ok(file) = File::open(transcript_path)
        {
            let mut reader = BufReader::new(file);
            let mut pos = 0u64;
            let mut buf = Vec::new();
            while pos < start_offset {
                buf.clear();
                let Ok(n) = reader.read_until(b'\n', &mut buf) else {
                    break;
                };
                if n == 0 {
                    break;
                }
                pos += n as u64;
                if pos > start_offset {
                    break;
                }
                if let Ok(mut msg) = GrokMessage::decode(&buf)
                    && let Some(id) = state.enrich(&mut msg)
                {
                    state.replayed.push(id);
                }
            }
        }
        state
    }

    /// The messages of a transcript a compaction replaced (see [`compacted_beside`]), read from
    /// its start as it was then. The reader restarts after it, for the file itself.
    fn compacted(&mut self, lines: Vec<Value>, rejoin: &mut Rejoin) -> Vec<GrokMessage> {
        self.restart();
        rejoin.again();
        let mut out = Vec::new();
        for line in lines {
            let Ok(mut message) = serde_json::from_value::<GrokMessage>(line) else {
                continue;
            };
            if self.enrich(&mut message).is_some() {
                rejoin.line(&mut message);
                out.push(message);
            }
        }
        self.restart();
        rejoin.again();
        out
    }

    /// The session's metadata row and its title, from `summary.json`, a read from the start
    /// begins with.
    fn header(&self, session_id: &str) -> Vec<GrokMessage> {
        if !self.summary.present {
            return Vec::new();
        }
        let title = self.summary.title.clone().map(|change| {
            let ts = self.summary.created_at.or(self.summary.updated_at);
            GrokMessage::title_info(session_id, change, ts)
        });
        std::iter::once(GrokMessage::session_meta(session_id, &self.summary)).chain(title).collect()
    }

    fn take_pending_title(&mut self) -> Option<TitleChange> {
        if self.summary.title == self.emitted_title {
            None
        } else {
            self.emitted_title.clone_from(&self.summary.title);
            self.summary.title.clone()
        }
    }

    fn refresh_files(&mut self, transcript_path: &Path) -> Option<TitleChange> {
        let next_updates = file_signature(&transcript_path.with_file_name(UPDATES_FILE));
        if next_updates != self.updates_sig {
            self.updates_sig = next_updates;
            self.times = read_update_times_beside(transcript_path);
        }
        let next_snapshots = file_signature(&transcript_path.with_file_name(REWINDS_FILE));
        if next_snapshots != self.snapshots_sig {
            self.snapshots_sig = next_snapshots;
            self.snapshots = read_snapshots_beside(transcript_path);
        }
        let next_sidecar = file_signature(&transcript_path.with_file_name(SIDECAR_FILE));
        if next_sidecar != self.sidecar_sig {
            self.sidecar_sig = next_sidecar;
            self.sidecar = AtuinSidecar::read_beside(transcript_path);
        }
        let next_summary = file_signature(&transcript_path.with_file_name(SUMMARY_FILE));
        if next_summary == self.summary_sig {
            return None;
        }
        self.summary_sig = next_summary;
        self.summary = read_summary_beside(transcript_path);
        self.take_pending_title()
    }

    /// The change edit `call` made to its file, replayed on what the turn's snapshot and its edits
    /// before say the file held. `None` when they don't say (a file outside the session's
    /// directory), or the edit doesn't apply (the file was changed some other way).
    fn replay(&mut self, call: &GrokToolCall) -> Option<Patch> {
        let input = parse_tool_arguments(&call.arguments);
        let path = input["file_path"].as_str()?;
        let files = self.snapshots.get(&self.prompt?)?;
        let relative =
            self.summary.cwd.as_deref().and_then(|cwd| Path::new(path).strip_prefix(cwd).ok());
        let key = [relative.and_then(Path::to_str), Some(path)]
            .into_iter()
            .flatten()
            .find(|key| files.contains_key(*key) || self.edited.contains_key(*key))?
            .to_owned();
        let before = match self.edited.get(&key) {
            Some(edited) => edited.clone(),
            None => files.get(&key)?.clone(),
        };
        let after = if call.name == "write" {
            input["content"].as_str()?.to_owned()
        } else {
            let (old, new) = (input["old_string"].as_str()?, input["new_string"].as_str()?);
            let text = before.as_deref().filter(|text| !old.is_empty() && text.contains(old))?;
            if input["replace_all"].as_bool() == Some(true) {
                text.replace(old, new)
            } else {
                text.replacen(old, new, 1)
            }
        };
        let file = FilePatch::between(path.to_owned(), before.as_deref(), &after);
        self.edited.insert(key, Some(after));
        Some(Patch {
            call: ToolCallId::from(call.id.clone()),
            files: vec![file],
        })
    }

    /// Start reading another of the session's transcripts (one a compaction replaced, or the
    /// file rewritten), whose lines key from the start again.
    fn restart(&mut self) {
        self.chain = Chain::default();
        self.edited.clear();
        self.edits.clear();
        self.prompt_seq = 0;
        self.current_turn = None;
        self.prompt = None;
    }

    fn enrich(&mut self, msg: &mut GrokMessage) -> Option<String> {
        let gk = self.chain.key(msg)?;
        let role = msg.role();
        let resolved_id = self.sidecar.ids.get(&gk).cloned().unwrap_or(gk);
        // Lines are in the order they happened: a line timed before the one above it (a reply
        // timed by the turn's end, say) takes that line's time instead.
        let ts = self
            .timestamp_for(msg, &role)
            .map(|ts| self.previous_ts.map_or(ts, |previous| ts.max(previous)));
        if let Some(ts) = ts {
            self.last_ts = Some(ts);
            self.previous_ts = Some(ts);
        }
        if role == Role::User {
            self.prompt = msg.prompt_index;
            self.edited.clear();
        }
        for call in msg
            .tool_calls
            .iter()
            .filter(|call| matches!(call.name.as_str(), "search_replace" | "write"))
        {
            self.edits.insert(call.id.clone(), call.clone());
        }
        if let Some(call) = msg.tool_call_id.as_ref().and_then(|id| self.edits.remove(id))
            && !inferred_tool_error(&msg.content)
        {
            msg.patch = self.replay(&call);
        }
        if role != Role::User
            && msg.stop_reason() == Some(StopReason::EndTurn)
            && let Some(prompt) = self.prompt
            && let Some(usage) = self.times.usage.get(&prompt)
            && self.usage_given.insert(prompt)
        {
            msg.usage = Some(*usage);
        }
        msg.computed_id = Some(resolved_id.clone());
        msg.computed_timestamp = ts.or(self.last_ts);
        self.last_chain_id = Some(resolved_id.clone());
        Some(resolved_id)
    }

    fn timestamp_for(&mut self, msg: &GrokMessage, role: &Role) -> Option<OffsetDateTime> {
        match role {
            Role::User => {
                let seq = self.prompt_seq;
                self.prompt_seq += 1;
                self.current_turn = Some(seq);
                msg.prompt_index
                    .and_then(|idx| self.times.prompts_by_index.get(&idx).copied())
                    .or_else(|| self.times.prompts_in_order.get(seq).copied())
                    .or(self.summary.updated_at)
            }
            // Thinking is timed by nothing of its own: it follows the line before it.
            Role::Assistant if msg.kind == "reasoning" => self.previous_ts.or(self.last_ts),
            Role::Assistant => {
                if let Some(first_call) = msg.tool_calls.first()
                    && let Some(&ts) = self.times.call_starts.get(&first_call.id)
                {
                    return Some(ts);
                }
                self.prompt
                    .and_then(|prompt| self.times.replies.get(&prompt).copied())
                    .or_else(|| {
                        self.current_turn
                            .and_then(|turn| self.times.turn_replies.get(turn).copied())
                    })
                    .or(self.times.latest)
                    .or(self.summary.updated_at)
            }
            Role::Tool => msg
                .tool_call_id
                .as_deref()
                .and_then(|id| self.times.call_ends.get(id).copied())
                .or(self.times.latest)
                .or(self.summary.updated_at),
            Role::System | Role::Other(_) => self.last_ts.or(self.summary.created_at),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GrokSession {
    id: SessionId,
    path: PathBuf,
    changes: Option<watch::Receiver<FileStat>>,
    summary_changes: Option<watch::Receiver<FileStat>>,
    pool: BlockingPool,
}

impl GrokSession {
    #[must_use]
    pub fn open(id: SessionId, path: PathBuf, pool: BlockingPool) -> Self {
        Self {
            id,
            path,
            changes: None,
            summary_changes: None,
            pool,
        }
    }

    async fn load_state(&self, start_offset: u64) -> ReaderState {
        let id = self.id.to_string();
        let path = self.path.clone();
        self.pool.run(move || ReaderState::load(&id, &path, start_offset)).await.unwrap_or_default()
    }
}

impl Session for GrokSession {
    type Message = GrokMessage;

    fn id(&self) -> SessionId {
        self.id.clone()
    }

    fn messages_from(
        self,
        from: Option<Checkpoint>,
    ) -> impl Stream<Item = Result<(Checkpoint, GrokMessage), MessageError>> + Send + 'static {
        async_stream::stream! {
            let probe = self.clone();
            let start = match from {
                Some(from) => checkpoint_offset(&self.path, &self.pool, from).await,
                None => 0,
            };
            let over = from.is_some() && start == 0;
            // Synthetic metadata/title events carry `safe_checkpoint` (the last fully stored line's
            // checkpoint, or `Checkpoint::default()` at start), never the checkpoint of an
            // unyielded line, so a crash mid-line never skips the line's message on restart.
            let mut safe_checkpoint = match from {
                Some(cp) if start > 0 => cp,
                _ => Checkpoint::new(0, b"").started_over(over),
            };
            let mut state = Some(probe.load_state(start).await);
            let mut rejoin = Rejoin::default();
            if let Some(st) = state.as_mut() {
                rejoin.read.extend(std::mem::take(&mut st.replayed));
                rejoin.last.clone_from(&st.last_chain_id);
            }
            let mut need_follows = start > 0;
            if start == 0 {
                let header = state.as_ref().map(|st| st.header(probe.id.as_ref())).unwrap_or_default();
                for message in header {
                    yield Ok((safe_checkpoint, message));
                }
                let path = probe.path.clone();
                let compacted =
                    probe.pool.run(move || compacted_beside(&path)).await.unwrap_or_default();
                if let Some(st) = state.as_mut() {
                    for lines in compacted {
                        for message in st.compacted(lines, &mut rejoin) {
                            yield Ok((safe_checkpoint, message));
                        }
                    }
                }
            }
            let mut last_generation = 0;
            let live = self.changes.is_some();
            let mut summary_rx = self.summary_changes;

            if start > 0
                && let Some(st) = state.as_mut()
                && let Some(change) = st.take_pending_title()
            {
                let ts = st.summary.updated_at.or(st.last_ts);
                yield Ok((
                    safe_checkpoint,
                    GrokMessage::title_info(probe.id.as_ref(), change, ts),
                ));
            }

            let lines = FollowLines::new(PooledReadLines::new(
                PathLineReader::at(&self.path, start),
                self.pool,
            ));
            let mut messages = match self.changes {
                Some(changes) => lines.follow(changes).left_stream(),
                None => lines.read_to_end().right_stream(),
            }
            .json_with(GrokMessage::decode)
            .map(|r| r.map_err(MessageError::from))
            .boxed();

            loop {
                // `None` means `summary.json` changed while `messages` had no line ready;
                // `Some(item)` is the next item (or EOF) from `messages`.
                let event = match summary_rx.as_mut() {
                    Some(rx) => {
                        tokio::select! {
                            biased;
                            item = messages.next() => Some(item),
                            changed = rx.changed() => {
                                if changed.is_err() {
                                    summary_rx = None;
                                    Some(messages.next().await)
                                } else {
                                    None
                                }
                            }
                        }
                    }
                    None => Some(messages.next().await),
                };

                match event {
                    None => {
                        if let Some(mut st) = state.take() {
                            let path = probe.path.clone();
                            if let Ok((returned, late_title)) = probe
                                .pool
                                .run(move || {
                                    let t = st.refresh_files(&path);
                                    (st, t)
                                })
                                .await
                            {
                                let ts = returned.summary.updated_at.or(returned.last_ts);
                                state = Some(returned);
                                if let Some(change) = late_title
                                {
                                    yield Ok((
                                        safe_checkpoint,
                                        GrokMessage::title_info(probe.id.as_ref(), change, ts),
                                    ));
                                }
                            }
                        }
                    }
                    Some(None) => break,
                    Some(Some(Err(err))) => yield Err(err),
                    Some(Some(Ok((line, mut message)))) => {
                        if line.generation != last_generation {
                            last_generation = line.generation;
                            need_follows = false;
                            safe_checkpoint = Checkpoint {
                                generation: line.generation,
                                ..Checkpoint::new(0, b"")
                            }
                            .started_over(over);
                            let mut st = probe.load_state(0).await;
                            for message in st.header(probe.id.as_ref()) {
                                yield Ok((safe_checkpoint, message));
                            }
                            rejoin.again();
                            let path = probe.path.clone();
                            let compacted =
                                probe.pool.run(move || compacted_beside(&path)).await.unwrap_or_default();
                            for lines in compacted {
                                for message in st.compacted(lines, &mut rejoin) {
                                    yield Ok((safe_checkpoint, message));
                                }
                            }
                            state = Some(st);
                        } else if live
                            && let Some(mut st) = state.take()
                        {
                            let path = probe.path.clone();
                            if let Ok((returned, late_title)) = probe
                                .pool
                                .run(move || {
                                    let t = st.refresh_files(&path);
                                    (st, t)
                                })
                                .await
                            {
                                let ts = returned.summary.updated_at.or(returned.last_ts);
                                state = Some(returned);
                                if let Some(change) = late_title
                                {
                                    yield Ok((
                                        safe_checkpoint,
                                        GrokMessage::title_info(probe.id.as_ref(), change, ts),
                                    ));
                                }
                            }
                        }

                        if let Some(st) = state.as_mut() {
                            let prior_chain =
                                need_follows.then(|| st.last_chain_id.clone()).flatten();
                            if st.enrich(&mut message).is_some() {
                                if need_follows {
                                    message.follows_id = prior_chain;
                                    need_follows = false;
                                }
                                rejoin.line(&mut message);
                            }
                        }
                        let checkpoint = Checkpoint::after(&line).started_over(over);
                        yield Ok((checkpoint, message));
                        safe_checkpoint = checkpoint;
                    }
                }
            }
        }
    }

    fn read(&self) -> impl Stream<Item = Result<GrokMessage, MessageError>> + Send + 'static {
        let session = self.clone();
        async_stream::stream! {
            let mut state = session.load_state(0).await;
            for message in state.header(session.id.as_ref()) {
                yield Ok(message);
            }
            let mut rejoin = Rejoin::default();
            let path = session.path.clone();
            let compacted =
                session.pool.run(move || compacted_beside(&path)).await.unwrap_or_default();
            for lines in compacted {
                for message in state.compacted(lines, &mut rejoin) {
                    yield Ok(message);
                }
            }
            let messages = FollowLines::new(PooledReadLines::new(
                PathLineReader::new(&session.path),
                session.pool,
            ))
            .read_to_end()
            .json_with(GrokMessage::decode);
            for await item in messages {
                match item {
                    Ok((_line, mut message)) => {
                        if state.enrich(&mut message).is_some() {
                            rejoin.line(&mut message);
                            yield Ok(message);
                        }
                    }
                    Err(err) => yield Err(MessageError::from(err)),
                }
            }
        }
    }

    async fn changed_after(mut self, since: std::time::SystemTime) -> Option<Self> {
        let transcript_changed = modified_after(self.changes.as_mut(), since);
        match self.summary_changes.as_mut() {
            Some(summary_rx) => {
                let summary_changed = modified_after(Some(summary_rx), since);
                tokio::select! {
                    res = transcript_changed => { res?; }
                    res = summary_changed => { res?; }
                }
            }
            None => {
                transcript_changed.await?;
            }
        }
        Some(self)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GrokToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

/// One line of a Grok Build `chat_history.jsonl` transcript, or a synthetic session/title
/// metadata record derived from `summary.json`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GrokMessage {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    content: Value,
    #[serde(default)]
    tool_calls: Vec<GrokToolCall>,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    model_id: Option<String>,
    #[serde(default)]
    prompt_index: Option<u64>,
    #[serde(default)]
    synthetic_reason: Option<String>,
    /// A prompt a compaction repeats (see [`Chain`]).
    #[serde(skip)]
    replayed: bool,
    /// What the edit this line answers changed (see [`ReaderState::replay`]).
    #[serde(skip)]
    patch: Option<Patch>,
    #[serde(skip)]
    usage: Option<Usage>,
    #[serde(skip)]
    computed_id: Option<String>,
    #[serde(skip)]
    parent_override: Option<String>,
    #[serde(skip)]
    follows_id: Option<String>,
    #[serde(skip)]
    computed_timestamp: Option<OffsetDateTime>,
    #[serde(skip)]
    meta_cwd: Option<PathBuf>,
    #[serde(skip)]
    meta_title: Option<TitleChange>,
    #[serde(skip)]
    meta_parent: Option<(SessionId, ParentKind)>,
}

impl GrokMessage {
    pub(crate) fn decode(bytes: &[u8]) -> serde_json::Result<Self> {
        crate::json::js::from_slice(bytes)
    }

    #[must_use]
    pub(crate) fn session_meta(session_id: &str, summary: &GrokSummary) -> Self {
        Self {
            kind: "system".to_owned(),
            computed_id: Some(meta_id(session_id)),
            computed_timestamp: summary.created_at.or(summary.updated_at),
            meta_cwd: summary.cwd.clone(),
            meta_parent: summary.parent.clone(),
            ..Self::default()
        }
    }

    #[must_use]
    pub(crate) fn title_info(
        session_id: &str,
        change: TitleChange,
        at: Option<OffsetDateTime>,
    ) -> Self {
        let id = title_id(session_id, &change);
        Self {
            kind: "system".to_owned(),
            computed_id: Some(id),
            parent_override: Some(meta_id(session_id)),
            computed_timestamp: at,
            meta_title: Some(change),
            ..Self::default()
        }
    }

    fn text_blocks(&self) -> Vec<String> {
        match &self.content {
            Value::String(s) if !s.is_empty() => vec![s.clone()],
            Value::Array(items) => items
                .iter()
                .filter_map(|item| match item {
                    Value::String(s) if !s.is_empty() => Some(s.clone()),
                    Value::Object(map) => map
                        .get("text")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn prompt_index(&self) -> Option<u64> {
        self.prompt_index
    }

    /// The summary a compaction leaves in place of the conversation, after its `<user_info>`.
    fn is_compaction_summary(&self) -> bool {
        self.synthetic_reason.as_deref() == Some(COMPACTION)
            && !self
                .text_blocks()
                .first()
                .is_some_and(|t| t.trim_start().starts_with("<user_info>"))
    }

    fn is_injected_user(&self, texts: &[String]) -> bool {
        if self.synthetic_reason.is_some() || self.replayed {
            return true;
        }
        if self.prompt_index.is_some() {
            return false;
        }
        texts.first().is_some_and(|first| {
            let trimmed = first.trim_start();
            trimmed.starts_with("<user_info>") || trimmed.starts_with("<system-reminder>")
        })
    }
}

/// Unwrap `<user_query>\n...\n</user_query>` that Grok Build wraps typed prompts in.
fn unwrap_user_query(text: &str) -> String {
    if let Some(inner) =
        text.strip_prefix("<user_query>\n").and_then(|s| s.strip_suffix("\n</user_query>"))
    {
        return inner.to_owned();
    }
    if let Some(inner) =
        text.strip_prefix("<user_query>").and_then(|s| s.strip_suffix("</user_query>"))
    {
        return inner.to_owned();
    }
    text.to_owned()
}

fn parse_tool_arguments(raw: &Value) -> Value {
    match raw {
        Value::String(text) => {
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
        }
        other => other.clone(),
    }
}

pub(crate) fn inferred_tool_error(output: &Value) -> bool {
    let Some(text) = output.as_str() else {
        return false;
    };
    if let Some(rest) = text.strip_prefix("exit: ") {
        let code = rest.lines().next().unwrap_or_default().trim();
        return code.parse::<i64>().is_ok_and(|n| n != 0);
    }
    text.starts_with("Error: ") || text.starts_with("Failed to parse arguments")
}

impl Message for GrokMessage {
    fn id(&self) -> Option<MessageId> {
        self.computed_id.as_ref().filter(|s| !s.is_empty()).cloned().map(MessageId::from)
    }

    fn parent_id(&self) -> Option<MessageId> {
        self.parent_override.as_ref().filter(|s| !s.is_empty()).cloned().map(MessageId::from)
    }

    fn follows(&self) -> Option<MessageId> {
        self.follows_id.as_ref().filter(|s| !s.is_empty()).cloned().map(MessageId::from)
    }

    fn role(&self) -> Role {
        match self.kind.as_str() {
            "system" => Role::System,
            "user" => {
                if self.is_injected_user(&self.text_blocks()) {
                    Role::System
                } else {
                    Role::User
                }
            }
            "assistant" | "reasoning" => Role::Assistant,
            "tool_result" => Role::Tool,
            other => Role::Other(other.to_owned()),
        }
    }

    fn timestamp(&self) -> Option<OffsetDateTime> {
        self.computed_timestamp
    }

    fn content(&self) -> Vec<Content> {
        match self.kind.as_str() {
            "system" => Vec::new(),
            "user" if self.is_compaction_summary() => {
                self.text_blocks().into_iter().map(Content::Summary).collect()
            }
            "user" => {
                let texts = self.text_blocks();
                if self.is_injected_user(&texts) {
                    texts.into_iter().map(Content::Text).collect()
                } else {
                    texts.into_iter().map(|t| Content::Text(unwrap_user_query(&t))).collect()
                }
            }
            "reasoning" => vec![Content::ReasoningSummary { tokens: None }],
            "assistant" => {
                let mut out: Vec<Content> =
                    self.text_blocks().into_iter().map(Content::Text).collect();
                for call in &self.tool_calls {
                    out.push(Content::ToolUse(ToolUse {
                        id: ToolCallId::from(call.id.clone()),
                        name: call.name.clone(),
                        input: parse_tool_arguments(&call.arguments),
                    }));
                }
                out
            }
            "tool_result" => {
                let call = ToolCallId::from(self.tool_call_id.clone().unwrap_or_default());
                let error = inferred_tool_error(&self.content);
                std::iter::once(Content::ToolResult(ToolResult {
                    call,
                    output: self.content.clone(),
                    error,
                }))
                .chain(self.patch.clone().map(Content::Patch))
                .collect()
            }
            _ => Vec::new(),
        }
    }

    fn model(&self) -> Option<String> {
        self.model_id.clone().filter(|s| !s.is_empty())
    }

    fn usage(&self) -> Option<Usage> {
        self.usage
    }

    fn stop_reason(&self) -> Option<StopReason> {
        if self.kind != "assistant" {
            return None;
        }
        if !self.tool_calls.is_empty() {
            Some(StopReason::ToolUse)
        } else if !self.text_blocks().is_empty() {
            Some(StopReason::EndTurn)
        } else {
            None
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.meta_cwd.clone()
    }

    fn parent_session(&self) -> Option<SessionId> {
        self.meta_parent.as_ref().map(|(id, _)| id.clone())
    }

    fn parent_kind(&self) -> Option<ParentKind> {
        self.meta_parent.as_ref().map(|(_, kind)| *kind)
    }

    fn title(&self) -> Option<TitleChange> {
        self.meta_title.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use futures::TryStreamExt;
    use rstest::rstest;

    use super::*;

    fn pool() -> BlockingPool {
        BlockingPool::new(NonZeroUsize::MIN)
    }

    #[rstest]
    #[case("/tmp/work", "%2Ftmp%2Fwork")]
    #[case("/Users/alice/my repo+1", "%2FUsers%2Falice%2Fmy%20repo%2B1")]
    #[case("/work/a-b.c_d~e", "%2Fwork%2Fa-b.c_d~e")]
    fn encode_cwd_matches_grok_workspace_encoding(#[case] raw: &str, #[case] expected: &str) {
        assert_eq!(encode_cwd(Path::new(raw)), expected);
    }

    #[rstest]
    fn encode_cwd_caps_long_paths_under_filesystem_limit() {
        let deep = format!("/{}/{}/{}", "a".repeat(90), "b".repeat(90), "c".repeat(90));
        let slug = encode_cwd(Path::new(&deep));
        assert!(slug.len() <= MAX_WORKSPACE_SLUG_LEN, "len={} slug={slug}", slug.len());
        assert!(!slug.ends_with('%'), "must never leave a torn percent-escape: {slug}");
        assert_eq!(slug, encode_cwd(Path::new(&deep)));
    }

    #[rstest]
    fn line_key_distinguishes_identical_prompts_across_diverged_branches() {
        let prompt = vec![Content::Text("continue".to_owned())];
        let after_branch_a = line_key(Some("gk-aaaa"), &Role::User, &prompt).unwrap();
        let after_branch_b = line_key(Some("gk-bbbb"), &Role::User, &prompt).unwrap();
        assert_ne!(after_branch_a, after_branch_b);
    }

    /// A real session (Grok Build 1.0.70): shell calls, a read and an edit, then `/compact`.
    #[rstest]
    #[tokio::test]
    async fn a_compacted_session_reads_back_whole() {
        let id = "01a18a39-82d5-7fc1-ae65-177c81cb02ff";
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .joined(format!("tests/fixtures/grok/{id}/chat_history.jsonl"));
        let msgs: Vec<_> = GrokSession::open(SessionId::from(id.to_owned()), fixture, pool())
            .read()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(msgs[0].cwd(), Some(PathBuf::from("/work/proj")));

        // What the compaction replaced comes from its request, each prompt once.
        let prompts: Vec<_> = msgs.iter().filter(|m| m.role() == Role::User).collect();
        assert_eq!(prompts.len(), 2, "{prompts:?}");
        assert!(
            matches!(&prompts[0].content()[..], [Content::Text(t)] if t.starts_with("Run 'ls -la'"))
        );
        assert_eq!(prompts[1].content(), vec![Content::Text("Reply with exactly OK2".to_owned())]);
        let calls: Vec<_> = msgs
            .iter()
            .flat_map(Message::content)
            .filter_map(|c| match c {
                Content::ToolUse(call) => Some(call.name),
                _ => None,
            })
            .collect();
        assert_eq!(calls, [
            "run_terminal_command",
            "read_file",
            "search_replace",
            "run_terminal_command"
        ]);

        let summaries: Vec<_> = msgs
            .iter()
            .filter(|m| matches!(&m.content()[..], [Content::Summary(t)] if t.starts_with("This session is being continued")))
            .collect();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].role(), Role::System);

        // The edit's diff, replayed on the file as the turn found it (`rewind_points.jsonl`).
        let patches: Vec<_> = msgs
            .iter()
            .flat_map(Message::content)
            .filter_map(|c| match c {
                Content::Patch(patch) => Some(patch),
                _ => None,
            })
            .collect();
        assert_eq!(patches.len(), 1, "{patches:#?}");
        assert_eq!(patches[0].files[0].path, "/work/proj/calc.py");
        assert_eq!(patches[0].files[0].hunks[0].lines, [
            " def add(a, b):",
            "-    return a - b",
            "+    return a + b"
        ]);

        // Each turn's usage is on its reply, and the lines are timed in order.
        let used: Vec<_> = msgs.iter().filter_map(Message::usage).collect();
        assert_eq!(used.len(), 2, "{used:?}");
        assert_eq!(used[0].reasoning, Some(184));
        let times: Vec<_> = msgs.iter().filter_map(Message::timestamp).collect();
        assert!(times.is_sorted(), "{times:?}");
        // Only the `<user_info>` both transcripts open with is read twice, as one row.
        let mut seen = std::collections::HashSet::new();
        let twice: Vec<_> = msgs.iter().filter(|m| !seen.insert(m.id())).collect();
        assert_eq!(twice.len(), 1, "{twice:?}");
        assert_eq!(twice[0].role(), Role::System);
    }

    #[rstest]
    #[case::fork(Some("headless"), ParentKind::Fork)]
    #[case::subagent_fork(Some("subagent_fork"), ParentKind::Subagent)]
    #[case::subagent_resume(Some("subagent_resume"), ParentKind::Subagent)]
    #[tokio::test]
    async fn reads_summary_updates_timestamps_and_transcript_entries(
        #[case] session_kind: Option<&str>,
        #[case] expected_parent_kind: ParentKind,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("%2Fwork%2Fproj");
        let sid = "01a18221-a7b8-75e0-b6ef-b2a5cc95d2c9";
        let sdir = ws.joined(sid);
        std::fs::create_dir_all(sdir.join("terminal")).unwrap();
        std::fs::write(sdir.join("terminal").joined("c1.log"), "noise").unwrap();

        let mut summary = json!({
            "info": {"id": sid, "cwd": "/work/proj"},
            "session_summary": "Fallback summary",
            "generated_title": "Fix login bug",
            "created_at": "2026-10-28T10:00:00Z",
            "updated_at": "2026-10-28T10:05:00Z",
            "parent_session_id": "01a18220-0000-7000-8000-000000000000",
        });
        if let Some(kind) = session_kind {
            summary["session_kind"] = json!(kind);
        }
        std::fs::write(sdir.join(SUMMARY_FILE), summary.to_string()).unwrap();

        let updates = [
            json!({"timestamp": 1_793_181_610, "method": "session/update", "params": {"sessionId": sid, "update": {"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "Run cargo test"}, "_meta": {"promptIndex": 0}}}}),
            json!({"timestamp": 1_793_181_620, "method": "session/update", "params": {"sessionId": sid, "update": {"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "run_terminal_command"}}}),
            json!({"timestamp": 1_793_181_625, "method": "session/update", "params": {"sessionId": sid, "update": {"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed"}}}),
            json!({"timestamp": 1_793_181_630, "method": "session/update", "params": {"sessionId": sid, "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "2 tests failed."}}}}),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        std::fs::write(sdir.join(UPDATES_FILE), updates).unwrap();

        let transcript = [
            json!({"type": "system", "content": "You are Grok."}),
            json!({"type": "user", "content": [{"type": "text", "text": "<user_info>\ncwd\n</user_info>"}]}),
            json!({"type": "user", "content": [{"type": "text", "text": "<user_query>\nRun cargo test\n</user_query>"}], "prompt_index": 0}),
            json!({"type": "reasoning", "summary": [{"type": "summary_text", "text": "thinking"}]}),
            json!({"type": "assistant", "content": "Running.", "tool_calls": [{"id": "c1", "name": "run_terminal_command", "arguments": "{\"command\":\"cargo test\"}"}], "model_id": "grok-4.7"}),
            json!({"type": "tool_result", "tool_call_id": "c1", "content": "exit: 2\nfailed\n"}),
            json!({"type": "assistant", "content": "2 tests failed.", "model_id": "grok-4.7"}),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
            + "\n";
        let path = sdir.joined(TRANSCRIPT_FILE);
        std::fs::write(&path, transcript).unwrap();

        assert_eq!(locate(dir.path(), sid), Some(path));

        let sessions = GrokSessions::builder().root(dir.path()).pool(pool()).build();
        let found: Vec<_> = sessions.existing().unwrap().try_collect().await.unwrap();
        assert_eq!(found.len(), 1);

        let msgs: Vec<_> = found[0].read().try_collect().await.unwrap();
        assert_eq!(msgs.len(), 8);
        assert_eq!(msgs[0].id(), Some(MessageId::from(meta_id(sid))));
        assert_eq!(msgs[0].cwd(), Some(PathBuf::from("/work/proj")));
        assert_eq!(
            msgs[0].parent_session(),
            Some(SessionId::from("01a18220-0000-7000-8000-000000000000".to_owned()))
        );
        assert_eq!(msgs[0].parent_kind(), Some(expected_parent_kind));

        let expected_title = TitleChange::new(TitleSource::Generated, "Fix login bug");
        assert_eq!(msgs[1].id(), Some(MessageId::from(title_id(sid, &expected_title))));
        assert_eq!(msgs[1].title(), Some(expected_title));
        assert!(!msgs[1].id().unwrap().as_ref().contains("Fix login bug"));

        assert_eq!(msgs[3].timestamp().unwrap().unix_timestamp(), 1_793_181_610);
        assert_eq!(msgs[5].timestamp().unwrap().unix_timestamp(), 1_793_181_620);
        assert_eq!(msgs[6].timestamp().unwrap().unix_timestamp(), 1_793_181_625);
        assert_eq!(msgs[7].timestamp().unwrap().unix_timestamp(), 1_793_181_630);
        assert!(matches!(msgs[6].content().as_slice(), [Content::ToolResult(ToolResult {
            error: true,
            ..
        })]));
    }

    #[rstest]
    #[tokio::test]
    async fn checkpointed_resume_keeps_safe_checkpoints_and_delivers_late_titles_without_new_lines()
    {
        let dir = tempfile::tempdir().unwrap();
        let sid = "01a18221-a7b8-75e0-b6ef-b2a5cc95d2c9";
        let sdir = dir.path().join("%2Fw").joined(sid);
        std::fs::create_dir_all(&sdir).unwrap();

        let summary_path = sdir.join(SUMMARY_FILE);
        std::fs::write(
            &summary_path,
            json!({
                "info": {"id": sid, "cwd": "/w"},
                "session_summary": "",
                "created_at": "2026-10-28T10:00:00Z",
                "updated_at": "2026-10-28T10:00:01Z",
            })
            .to_string(),
        )
        .unwrap();

        let path = sdir.joined(TRANSCRIPT_FILE);
        let first_turn = format!(
            "{}\n{}\n",
            json!({"type": "user", "content": [{"type": "text", "text": "<user_query>\nok\n</user_query>"}], "prompt_index": 0}),
            json!({"type": "assistant", "content": "done"}),
        );
        std::fs::write(&path, &first_turn).unwrap();

        let session = GrokSession::open(SessionId::from(sid.to_owned()), path.clone(), pool());
        let initial: Vec<_> = session.clone().messages_from(None).try_collect().await.unwrap();
        // Synthetic session_meta carries Checkpoint::default() (at == 0), NOT the first line's end!
        assert_eq!(initial[0].0.at, 0);
        let last_checkpoint = initial.last().unwrap().0;
        let first_ok_id = initial[1].1.id().unwrap();

        // Update ONLY summary.json with a late generated title while capture is stopped (`0` new
        // lines in chat_history.jsonl). Resuming from `last_checkpoint` must still deliver it!
        std::fs::write(
            &summary_path,
            json!({
                "info": {"id": sid, "cwd": "/w"},
                "generated_title": "Named later",
                "created_at": "2026-10-28T10:00:00Z",
                "updated_at": "2026-10-28T10:00:10Z",
            })
            .to_string(),
        )
        .unwrap();

        let title_only: Vec<_> =
            session.clone().messages_from(Some(last_checkpoint)).try_collect().await.unwrap();
        assert_eq!(title_only.len(), 1);
        assert_eq!(title_only[0].0, last_checkpoint);
        assert_eq!(
            title_only[0].1.title(),
            Some(TitleChange::new(TitleSource::Generated, "Named later"))
        );

        // Append a second identical prompt "ok" and verify it gets a distinct prefix-chained key.
        let second_turn = format!(
            "{first_turn}{}\n",
            json!({"type": "user", "content": [{"type": "text", "text": "<user_query>\nok\n</user_query>"}], "prompt_index": 1}),
        );
        std::fs::write(&path, second_turn).unwrap();

        let resumed: Vec<_> =
            session.clone().messages_from(Some(last_checkpoint)).try_collect().await.unwrap();
        let second_ok_id = resumed.last().unwrap().1.id().unwrap();
        assert_ne!(second_ok_id, first_ok_id);

        let full: Vec<_> = session.read().try_collect().await.unwrap();
        let user_ids: Vec<_> =
            full.iter().filter(|m| m.role() == Role::User).map(|m| m.id().unwrap()).collect();
        assert_eq!(user_ids, vec![first_ok_id, second_ok_id]);
    }
}
