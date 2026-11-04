//! Writing a Kiro session back out from captured messages, as
//! `<sessions>/<bucket>/<session>/messages.jsonl` with its `session.json` beside it, the log
//! `kiro-cli chat --resume-id <id>` opens, and the session's line in `session-index/<bucket>.jsonl`.
//! Kiro appends to the log as the session goes on.
//!
//! What Kiro needs of a log it did not write was probed against Kiro CLI 2.36.0 (KAS 0.66.70) with
//! sessions written by hand into a scratch `KIRO_HOME` and resumed headless (`--no-interactive`):
//!
//! - **The rows the model reads** are `user`, `assistant` (`Say`, `Summary`), `tool_call`,
//!   `tool_result`, `agent_note` and `error`; `turn_start` and `turn_end` frame a turn. Kiro
//!   carried a hand-written conversation whole, its tool calls and results with it, and answered
//!   from them.
//! - **No `session_start`**, no `<session_context>` note and no `usage_summary` are needed: Kiro
//!   resumed without them and wrote none after.
//! - **Ids are free text.** Row ids, `executionId`s and `toolCallId`s of atuin's own shape
//!   (`atuin-u1`, `call_x1`, a Claude `toolu_…`) were all taken; a tool call's id needs no
//!   `run_command_` prefix and no `modelToolUseId`.
//! - **A `tool_call` needs `status`.** Without one Kiro dropped the call and its result from
//!   the model's context (the model then denied ever running the command); with `completed`
//!   it kept them, `kind` or no `kind`. A failed result is `success: false` under a `completed`
//!   call, as a hand-written one read back.
//! - **`agent_note` is read by the model**, so a continuation's marker goes there: Kiro gives
//!   the model the note's text as context, as it does its own `<session_context>` block, and
//!   the model quoted the marker's first line back verbatim. The reader yields it as a system
//!   row, from which capture reads the link ([`continued_from_message`]).
//! - **A compaction written by hand holds.** A `tombstone` (`kind: summarization`,
//!   `effectiveFromMessageId`) followed by an `assistant` `Summary` row hid the rows before it:
//!   the model answered from the summary, not from the rows it covered.
//! - **A tool call under a foreign name** (Claude Code's `Read`, with its own `args`) was read
//!   like any other; so was a turn with no prompt (`turn_start` right after a `turn_end`, as an
//!   append of a turn's tail writes), and an `error` row.
//! - **The header is rewritten whole** after a turn, keeping `title`, `parentSessionId`,
//!   `forkedAtMessageId` and `_meta.kiro.atuin` as written and adding Kiro's defaults; a title
//!   written there is kept, not generated over. A row's extra field (`atuinMerged`) is ignored.
//!
//! Each captured row becomes one row under its own id (the row's source id) where Kiro's format
//! has a row for it, so re-capturing the log dedups against the synced rows. A prompt is a
//! `user` row, a reply's text an `assistant` `Say`, each of its calls a `tool_call`, a result a
//! `tool_result`, a compaction summary a `tombstone` and a `Summary`, and text the harness put in
//! the user's turn itself (a continuation's marker, Kiro's own notes) an `agent_note`. Each turn
//! (a prompt to the next) is framed by Kiro's `turn_start` and `turn_end`, the latter with the
//! stop reason of the turn's last reply, under an `executionId` the turn's rows share: the rows'
//! captured `turn_id` where they have one (a restored Kiro session keeps its own), else one
//! derived from the row. What the format (or the capture) can't carry is left out:
//!
//! - **Reasoning** is written as Kiro's own redacted row (`Reasoning`, content `...`, no
//!   signature: Kiro resumed one in the probe), under the row's id: capture keeps reasoning only
//!   as a marker, Kiro keeps the model's reasoning behind a `reasoningSignature` only it can read
//!   back, and Kiro ends every turn on such a row after the last reply, so a copy written back
//!   reads as the native log does, its tip included.
//! - **Tool calls captured without their input** (`threads.capture_tools` off, or a policy
//!   withheld it) become a note in their turn's text ([`tool_note`]), their result dropped, and
//!   the run of assistant rows the results stood between is merged into its last row
//!   ([`Flatten::Runs`]), as the Claude Code and Copilot writers do. The row records which
//!   synced rows went into it ([`MERGED_FIELD`]), as does the row a row with nothing to write
//!   hangs from.
//! - **Tool output** is written back as captured; a call kept with its input but not its output
//!   gets [`UNCAPTURED_OUTPUT`]. Patches have no field in Kiro's result and are dropped.
//! - **A row of text and several calls** (Claude Code's) is a `Say` under the row's id and a
//!   `tool_call` each under ids derived from it; **a row of several results** is a `tool_result`
//!   each, the first under the row's id. Such rows read back as several. Rows a writer of its own
//!   makes for Kiro (a continuation's) hold one call or result each.
//! - **Usage and the model.** Kiro bills credits, not tokens, and names the model in the header
//!   alone (`modelId`): the session's, else `auto`. The header's `agentMode` is the user's default
//!   profile (`chat.defaultAgent` in `settings/cli.json`, else `vibe`): the Kiro TUI reopens a
//!   session in the profile its header names, whatever `--agent` says, and a profile not the
//!   user's asks approval for every write and command.
//! - **Rows with nothing to write** (a reader's title rows, attachments) are skipped: the title
//!   is the header's, which Kiro keeps.
//!
//! A [fork](crate::harnesstools::fork) names the session it was forked from in its header, as
//! Kiro's own rewinds and tangents do (`parentSessionId`, `forkedAtMessageId`: the row it was
//! forked at), with the original's atuin id under `_meta.kiro.atuin`, out of the model's sight;
//! the reader links it as the fork it is.
//!
//! [`tool_note`]: crate::harnesstools::note::tool_note
//! [`Flatten::Runs`]: crate::harnesstools::rehydrate::Flatten::Runs
//! [`MERGED_FIELD`]: crate::harnesstools::rehydrate::MERGED_FIELD
//! [`continued_from_message`]: crate::harnesstools::continuation::continued_from_message

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use time::OffsetDateTime;
use time::macros::format_description;
use uuid::Uuid;

use super::kiro_home;
use super::session::{HEADER_FILE, LOG_FILE, locate};
use crate::harnesstools::rehydrate::{
    RehydrateError, RehydrateMessage, RehydrateSession, UNCAPTURED_OUTPUT, merge_uncaptured_runs,
    record_merged,
};
use crate::harnesstools::resume;
use crate::harnesstools::session::{Content, Role, StopReason, ToolResult, ToolUse};
use crate::path::PathBufExt as _;

/// The header's `schemaVersion`, the one Kiro CLI 2.36.0 writes and the only one it opens.
const SCHEMA_VERSION: &str = "1.0.0";

/// The agent profile a written session says it runs under (`agentMode`) when the user set no
/// default: Kiro's own.
const AGENT_MODE: &str = "vibe";

/// The user's default agent profile, as `kiro-cli agent set-default` records it
/// (`chat.defaultAgent` in `<home>/settings/cli.json`), else [`AGENT_MODE`].
pub(crate) fn default_agent(home: &Path) -> String {
    std::fs::read_to_string(home.join("settings").joined("cli.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|settings| {
            settings["chat.defaultAgent"].as_str().map(str::trim).map(str::to_owned)
        })
        .filter(|agent| !agent.is_empty())
        .unwrap_or_else(|| AGENT_MODE.to_owned())
}

/// The `status` a written `tool_call` carries: the one Kiro keeps the call under when it
/// replays the log (see the module docs).
const CALL_STATUS: &str = "completed";

/// What Kiro writes for the model's reasoning when it can't be shown, and the reader reads as a
/// marker that the model reasoned.
const REDACTED: &str = "...";

/// Write `session` under Kiro's home (`$KIRO_HOME`, else `~/.kiro`, where [`Harness::locate`]
/// looks).
///
/// [`Harness::locate`]: crate::harnesstools::Harness::locate
pub async fn rehydrate(session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
    let (home, session) = (kiro_home(), session.clone());
    tokio::task::spawn_blocking(move || rehydrate_into(&home, &session))
        .await
        .map_err(|e| RehydrateError::Other(e.to_string()))?
}

/// [`rehydrate`] under Kiro's home directory `home`: the log's path.
pub(crate) fn rehydrate_into(
    home: &Path,
    session: &RehydrateSession,
) -> Result<PathBuf, RehydrateError> {
    if !resume::is_plain_name(&session.id) {
        return Err(RehydrateError::Other(format!("not a session id: {:?}", session.id)));
    }
    let root = home.join("sessions");
    if let Some(existing) = locate(&root, &session.id) {
        return Err(RehydrateError::AlreadyExists(existing));
    }
    let bucket = workspace_hash(&session.cwd);
    let dir = root.joined(&bucket).joined(&session.id);
    std::fs::create_dir_all(&dir)?;
    // The log first, never over one that is there: it is the transcript, and the only file a
    // retry may find and call complete. The header is written over whatever a write that failed
    // after the log left, and a write that fails after the log takes the log with it, so a retry
    // starts over rather than resuming a session with no conversation.
    let path = dir.join(LOG_FILE);
    crate::fs::write_new(&path, log(session).as_bytes()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            RehydrateError::AlreadyExists(path.clone())
        } else {
            e.into()
        }
    })?;
    if let Err(e) = std::fs::write(dir.join(HEADER_FILE), header(session, &default_agent(home))) {
        let _ = std::fs::remove_file(&path);
        return Err(e.into());
    }
    // Kiro finds a session its index misses by scanning, so a line that can't be written costs
    // the resume nothing but the scan.
    if let Err(e) = index_add(home, &bucket, &session.id, session.started_at) {
        tracing::warn!(path = %path.display(), %e, "couldn't add a Kiro session to its index");
    }
    Ok(path)
}

/// Kiro's bucket for sessions started in `cwd`: the first 16 hex digits of the SHA-256 of the
/// workspace paths (for a CLI session, `cwd` alone) as Kiro normalises them: `/` for `\`, no
/// trailing slash but a root's, lowercased on Windows. Kiro finds a session in any bucket (its
/// index, then a scan), so a bucket named otherwise still resumes.
pub(crate) fn workspace_hash(cwd: &Path) -> String {
    use sha2::Digest as _;

    let mut path = cwd.to_string_lossy().replace('\\', "/");
    while path.len() > 1 && path.ends_with('/') && !path.ends_with(":/") {
        path.pop();
    }
    if cfg!(windows) {
        path = path.to_lowercase();
    }
    format!("{:x}", sha2::Sha256::digest(path.as_bytes()))[..16].to_owned()
}

/// Append the session's `add` line to `session-index/<bucket>.jsonl` beside the sessions
/// directory, as Kiro does on creating a session: its lookup by id reads these before scanning
/// every bucket.
fn index_add(home: &Path, bucket: &str, id: &str, at: OffsetDateTime) -> std::io::Result<()> {
    let index = home.join("session-index");
    std::fs::create_dir_all(&index)?;
    let line = json!({
        "op": "add",
        "sessionPath": format!("{bucket}/{id}"),
        "at": i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or_default(),
    });
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(index.join(format!("{bucket}.jsonl")))?;
    writeln!(file, "{line}")
}

/// The header beside the log, as Kiro writes one: the session's directory, title, model, the
/// profile `agent` it opens under, and status, and for a fork the session it was forked from.
pub(crate) fn header(session: &RehydrateSession, agent: &str) -> String {
    let at = timestamp(session.started_at);
    let last = session.messages.iter().map(|m| m.timestamp).max().unwrap_or(session.started_at);
    let mut header = json!({
        "schemaVersion": SCHEMA_VERSION,
        "dataModelVersion": 1,
        "id": session.id,
        "title": session.title.as_deref().map(str::trim).unwrap_or_default(),
        "agentMode": agent,
        "workspacePaths": [session.cwd],
        "rootPaths": [session.cwd],
        "createdAt": at,
        "lastModifiedAt": timestamp(last.max(session.started_at)),
        "modelId": session.model.as_deref().unwrap_or("auto"),
        "status": "idle",
    });
    if let Some(of) = &session.fork_of {
        header["parentSessionId"] = json!(of.id);
        if let Some(tip) = session.messages.last() {
            header["forkedAtMessageId"] = json!(tip.source_id);
        }
        header["_meta"] = json!({"kiro": {"atuin": {"forkedFrom": of.id, "atuinId": of.atuin_id}}});
    }
    serde_json::to_string_pretty(&header).unwrap_or_default()
}

/// The whole log: one line per written row, the turns framed.
pub(crate) fn log(session: &RehydrateSession) -> String {
    let mut out = String::new();
    for line in lines(session, None) {
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}

/// A timestamp as Kiro writes one: UTC, to the millisecond.
pub(crate) fn timestamp(at: OffsetDateTime) -> String {
    let format =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");
    at.to_offset(time::UtcOffset::UTC).format(&format).unwrap_or_default()
}

/// The rows for `session`'s messages, calls captured without their input flattened into notes
/// ([`Flatten::Runs`]): for a whole log, or for rows appended to one holding `current`, whose
/// first row a summary's tombstone then covers from. The rows written as no row of their own are
/// recorded on the rows they went into ([`MERGED_FIELD`]).
///
/// [`Flatten::Runs`]: crate::harnesstools::rehydrate::Flatten::Runs
/// [`MERGED_FIELD`]: crate::harnesstools::rehydrate::MERGED_FIELD
pub(crate) fn lines(session: &RehydrateSession, current: Option<&[u8]>) -> Vec<Value> {
    let (messages, hosts) = merge_uncaptured_runs(&session.messages);
    let mut writer = Writer::new(current);
    for message in &messages {
        writer.push(message);
    }
    writer.finish();
    for (row, into) in &mut writer.merged {
        if let Some(host) = hosts.get(row) {
            into.clone_from(host);
        }
    }
    let mut lines = writer.lines;
    record_merged(&mut lines, &writer.merged, |l| l["id"].as_str().map(str::to_owned));
    lines
}

/// The turn being written: its rows share an `executionId`, and its `turn_end` carries the stop
/// reason of its last reply.
struct Turn {
    execution: String,
    stop: Option<StopReason>,
}

struct Writer {
    lines: Vec<Value>,
    /// The row written last, which a row with nothing to write is recorded on.
    last: Option<String>,
    /// The first conversation row of the log, which a summary's tombstone covers from.
    first: Option<String>,
    /// How many rows the log holds, this writer's included: a tombstone says how many it covers.
    count: usize,
    turn: Option<Turn>,
    /// The `executionId`s used so far: a turn's is unique in the log.
    executions: HashSet<String>,
    /// The rows written as no row, each with the row it went into.
    merged: Vec<(String, String)>,
}

impl Writer {
    fn new(current: Option<&[u8]>) -> Self {
        let mut first = None;
        let mut count = 0;
        let mut executions = HashSet::new();
        for line in current.into_iter().flat_map(|bytes| bytes.split(|b| *b == b'\n')) {
            let Ok(line) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            count += 1;
            if first.is_none() && conversation(&line["payload"]) {
                first = line["id"].as_str().map(str::to_owned);
            }
            if let Some(execution) = line["payload"]["executionId"].as_str() {
                executions.insert(execution.to_owned());
            }
        }
        Self {
            lines: Vec::new(),
            last: None,
            first,
            count,
            turn: None,
            executions,
            merged: Vec::new(),
        }
    }

    /// Write `payload` as the row `id`, stamped `at`.
    fn row(&mut self, id: String, at: OffsetDateTime, payload: Value) {
        if self.first.is_none() && conversation(&payload) {
            self.first = Some(id.clone());
        }
        let mut line = json!({"id": id, "timestamp": timestamp(at), "payload": Value::Null});
        line["payload"] = payload;
        self.lines.push(line);
        self.count += 1;
        self.last = Some(id);
    }

    /// The turn `m` is in, opened at `m` when none is: the first row of a turn says which turn
    /// it is (a restored Kiro row its own), else the turn is named for the row.
    fn turn(&mut self, m: &RehydrateMessage) -> String {
        if let Some(turn) = &self.turn {
            return turn.execution.clone();
        }
        let mut execution = m
            .turn_id
            .clone()
            .filter(|turn| resume::is_plain_name(turn))
            .unwrap_or_else(|| derived(&m.source_id, "turn"));
        if !self.executions.insert(execution.clone()) {
            execution = derived(&m.source_id, "turn");
            self.executions.insert(execution.clone());
        }
        self.row(
            format!("{execution}-turn-start"),
            m.timestamp,
            json!({
                "type": "turn_start",
                "executionId": execution,
                "turnId": execution,
                "boundarySource": "observed",
            }),
        );
        self.turn = Some(Turn {
            execution: execution.clone(),
            stop: None,
        });
        execution
    }

    /// Close the turn being written, if any, as the turn's last reply ended it.
    fn end_turn(&mut self, at: OffsetDateTime) {
        let Some(turn) = self.turn.take() else {
            return;
        };
        let stop = stop_reason(turn.stop.as_ref().unwrap_or(&StopReason::EndTurn));
        self.row(
            format!("{}-turn-end", turn.execution),
            at,
            json!({
                "type": "turn_end",
                "stopReason": stop,
                "executionId": turn.execution,
                "turnId": turn.execution,
                "boundarySource": "observed",
            }),
        );
    }

    fn finish(&mut self) {
        let at = self
            .lines
            .last()
            .and_then(|l| l["timestamp"].as_str())
            .and_then(|s| {
                OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            })
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        self.end_turn(at);
    }

    fn push(&mut self, m: &RehydrateMessage) {
        let before = self.lines.len();
        match &m.role {
            Role::User => self.prompt(m),
            Role::Assistant => self.reply(m),
            Role::Tool => {
                let results = Self::results(m);
                if !results.is_empty() {
                    let execution = self.turn(m);
                    self.answer(m, &m.source_id, results, &execution);
                }
            }
            Role::System => self.note(m),
            Role::Other(_) => {}
        }
        if self.lines.len() == before {
            self.merged.push((m.source_id.clone(), self.last.clone().unwrap_or_default()));
        }
    }

    /// A `user` row: what the user typed. A result on the user's line (a call the user
    /// rejected) answers its call first, in the turn the call was made in.
    fn prompt(&mut self, m: &RehydrateMessage) {
        let text = joined_text(&m.content);
        let results = Self::results(m);
        let own = text.is_none();
        if !results.is_empty() {
            let execution = self.turn(m);
            let id = if own {
                m.source_id.clone()
            } else {
                derived(&m.source_id, "results")
            };
            self.answer(m, &id, results, &execution);
        }
        let Some(text) = text else {
            return;
        };
        self.end_turn(m.timestamp);
        self.row(
            m.source_id.clone(),
            m.timestamp,
            json!({"type": "user", "content": text, "documents": [], "images": []}),
        );
    }

    /// A reply: its text as a `Say`, each call a `tool_call`, and any result the row holds (a
    /// tool the API ran itself, which Claude Code keeps on the reply) a `tool_result`; one of
    /// reasoning alone is Kiro's redacted `Reasoning` row. Nothing for an error alone.
    fn reply(&mut self, m: &RehydrateMessage) {
        let text = joined_text(&m.content);
        let calls: Vec<&ToolUse> = m
            .content
            .iter()
            .filter_map(|c| match c {
                Content::ToolUse(call) => Some(call),
                _ => None,
            })
            .collect();
        let results = Self::results(m);
        if text.is_none() && calls.is_empty() && results.is_empty() {
            let reasoned = m
                .content
                .iter()
                .any(|c| matches!(c, Content::Reasoning(_) | Content::ReasoningSummary { .. }));
            if reasoned {
                let execution = self.turn(m);
                let mut row =
                    json!({"type": "assistant", "content": REDACTED, "operationType": "Reasoning"});
                extend(&mut row, &json!({"executionId": execution, "turnId": execution}));
                self.row(m.source_id.clone(), m.timestamp, row);
            }
            return;
        }
        let execution = self.turn(m);
        if let Some(stop) = &m.stop_reason
            && let Some(turn) = &mut self.turn
        {
            turn.stop = Some(stop.clone());
        }
        // The row reads back as its text, else as its first call or result.
        let mut own = Some(m.source_id.clone());
        let mut id = |what: &str| own.take().unwrap_or_else(|| derived(&m.source_id, what));
        let in_turn = json!({"executionId": execution, "turnId": execution});
        if let Some(text) = text {
            let mut say = json!({"type": "assistant", "content": text, "operationType": "Say"});
            extend(&mut say, &in_turn);
            self.row(id("say"), m.timestamp, say);
        }
        for call in calls {
            let args = match &call.input {
                Value::Object(_) => call.input.clone(),
                other => json!({"input": other}),
            };
            let mut row = json!({
                "type": "tool_call",
                "toolCallId": call.id,
                "toolName": call.name,
                "args": args,
                "status": CALL_STATUS,
            });
            extend(&mut row, &in_turn);
            self.row(id(call.id.as_ref()), m.timestamp, row);
        }
        let id = id("result");
        self.answer(m, &id, results, &execution);
    }

    /// The results among `m`'s content.
    fn results(m: &RehydrateMessage) -> Vec<&ToolResult> {
        m.content
            .iter()
            .filter_map(|c| match c {
                Content::ToolResult(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    /// A `tool_result` for each of `results`, the first under `id` and the rest under ids
    /// derived from it.
    fn answer(
        &mut self,
        m: &RehydrateMessage,
        id: &str,
        results: Vec<&ToolResult>,
        execution: &str,
    ) {
        for (n, result) in results.into_iter().enumerate() {
            let output = result
                .output_text()
                .map_or_else(|| UNCAPTURED_OUTPUT.to_owned(), |t| t.into_owned());
            let id = if n == 0 {
                id.to_owned()
            } else {
                derived(id, &n.to_string())
            };
            self.row(
                id,
                m.timestamp,
                json!({
                    "type": "tool_result",
                    "toolCallId": result.call,
                    "content": output,
                    "success": !result.error,
                    "executionId": execution,
                    "turnId": execution,
                }),
            );
        }
    }

    /// A system row: a compaction's summary as Kiro records one, a `tombstone` covering the log
    /// so far then the `Summary`; an error as an `error` row; other text as an `agent_note`,
    /// the context Kiro itself gives the model (a continuation's marker).
    fn note(&mut self, m: &RehydrateMessage) {
        let summary = m.content.iter().find_map(|c| match c {
            Content::Summary(s) if !s.trim().is_empty() => Some(s.clone()),
            _ => None,
        });
        if let Some(summary) = summary {
            self.end_turn(m.timestamp);
            if let Some(first) = self.first.clone() {
                self.row(
                    derived(&m.source_id, "tombstone"),
                    m.timestamp,
                    json!({
                        "type": "tombstone",
                        "kind": "summarization",
                        "effectiveFromMessageId": first,
                        "metadata": {
                            "truncatedMessageCount": self.count,
                            "truncatedAt": timestamp(m.timestamp),
                        },
                    }),
                );
            }
            self.row(
                m.source_id.clone(),
                m.timestamp,
                json!({"type": "assistant", "content": summary, "operationType": "Summary"}),
            );
        } else if let Some(text) = joined_text(&m.content) {
            self.row(
                m.source_id.clone(),
                m.timestamp,
                json!({"type": "agent_note", "content": text}),
            );
        } else if let Some(Content::Error(why)) =
            m.content.iter().find(|c| matches!(c, Content::Error(_)))
        {
            self.row(m.source_id.clone(), m.timestamp, json!({"type": "error", "message": why}));
        }
    }
}

/// Whether `payload` is a row of the conversation (one the reader yields), rather than a turn's
/// framing or Kiro's bookkeeping.
pub(crate) fn conversation(payload: &Value) -> bool {
    matches!(
        payload["type"].as_str(),
        Some(
            "user" | "assistant" | "tool_call" | "tool_result" | "agent_note" | "system" | "error"
        )
    )
}

/// Add `fields` to the object `row`.
fn extend(row: &mut Value, fields: &Value) {
    if let (Some(row), Some(fields)) = (row.as_object_mut(), fields.as_object()) {
        row.extend(fields.clone());
    }
}

/// A `turn_end`'s `stopReason` for `stop` (the reader's `stop_reason` read the other way).
fn stop_reason(stop: &StopReason) -> String {
    match stop {
        StopReason::EndTurn => "end_turn".to_owned(),
        StopReason::ToolUse => "tool_use".to_owned(),
        StopReason::MaxTokens => "max_tokens".to_owned(),
        StopReason::StopSequence => "stop_sequence".to_owned(),
        StopReason::Refusal => "refusal".to_owned(),
        StopReason::Aborted => "aborted".to_owned(),
        StopReason::Error => "error".to_owned(),
        StopReason::Other(other) => other.clone(),
    }
}

/// An id for what a row's or session's `base` id derives: the same every time it is written.
fn derived(base: &str, what: &str) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("atuin:kiro:{base}:{what}").as_bytes()).to_string()
}

fn joined_text(content: &[Content]) -> Option<String> {
    let texts: Vec<&str> = content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) if !t.trim().is_empty() => Some(t.as_str()),
            _ => None,
        })
        .collect();
    (!texts.is_empty()).then(|| texts.join("\n"))
}

#[cfg(test)]
pub(crate) mod tests;
