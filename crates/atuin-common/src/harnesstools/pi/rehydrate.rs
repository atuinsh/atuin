//! Writing a pi session back out from captured messages, as
//! `<session dir>/<started>_<session>.jsonl`: a `session` header naming the directory it resumes
//! in, then one entry per captured row under the row's own id, `parentId`-linked into the same
//! tree, so re-capturing the file dedups against the synced rows and pi rebuilds the same
//! context (pi-mono coding-agent `session-manager.ts`).
//!
//! What the format (or the capture) can't carry is left out:
//!
//! - **Thinking.** Capture keeps only a marker ([`Content::ReasoningSummary`]), no text or
//!   signature, so it is dropped; pi itself drops a thinking block it can't replay.
//! - **Pasted images** and extension-injected messages (`custom_message`): capture keeps
//!   neither, so they are not written back.
//! - **Compactions** become `compaction` entries keeping every entry since the one before
//!   (capture doesn't record which entries pi kept): the model sees the summary and the full
//!   history since the previous compaction. A branch summary is written the same way.
//! - **Settings entries** (model and thinking-level changes, labels, extension state) carry no
//!   content and are skipped, so pi starts on its configured thinking level; the model is named
//!   by each assistant turn, its provider as captured or (for rows captured before providers were) guessed from the model's name.
//!   A new file opens with a `model_change` entry naming the agent's configured default model
//!   (`StartModel`), the one it starts the session on: pi falls back to that default by itself
//!   when it can't use the model a turn names, but omp refuses to open a session whose turns
//!   name no model, or one it has no provider for, unless a `model_change` on the path names one
//!   it can use. A system-role text entry is skipped too.
//! - **Tool calls captured without their input** (`threads.capture_tools` off, or a policy withheld
//!   it): pi-ai sends a `toolCall` without `arguments` as an empty input to Anthropic and as
//!   `"null"` to OpenAI, a call on nothing either way, so each becomes a note in its turn's text
//!   (`[ran a shell command]`; see [`tool_note`](crate::harnesstools::note::tool_note)) and its
//!   `toolResult` is dropped. The run of assistant messages the dropped results stood between is merged into its last one, so the
//!   turns still alternate ([`Flatten::Runs`]). Re-captured, that entry keeps its id (capture
//!   already holds it: nothing is pushed), and the entries merged away and the results are not
//!   there to capture again. The entry records which synced rows went into it
//!   ([`MERGED_FIELD`](crate::harnesstools::rehydrate::MERGED_FIELD)), as does the entry a row
//!   with nothing to write hangs from, so the file says which synced rows it holds.
//! - **Tool results** keep their output; their tool's name comes from the call they answer. With
//!   none captured (withheld by a policy, or over the size limit), a result says
//!   [`UNCAPTURED_OUTPUT`], as does a `!command`'s output. An `edit`'s result whose patch capture kept gets back the `details` pi
//!   shows its diff from: the unified diff (`patch`), pi's own line-numbered one (`diff`) and the
//!   first line it changed.
//! - A `!command` keeps its command, output and whether it failed, not its exit code; one from a
//!   v1 file (no entry ids) has its result renamed after the id it is written under.
//! - The title is written as a `session_info` entry, under the id of the row that set it.
//!
//! [`Flatten::Runs`]: crate::harnesstools::rehydrate::Flatten::Runs

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use time::OffsetDateTime;
use time::macros::format_description;

use super::Flavour;
use super::session::{default_root, locate, new_session_dir};
use crate::harnesstools::rehydrate::{
    RehydrateError, RehydrateMessage, RehydrateSession, UNCAPTURED_OUTPUT, break_line_cycles,
    merge_uncaptured_runs, on_parent_cycles, record_merged,
};
use crate::harnesstools::resume;
use crate::harnesstools::session::{Content, Patch, Role, StopReason, ToolResult, Usage, patch};

/// The session file format version this writes (session-manager.ts `CURRENT_SESSION_VERSION`).
const VERSION: u32 = 3;

/// Write `session` where pi keeps new sessions for the directory it resumes in, after checking
/// every session directory [`Harness::locate`] searches for one already holding it.
///
/// [`Harness::locate`]: crate::harnesstools::Harness::locate
pub async fn rehydrate(
    flavour: Flavour,
    session: &RehydrateSession,
) -> Result<PathBuf, RehydrateError> {
    let session = session.clone();
    tokio::task::spawn_blocking(move || {
        let dir = new_session_dir(flavour, &session.cwd);
        let start = StartModel::configured(flavour);
        rehydrate_into(flavour, &default_root(flavour), &dir, &session, start.as_ref())
    })
    .await
    .map_err(|e| RehydrateError::Other(e.to_string()))?
}

/// [`rehydrate`], looking for an existing copy under `root` and writing into `dir`, the file
/// opening on `start` when there is one.
pub(crate) fn rehydrate_into(
    flavour: Flavour,
    root: &Path,
    dir: &Path,
    session: &RehydrateSession,
    start: Option<&StartModel>,
) -> Result<PathBuf, RehydrateError> {
    if !resume::is_plain_name(&session.id) {
        return Err(RehydrateError::Other(format!("not a session id: {:?}", session.id)));
    }
    if let Some(existing) = locate(root, &session.id) {
        return Err(RehydrateError::AlreadyExists(existing));
    }
    if session.fork_of.as_ref().is_some_and(|of| of.path.is_none()) {
        return Err(RehydrateError::Other(
            "a pi fork names the file of the session it was forked from, which isn't here".into(),
        ));
    }
    std::fs::create_dir_all(dir)?;
    // pi's own name: the start time with `:` and `.` made file-safe, then the id.
    let started = timestamp(session.started_at).replace([':', '.'], "-");
    let path = dir.join(format!("{started}_{}.jsonl", session.id));
    let mut body = transcript(session, start);
    if flavour == Flavour::Omp {
        body.insert_str(0, &title_slot(session.started_at));
    }
    crate::fs::write_new(&path, body.as_bytes()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            RehydrateError::AlreadyExists(path.clone())
        } else {
            e.into()
        }
    })?;
    Ok(path)
}

/// The model an agent starts a new session on: its configured default. pi's is
/// `settings.json` `defaultProvider` and `defaultModel` (pi-mono coding-agent
/// `settings-manager.ts`); omp's is `config.yml` `modelRoles.default`, as `<provider>/<id>`
/// (omp `config.ts`). Each names it in its own `model_change` shape: pi `provider` and
/// `modelId`, omp `model` as `<provider>/<id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StartModel {
    pub flavour: Flavour,
    pub provider: String,
    pub id: String,
}

impl StartModel {
    /// The default model the agent's config directory names, if it names one.
    pub(crate) fn configured(flavour: Flavour) -> Option<Self> {
        Self::configured_in(flavour, &flavour.agent_dir())
    }

    pub(crate) fn configured_in(flavour: Flavour, agent_dir: &Path) -> Option<Self> {
        let (provider, id) = match flavour {
            Flavour::Pi => {
                let raw = std::fs::read_to_string(agent_dir.join("settings.json")).ok()?;
                let settings: Value =
                    serde_json::from_str(raw.strip_prefix('\u{feff}').unwrap_or(&raw)).ok()?;
                let field = |name: &str| {
                    settings
                        .get(name)?
                        .as_str()
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(str::to_owned)
                };
                (field("defaultProvider")?, field("defaultModel")?)
            }
            Flavour::Omp => {
                let raw = std::fs::read_to_string(agent_dir.join("config.yml")).ok()?;
                let model = yaml_model_role_default(&raw)?;
                let (provider, id) = model.split_once('/')?;
                (provider.to_owned(), id.to_owned())
            }
        };
        Some(Self {
            flavour,
            provider,
            id,
        })
    }

    /// The `model_change` entry naming this model, with `id` for the entries after it to hang from.
    fn entry(&self, id: &str, at: OffsetDateTime) -> Value {
        let mut entry = json!({
            "type": "model_change",
            "id": id,
            "parentId": Value::Null,
            "timestamp": timestamp(at),
        });
        match self.flavour {
            Flavour::Pi => {
                entry["provider"] = json!(self.provider);
                entry["modelId"] = json!(self.id);
            }
            Flavour::Omp => entry["model"] = json!(format!("{}/{}", self.provider, self.id)),
        }
        entry
    }
}

/// `modelRoles.default` from omp's `config.yml`: the first `default:` scalar indented under a
/// top-level `modelRoles:` key. omp writes the file itself, so a plain scalar (quoted or not)
/// on its own line is all it ever holds there.
fn yaml_model_role_default(yaml: &str) -> Option<String> {
    let mut in_roles = false;
    for line in yaml.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        let indented = line.len() > trimmed.len();
        if !indented {
            in_roles = trimmed.trim_end() == "modelRoles:";
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("default:").filter(|_| in_roles) {
            let value = value.split('#').next().unwrap_or_default().trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            return Some(value.to_owned()).filter(|v| !v.is_empty());
        }
    }
    None
}

/// A timestamp as pi writes one (`Date.toISOString`): UTC, to the millisecond.
fn timestamp(at: OffsetDateTime) -> String {
    let format =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");
    at.to_offset(time::UtcOffset::UTC).format(&format).unwrap_or_default()
}

/// omp's title slot (omp `session-title-slot.ts`): the `title` line every omp session starts
/// with, exactly 256 bytes with its newline, which omp rewrites in place. Untitled, so omp names
/// the session. It is also what tells an omp session from a pi one where both keep sessions.
fn title_slot(at: OffsetDateTime) -> String {
    const SLOT_BYTES: usize = 256;
    let slot = |pad: &str| {
        json!({"type": "title", "v": 1, "title": "", "updatedAt": timestamp(at), "pad": pad})
            .to_string()
    };
    slot(&" ".repeat(SLOT_BYTES - 1 - slot("").len())) + "\n"
}

/// Milliseconds since the epoch, as pi times its messages.
fn millis(at: OffsetDateTime) -> i64 {
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or_default()
}

fn transcript(session: &RehydrateSession, start: Option<&StartModel>) -> String {
    let mut header = json!({
        "type": "session",
        "version": VERSION,
        "id": session.id,
        "timestamp": timestamp(session.started_at),
        "cwd": session.cwd,
    });
    // A fork names the file of the session it was forked from, as pi's own `/fork` does.
    if let Some(path) = session.fork_of.as_ref().and_then(|of| of.path.as_ref()) {
        header["parentSession"] = json!(path);
    }
    let mut out = header.to_string();
    out.push('\n');
    for line in lines(session, &Continuing::default(), true, start) {
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}

/// A session file rows are appended to: what [`lines`] needs to know of it.
#[derive(Default)]
pub(crate) struct Continuing<'a> {
    /// The entry the file ends on, which the first row then hangs from.
    pub after: Option<&'a str>,
    /// The first entry since the last compaction on the path to `after`: a compaction among the
    /// rows keeps from it.
    pub kept_from: Option<&'a str>,
}

/// The entries for `session`'s rows, calls captured without their input flattened into notes
/// ([`Flatten::Runs`]), and its title when `title`: for a whole session, opening on `start` when
/// there is one, or for rows appended to the file `continuing` describes. The rows written as no
/// entry of their own are recorded on the entries they went into
/// ([`MERGED_FIELD`](crate::harnesstools::rehydrate::MERGED_FIELD)).
///
/// [`Flatten::Runs`]: crate::harnesstools::rehydrate::Flatten::Runs
pub(crate) fn lines(
    session: &RehydrateSession,
    continuing: &Continuing<'_>,
    title: bool,
    start: Option<&StartModel>,
) -> Vec<Value> {
    let (messages, hosts) = merge_uncaptured_runs(&session.messages);
    let session = &RehydrateSession {
        messages,
        ..session.clone()
    };
    let mut writer = Writer::new(session, continuing);
    if let Some(start) = start.filter(|_| continuing.after.is_none()) {
        writer.start(start);
    }
    for message in &session.messages {
        writer.push(message);
    }
    if title {
        writer.title();
    }
    let mut lines = writer.lines;
    break_line_cycles(&mut lines, "id", &["parentId"]);
    for (row, into) in &mut writer.merged {
        if let Some(host) = hosts.get(row) {
            into.clone_from(host);
        }
    }
    record_merged(&mut lines, &writer.merged, |l| l["id"].as_str().map(str::to_owned));
    lines
}

struct Writer<'a> {
    session: &'a RehydrateSession,
    lines: Vec<Value>,
    /// Every row, by source id.
    by_id: HashMap<&'a str, &'a RehydrateMessage>,
    /// The rows that are written (the rest have nothing an entry can carry).
    writable: HashSet<&'a str>,
    /// The rows written so far.
    written: HashSet<&'a str>,
    /// The rows on a parent cycle.
    on_cycle: HashSet<&'a str>,
    /// The last row written.
    last: Option<&'a str>,
    /// The entry the file opens on, when it opens on a model: what a root row hangs from.
    start_id: Option<String>,
    /// The first entry since the last compaction written: the next compaction keeps from it.
    kept_from: Option<&'a str>,
    /// Each tool call's name, by call id.
    tools: HashMap<&'a str, &'a str>,
    /// The rows written as no entry, each with the entry it went into.
    merged: Vec<(String, String)>,
}

impl<'a> Writer<'a> {
    fn new(session: &'a RehydrateSession, continuing: &Continuing<'a>) -> Self {
        let tools = session
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|c| match c {
                Content::ToolUse(u) => Some((u.id.as_ref(), u.name.as_str())),
                _ => None,
            })
            .collect();
        let by_id = session.messages.iter().map(|m| (m.source_id.as_str(), m)).collect();
        let mut writer = Self {
            session,
            lines: Vec::new(),
            on_cycle: on_parent_cycles(&by_id),
            by_id,
            writable: HashSet::new(),
            written: HashSet::new(),
            last: continuing.after,
            start_id: None,
            kept_from: continuing.kept_from,
            tools,
            merged: Vec::new(),
        };
        writer.writable = session
            .messages
            .iter()
            .filter(|m| writer.entry(m).is_some())
            .map(|m| m.source_id.as_str())
            .collect();
        writer
    }

    /// The entry `m` hangs from: its nearest written ancestor, however the rows are ordered;
    /// `None` at the root. A parent that was never synced stands for the entry before, and so
    /// does a parent cycle (a corrupt session's) holding no entry written yet.
    fn parent(&self, m: &'a RehydrateMessage) -> Option<&'a str> {
        let candidates = if self.on_cycle.contains(m.source_id.as_str()) {
            &self.written
        } else {
            &self.writable
        };
        let mut parent = m.parent_source_id.as_deref();

        // At most one step per row, and never back to `m`.
        for _ in 0..=self.by_id.len() {
            let p = parent?;
            if p == m.source_id {
                break;
            }

            if candidates.contains(p) {
                return Some(p);
            }

            match self.by_id.get(p) {
                Some(row) => parent = row.parent_source_id.as_deref(),
                None => break,
            }
        }

        self.last
    }

    /// Open the file on `start`: the entry the root rows hang from.
    fn start(&mut self, start: &StartModel) {
        let id = format!("atuin-{}", crate::utils::uuid_v7().as_simple());
        self.lines.push(start.entry(&id, self.session.started_at));
        self.start_id = Some(id);
    }

    fn push(&mut self, m: &'a RehydrateMessage) {
        let parent = self.parent(m).map(str::to_owned).or_else(|| self.start_id.clone());
        let Some(mut entry) = self.entry(m) else {
            // Before any line (settings entries at the start): merged into none.
            self.merged.push((m.source_id.clone(), parent.unwrap_or_default()));
            return;
        };
        let compaction = entry["type"] == "compaction";
        let fields = entry.as_object_mut().expect("an entry is an object");
        let kind = fields.remove("type").unwrap_or_default();
        let mut line = serde_json::Map::new();
        line.insert("type".to_owned(), kind);
        line.insert("id".to_owned(), json!(m.source_id));
        line.insert("parentId".to_owned(), json!(parent));
        line.insert("timestamp".to_owned(), json!(timestamp(m.timestamp)));
        line.append(fields);
        self.lines.push(Value::Object(line));
        self.last = Some(&m.source_id);
        self.written.insert(&m.source_id);
        if compaction {
            self.kept_from = None;
        } else if self.kept_from.is_none() {
            self.kept_from = Some(&m.source_id);
        }
    }

    /// The session's title, under the id of the last row that named it (so a re-capture
    /// dedups), else a new one.
    fn title(&mut self) {
        let session = self.session;
        let Some(title) = session.title.as_deref().filter(|t| !t.trim().is_empty()) else {
            return;
        };
        let named = session
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Other("session_info".to_owned()))
            .filter(|m| !self.writable.contains(m.source_id.as_str()));
        let (id, at) = named.map_or_else(
            || {
                (
                    format!("atuin-{}", crate::utils::uuid_v7().as_simple()),
                    OffsetDateTime::now_utc(),
                )
            },
            |m| (m.source_id.clone(), m.timestamp),
        );
        self.lines.push(json!({
            "type": "session_info",
            "id": id,
            "parentId": self.last.map(str::to_owned).or_else(|| self.start_id.clone()),
            "timestamp": timestamp(at),
            "name": title,
        }));
    }

    /// The entry for `m`, without its tree fields; `None` when nothing of it can be written.
    fn entry(&self, m: &RehydrateMessage) -> Option<Value> {
        let at = millis(m.timestamp);
        match &m.role {
            Role::User => {
                if let Some(bash) = bash_execution(m, at) {
                    return Some(bash);
                }
                let content = user_blocks(&m.content);
                (!content.is_empty()).then(|| {
                    json!({
                        "type": "message",
                        "message": {"role": "user", "content": content, "timestamp": at},
                    })
                })
            }
            Role::Assistant => assistant_entry(m, at),
            Role::Tool => {
                let result = m.content.iter().find_map(|c| match c {
                    Content::ToolResult(r) => Some(r),
                    _ => None,
                })?;
                let name = self.tools.get(result.call.as_ref()).copied().unwrap_or_default();
                let mut entry = json!({
                    "type": "message",
                    "message": {
                        "role": "toolResult",
                        "toolCallId": result.call,
                        "toolName": name,
                        "content": tool_result_content(&result.output),
                        "isError": result.error,
                        "timestamp": at,
                    },
                });
                let patch = m.content.iter().find_map(|c| match c {
                    Content::Patch(p) if p.call == result.call => Some(p),
                    _ => None,
                });
                if let Some(details) = patch.and_then(edit_details) {
                    entry["message"]["details"] = details;
                }
                Some(entry)
            }
            Role::System => {
                let summary = m.content.iter().find_map(|c| match c {
                    Content::Summary(s) if !s.is_empty() => Some(s),
                    _ => None,
                })?;
                let mut entry = json!({
                    "type": "compaction",
                    "summary": summary,
                    "firstKeptEntryId": self.kept_from,
                    "tokensBefore": 0,
                });
                if let Some(usage) = &m.usage {
                    entry["usage"] = usage_json(usage);
                }
                Some(entry)
            }
            Role::Other(_) => None,
        }
    }
}

/// Text blocks for user-role content.
fn user_blocks(content: &[Content]) -> Vec<Value> {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) | Content::Summary(t) if !t.is_empty() => {
                Some(json!({"type": "text", "text": t}))
            }
            _ => None,
        })
        .collect()
}

/// The `details` pi's `edit` tool keeps of the file `patch` changed (edit.ts): its diff as pi
/// shows it (`diff`, see [`patch::pi_diff`]), as a unified diff (`patch`), and the first line it
/// changed, numbered in the new file.
fn edit_details(patch: &Patch) -> Option<Value> {
    let file = patch.files.first().filter(|f| !f.hunks.is_empty())?;
    let (diff, first) = patch::pi_diff(&file.hunks);
    let mut details = json!({"diff": diff, "patch": file.unified()});
    if let Some(first) = first {
        details["firstChangedLine"] = json!(first);
    }
    Some(details)
}

/// A tool result's content blocks: the ones captured, a text of what was, else
/// [`UNCAPTURED_OUTPUT`] (pi-ai sends a result's blocks as they are, and an empty result would
/// tell the model the tool printed nothing).
fn tool_result_content(output: &Value) -> Value {
    match output {
        Value::Array(_) => output.clone(),
        Value::Null => json!([{"type": "text", "text": UNCAPTURED_OUTPUT}]),
        Value::String(text) => json!([{"type": "text", "text": text}]),
        other => json!([{"type": "text", "text": other.to_string()}]),
    }
}

/// A `!command` the user ran in pi's shell (or `$code` in omp's Python kernel): captured as its
/// command line and a result named after the entry (a v1 entry, which had no id, by its time: it
/// is named after its id now).
fn bash_execution(m: &RehydrateMessage, at: i64) -> Option<Value> {
    let [
        Content::Text(line),
        Content::ToolResult(ToolResult {
            call,
            output,
            error,
        }),
    ] = m.content.as_slice()
    else {
        return None;
    };
    if call.as_ref() != m.source_id && !call.as_ref().starts_with("bash:") {
        return None;
    }
    let (sigil, role, key) = match line.chars().next()? {
        '!' => ('!', "bashExecution", "command"),
        '$' => ('$', "pythonExecution", "code"),
        _ => return None,
    };
    let line = &line[1..];
    let (command, hidden) = match line.strip_prefix(sigil) {
        Some(command) => (command, true),
        None => (line, false),
    };
    let mut entry = json!({
        "type": "message",
        "message": {
            "role": role,
            "output": if output.is_null() { json!(UNCAPTURED_OUTPUT) } else { output.clone() },
            "exitCode": i32::from(*error),
            "cancelled": false,
            "truncated": false,
            "excludeFromContext": hidden,
            "timestamp": at,
        },
    });
    entry["message"][key] = json!(command);
    Some(entry)
}

fn assistant_entry(m: &RehydrateMessage, at: i64) -> Option<Value> {
    let content: Vec<Value> = m
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) | Content::Summary(t) => Some(json!({"type": "text", "text": t})),
            Content::ToolUse(u) => Some(json!({
                "type": "toolCall",
                "id": u.id,
                "name": u.name,
                "arguments": u.input,
            })),
            _ => None,
        })
        .collect();
    let error = m.content.iter().find_map(|c| match c {
        Content::Error(e) => Some(e.as_str()),
        _ => None,
    });
    if content.is_empty() && error.is_none() {
        return None;
    }
    let mut message = json!({"role": "assistant", "content": content});
    if let Some(provider) =
        m.provider.as_deref().or_else(|| m.model.as_deref().and_then(provider_of))
    {
        message["provider"] = json!(provider);
    }
    if let Some(model) = &m.model {
        message["model"] = json!(model);
    }
    message["usage"] = usage_json(&m.usage.unwrap_or_default());
    message["stopReason"] = json!(stop_reason(m.stop_reason.as_ref(), error.is_some()));
    message["timestamp"] = json!(at);
    if let Some(error) = error {
        message["errorMessage"] = json!(error);
    }
    Some(json!({"type": "message", "message": message}))
}

/// The provider a model most likely came from, by its name: pi restores a session's model by
/// provider and id, and falls back to its default when that isn't one it can use.
fn provider_of(model: &str) -> Option<&'static str> {
    let model = model.to_ascii_lowercase();
    if model.starts_with("claude") {
        Some("anthropic")
    } else if model.starts_with("gpt") || model.starts_with("codex") || model.starts_with('o') {
        Some("openai")
    } else if model.starts_with("gemini") {
        Some("google")
    } else {
        None
    }
}

/// pi's name for a stop reason (pi-ai `StopReason`).
fn stop_reason(reason: Option<&StopReason>, failed: bool) -> String {
    match reason {
        Some(StopReason::ToolUse) => "toolUse",
        Some(StopReason::MaxTokens) => "length",
        Some(StopReason::Aborted) => "aborted",
        Some(StopReason::Error) => "error",
        Some(StopReason::Other(other)) => other.as_str(),
        _ if failed => "error",
        _ => "stop",
    }
    .to_owned()
}

/// Usage as pi records it (pi-ai `Usage`), costs unknown.
fn usage_json(usage: &Usage) -> Value {
    let n = |v: Option<u64>| v.unwrap_or_default();
    let mut out = json!({
        "input": n(usage.input),
        "output": n(usage.output),
        "cacheRead": n(usage.cache_read),
        "cacheWrite": n(usage.cache_write),
        "totalTokens": n(usage.input) + n(usage.output) + n(usage.cache_read) + n(usage.cache_write),
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0},
    });
    if let Some(reasoning) = usage.reasoning {
        out["reasoning"] = json!(reasoning);
    }
    out
}

#[cfg(test)]
mod tests;
