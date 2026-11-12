//! Writing a Hermes session from captured messages. A session written here is a restore of one
//! captured elsewhere, a fork, or a continuation from another agent. Each is a new session in the
//! store that a bare `hermes` uses, which [`InstallHome`] finds.
//!
//! **The door.** Hermes lets only its gateway write `state.db`. The one verb that takes a whole
//! session is `session.mutate import`, and `hermes sessions import` reaches it only through a
//! parser that drops tool calls. So atuin's plugin carries a command of its own, `hermes atuin
//! import <file>`, which the plugin's docs describe. This writer builds the session row in the
//! shape `hermes sessions export` writes, and runs that command under the home's profile, with
//! `-p` and with `HERMES_HOME` set to the home. The command exists only where `atuin hook install
//! hermes` put the plugin. Elsewhere Hermes says the command is unknown, and the user is told to
//! install it. Import never touches a session the store holds: an id it holds fails
//! `revision_conflict`, and an id it deleted is never taken back.
//!
//! **What a resumable session needs.** These were verified against Hermes main d70cbc5205. The
//! row has `source` `cli`, with no `user_id` and no gateway routing such as `session_key`,
//! `chat_id` or `origin_json`; otherwise resume refuses it as another owner's. The row names the
//! cwd that atuin resumes in, and the client changes into it. It has no system prompt, which
//! Hermes builds on the first turn. It has an `ended_at`, so that the reader doesn't take the copy
//! for an open one. Titles are unique in a store. A title that is taken goes on its lineage as
//! Hermes's `next_title_in_lineage` numbers it, one past the highest: `Title #2`, then `Title #3`.
//! The writer picks the title from a read-only open before the import, and picks again once if
//! the import still clashes.
//!
//! **Which model runs it.** Hermes resumes a session on the model and provider that its row names
//! in `model` and `model_config.gateway_runtime.provider`, else on the profile's default. A restore
//! names the captured ones only when the profile's own provider is the same one, compared without
//! case. The profile's provider is `model.provider` in the home's `config.yaml`. A provider the
//! profile isn't set up for would have no credentials here, and would fail at the first turn.
//! Otherwise the restore names neither, as a continuation always does, and the profile's default
//! runs it.
//!
//! **Rows.** Each captured row is one Hermes row, with its source id as its `message_uid`. Hermes
//! keeps that through an import, so the reader reads the row back under the key it was written
//! from. A prompt is a `user` row. A prompt of several blocks, or of blocks Hermes can't hold as
//! text, becomes Hermes's multimodal parts. A reply is an `assistant` row of its text, reasoning
//! and calls, in the OpenAI shape that Hermes stores, with its finish reason. A failed model call
//! is a `failed_turn` row. A result is a `tool` row that names its call and its tool. One captured
//! row can become several Hermes rows. For example, a row of several results does. The first takes
//! the row's source id, and the rest take `<source id>#<n>`.
//!
//! - **Every call is answered.** Hermes takes a call with no result for a dangling tail, and tells
//!   the model that its effect is unknown. So a call without a captured result gets one, which
//!   says its output wasn't captured, under `<call's row>#<n>`. Such rows read back as rows of
//!   their own.
//! - **Calls captured without their input** become notes in their turn's text, and their results
//!   are dropped. The run of replies that they stood between is merged into its last reply by
//!   [`merge_uncaptured_runs`], as the Claude Code writer does.
//! - **Rows with nothing to write** are left out. Reasoning that capture kept only as a marker is
//!   one, and the reader's title rows are another. Each row that is left out or merged is recorded
//!   in `display_metadata.atuin_merged` of the row it went into, else of the first row, as
//!   `{"<row>": "<row it went into>"}`. Import keeps `display_metadata`, `/branch` copies it, and
//!   the reader never shows it. So [`local_tip`] can say which synced rows the copy holds without
//!   a row of their own.
//! - **One branch.** Import makes every row it's given part of the conversation. So the resumer
//!   hands this writer only the branch that Hermes resumes, and a turn undone with `/undo` stays
//!   out. See [`Harness::writes_one_branch`].
//!
//! **Limits.** Hermes refuses a session of more than 10,000 rows, or of more than 5 MiB measured as
//! its row in compact JSON. Atuin keeps 64 KiB under that, so that its measure and Python's never
//! disagree at the edge. A session past that has its tool results trimmed, oldest first, until it
//! fits. Each trimmed output is replaced by a line that says so, and a `terminal` result keeps its
//! exit code and error. Hermes's own compactor prunes old results the same way. The status line
//! says how many were trimmed. A trimmed row keeps its `message_uid`, so the full row that atuin
//! holds stands. A session past 10,000 rows, or still past 5 MiB with every result trimmed, is
//! refused with the limit it's past, rather than cut short.
//!
//! A [fork](crate::harnesstools::fork) names the session it came from in
//! `model_config._branched_from`, as `/branch` does, and in its `parent_session_id`. Hermes keeps
//! the second only when the original is a session it took from the same importer, so the reader
//! reads the first.
//!
//! [`local_tip`]: crate::harnesstools::sync::SessionSync::local_tip
//! [`Harness::writes_one_branch`]: crate::harnesstools::Harness::writes_one_branch

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Map, Value, json};
use time::OffsetDateTime;

use super::cli::{CliOutput, HermesCli, HermesCliError, HermesCommand};
use super::home::InstallHome;
use super::store::{DB_FILE, Store};
use crate::harnesstools::rehydrate::{
    RehydrateError, RehydrateMessage, RehydrateSession, Rehydrated, UNCAPTURED_OUTPUT,
    merge_uncaptured_runs, parent_order,
};
use crate::harnesstools::session::{Content, Role, StopReason, ToolCallId, ToolResult, ToolUse};

/// The `display_metadata` key that records the synced rows a row holds without one of their own.
/// The module docs say when a writer records them.
pub const MERGED_KEY: &str = "atuin_merged";

/// Hermes's limits on one session's import, from `hermes_state.py`. Past them the import fails
/// `invalid_params`.
const MAX_MESSAGES: usize = 10_000;
const MAX_BYTES: usize = 5 * 1024 * 1024;

/// How far under [`MAX_BYTES`] a row has to be for atuin to send it: serde's compact JSON and
/// Python's `json.dumps` agree on these rows but for how a float may be written, so a row atuin
/// measures as fitting is never refused at the edge.
const SIZE_MARGIN: usize = 64 * 1024;

/// The most a row atuin sends measures.
const FITS: usize = MAX_BYTES - SIZE_MARGIN;

/// What Hermes's own CLI says, exiting 2, for a command no plugin registered.
const NO_COMMAND: &str = "is not a `hermes` command";

/// Write `session` into the store of `home` through `hermes atuin import`, returning the store.
pub async fn rehydrate(
    home: &InstallHome,
    session: &RehydrateSession,
) -> Result<Rehydrated, RehydrateError> {
    rehydrate_with(home.path(), session, &HermesCommand::new(home)).await
}

/// [`rehydrate`] into the store under the Hermes home `home`, running `hermes`.
pub(super) async fn rehydrate_with(
    home: &Path,
    session: &RehydrateSession,
    hermes: &impl HermesCli,
) -> Result<Rehydrated, RehydrateError> {
    let db = home.join(DB_FILE);
    let messages = messages(session)?;
    let route = Route::of(session).filter(|route| route.serves(profile_provider(home).as_deref()));
    let mut title = None;
    for attempt in 0..2 {
        let held = Held::read(&db, &session.id, session.title.as_deref()).await?;
        if held.id {
            return Err(RehydrateError::AlreadyExists(db));
        }
        let free = session.title.as_deref().map(|title| held.free(title));
        // The import refused for a reason it doesn't name, and nothing here explains it.
        if attempt > 0 && free == title {
            return Err(Refusal::Storage.into_error(&db));
        }
        title = free;
        let mut row = session_row(session, title.as_deref(), route.as_ref(), &messages);
        let trimmed = fit(&mut row)?;
        match import(hermes, &row).await {
            Ok(()) => {
                return Ok(Rehydrated {
                    path: db,
                    note: (trimmed > 0).then(|| trimmed_note(trimmed)),
                });
            }
            // Most likely another session took the title since it was read.
            Err(Refusal::Storage) if attempt == 0 => {}
            Err(refusal) => return Err(refusal.into_error(&db)),
        }
    }
    Err(Refusal::Storage.into_error(&db))
}

/// What the store holds that a new session must not clash with.
#[derive(Debug, Default)]
struct Held {
    /// Whether a session has the new one's id.
    id: bool,
    /// The titles of the lineage the new one's title is of.
    titles: HashSet<String>,
}

impl Held {
    /// What `db` holds of session `id` and the lineage of `title`, read-only. A store not made
    /// yet holds nothing: the gateway makes it.
    async fn read(db: &Path, id: &str, title: Option<&str>) -> Result<Self, RehydrateError> {
        if !db.is_file() {
            return Ok(Self::default());
        }
        let read = async {
            let mut store = Store::open(db).await?;
            let held = async {
                let id = store.holds(id).await?;
                let titles = match title {
                    Some(title) => store.titles_of(lineage(title)).await?,
                    None => Vec::new(),
                };
                Ok::<_, sqlx::Error>(Self {
                    id,
                    titles: titles.into_iter().collect(),
                })
            }
            .await;
            store.close().await;
            held
        };
        read.await.map_err(|err| {
            RehydrateError::Other(format!("reading Hermes's store at {}: {err}", db.display()))
        })
    }

    /// `title` when no session has it, else the next of its lineage.
    fn free(&self, title: &str) -> String {
        if !self.titles.contains(title) {
            return title.to_owned();
        }
        let base = lineage(title);
        let highest = self
            .titles
            .iter()
            .filter_map(|held| {
                if held == base {
                    return Some(1);
                }
                held.strip_prefix(base)?.strip_prefix(" #")?.parse::<u64>().ok()
            })
            .max()
            .unwrap_or(1);
        format!("{base} #{}", highest + 1)
    }
}

/// The title a lineage of `title` is numbered from: `title` without a trailing ` #<n>`.
fn lineage(title: &str) -> &str {
    match title.rsplit_once(" #") {
        Some((base, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => title,
    }
}

/// Why `hermes atuin import` wrote nothing.
#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    /// The profile has no `atuin` command: the plugin isn't installed there, or is disabled.
    NoCommand,
    /// The store holds a session with the id.
    Taken,
    /// The store refused the write without saying why, with `storage_unavailable`. The probes
    /// saw it for a title clash, and a store it can't write would say the same.
    Storage,
    /// Hermes deleted a session with the id, and never takes it back.
    Deleted,
    Other(String),
}

impl Refusal {
    fn into_error(self, db: &Path) -> RehydrateError {
        RehydrateError::Other(match self {
            Self::Taken => return RehydrateError::AlreadyExists(db.to_owned()),
            Self::NoCommand => "this Hermes profile has no `hermes atuin` command, which atuin \
                                writes sessions with: run `atuin hook install hermes`"
                .to_owned(),
            Self::Storage => format!(
                "Hermes's store at {} refused the session (storage_unavailable)",
                db.display()
            ),
            Self::Deleted => "Hermes deleted a session with this id, and won't take it back: fork \
                              it instead"
                .to_owned(),
            Self::Other(why) => format!("hermes atuin import failed: {why}"),
        })
    }
}

/// Import `row` through `hermes`: from a file of its own, which goes once the run is over.
async fn import(hermes: &impl HermesCli, row: &Value) -> Result<(), Refusal> {
    let file = write_row(row).map_err(|err| Refusal::Other(err.to_string()))?;
    let path = file.path().to_string_lossy().into_owned();
    let output = hermes.output(&["atuin", "import", &path]).await;
    drop(file);
    match output {
        Ok(output) => verdict(&output),
        Err(HermesCliError::NotFound) => Err(Refusal::Other("`hermes` is not on your PATH".into())),
        Err(err) => Err(Refusal::Other(err.to_string())),
    }
}

fn write_row(row: &Value) -> std::io::Result<tempfile::NamedTempFile> {
    let mut file =
        tempfile::Builder::new().prefix("atuin-hermes-import-").suffix(".json").tempfile()?;
    serde_json::to_writer(&mut file, row)?;
    std::io::Write::flush(&mut file)?;
    Ok(file)
}

/// What a run of `hermes atuin import` says happened: the plugin's line on stdout, or Hermes's own
/// complaint that it has no such command.
fn verdict(output: &CliOutput) -> Result<(), Refusal> {
    if output.code == Some(2)
        && [&output.stderr, &output.stdout].iter().any(|text| text.contains(NO_COMMAND))
    {
        return Err(Refusal::NoCommand);
    }
    let reply =
        output.stdout.lines().rev().find_map(|line| {
            serde_json::from_str::<Value>(line.trim()).ok().filter(Value::is_object)
        });
    let Some(reply) = reply else {
        return Err(Refusal::Other(output.last_line()));
    };
    if reply["ok"] == true && output.success() {
        return Ok(());
    }
    let error = reply["error"].as_str().unwrap_or("nothing imported");
    Err(if error.contains("revision_conflict") || error == "skipped" {
        Refusal::Taken
    } else if error.contains("admission_conflict") {
        Refusal::Deleted
    } else if error.contains("storage_unavailable") {
        Refusal::Storage
    } else if error.starts_with("unsupported_hermes") {
        Refusal::Other(format!("{error}: this Hermes is too new or too old for atuin's plugin"))
    } else {
        Refusal::Other(error.to_owned())
    })
}

/// What a tool result trimmed to fit [`FITS`] says instead.
pub const TRIMMED_OUTPUT: &str = "[output dropped by atuin to fit Hermes's import limit]";

/// How many bytes Hermes counts `value` as against [`MAX_BYTES`]: its compact UTF-8 JSON, which
/// serde writes the same. Hermes measures it in `hermes_state_portability.py`, with
/// `json.dumps(..., ensure_ascii=False, separators=(",", ":"))`.
fn measured(value: &Value) -> usize {
    value.to_string().len()
}

/// Trim `row`'s tool results, oldest first, until it fits [`FITS`], and say how
/// many were: each result's output becomes [`TRIMMED_OUTPUT`]. A `terminal` result keeps its
/// `exit_code` and `error`, which Hermes's compactor and error checks read; prompts, replies and
/// calls are never touched. Refused when trimming every result isn't enough.
fn fit(row: &mut Value) -> Result<usize, RehydrateError> {
    let mut size = measured(row);
    let mut trimmed = 0;
    let rows = row["messages"].as_array_mut().map(Vec::as_mut_slice).unwrap_or_default();
    for message in rows.iter_mut().filter(|m| m["role"] == "tool") {
        if size <= FITS {
            break;
        }
        let Some(content) = message["content"].as_str() else {
            continue;
        };
        let short = trim_result(content, message["tool_name"].as_str());
        let (before, after) = (measured(&json!(content)), measured(&json!(short)));
        if after >= before {
            continue;
        }
        size -= before - after;
        message["content"] = json!(short);
        trimmed += 1;
    }
    if size > FITS {
        return Err(RehydrateError::Other(format!(
            "the session is {:.1} MiB with every tool output trimmed, more than the {} MiB Hermes \
             takes in one session",
            mib(size),
            MAX_BYTES / (1024 * 1024),
        )));
    }
    Ok(trimmed)
}

/// What the status line says of `n` tool outputs trimmed.
fn trimmed_note(n: usize) -> String {
    let outputs = if n == 1 {
        "output"
    } else {
        "outputs"
    };
    format!(
        "trimmed {n} tool {outputs} to fit Hermes's {} MiB import limit (atuin keeps them whole)",
        MAX_BYTES / (1024 * 1024)
    )
}

#[allow(clippy::cast_precision_loss)]
fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// What a result of `tool` holding `content` says once trimmed: [`TRIMMED_OUTPUT`], inside the
/// envelope of a `terminal` result, whose `exit_code` and `error` stay.
fn trim_result(content: &str, tool: Option<&str>) -> String {
    if tool == Some("terminal")
        && let Ok(Value::Object(result)) = serde_json::from_str::<Value>(content)
    {
        let mut kept = Map::new();
        kept.insert("output".to_owned(), json!(TRIMMED_OUTPUT));
        for key in ["exit_code", "error"] {
            if let Some(value) = result.get(key) {
                kept.insert(key.to_owned(), value.clone());
            }
        }
        return Value::Object(kept).to_string();
    }
    TRIMMED_OUTPUT.to_owned()
}

/// Unix seconds, as Hermes stamps its rows.
fn secs(at: OffsetDateTime) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let nanos = at.unix_timestamp_nanos() as f64;
    nanos / 1e9
}

/// The file of a Hermes home holding its settings, the profile's default model among them.
const CONFIG_FILE: &str = "config.yaml";

/// The provider the Hermes home `home` runs its model on by default: `model.provider` in its
/// `config.yaml`. `None` when there's no such file or key, or it can't be read.
fn profile_provider(home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join(CONFIG_FILE)).ok()?;
    let docs = yaml_rust2::YamlLoader::load_from_str(&text).ok()?;
    let provider = docs.first()?["model"]["provider"].as_str()?.trim();
    (!provider.is_empty()).then(|| provider.to_owned())
}

/// The model a captured session ran on, and who served it. A restore can name them for Hermes to
/// resume the session on, as the module docs say.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Route {
    model: String,
    provider: String,
}

impl Route {
    /// `session`'s, when it names a model and its rows a provider; a continuation names neither.
    fn of(session: &RehydrateSession) -> Option<Self> {
        let model = session.model.clone()?;
        let provider = session.messages.iter().rev().find_map(|m| m.provider.clone())?;
        Some(Self { model, provider })
    }

    /// Whether a profile running on `provider` by default has what this route needs.
    fn serves(&self, provider: Option<&str>) -> bool {
        provider.is_some_and(|provider| provider.eq_ignore_ascii_case(&self.provider))
    }
}

/// The session row `hermes atuin import` takes for `session`, titled `title`, of `messages`,
/// naming `route` for Hermes to resume it on, if any.
fn session_row(
    session: &RehydrateSession,
    title: Option<&str>,
    route: Option<&Route>,
    messages: &[Value],
) -> Value {
    let ended = messages
        .iter()
        .filter_map(|row| row["timestamp"].as_f64())
        .fold(secs(session.started_at), f64::max);
    let mut row = json!({
        "id": session.id,
        "source": "cli",
        "started_at": secs(session.started_at),
        "ended_at": ended,
        "end_reason": "cli_close",
        "messages": messages,
    });
    let put = |row: &mut Value, key: &str, value: Option<Value>| {
        if let Some(value) = value {
            row[key] = value;
        }
    };
    put(&mut row, "title", title.map(Value::from));
    // Hermes refuses to resume in a directory that's gone.
    put(&mut row, "cwd", session.cwd.is_dir().then(|| json!(session.cwd)));
    put(&mut row, "git_branch", session.git_branch.clone().map(Value::from));
    let mut config = Map::new();
    if let Some(route) = route {
        row["model"] = json!(route.model);
        config.insert("gateway_runtime".to_owned(), json!({ "provider": route.provider }));
    }
    if let Some(fork_of) = &session.fork_of {
        config.insert("_branched_from".to_owned(), json!(fork_of.id));
        row["parent_session_id"] = json!(fork_of.id);
    }
    if !config.is_empty() {
        row["model_config"] = Value::Object(config);
    }
    row
}

/// The rows of `session` as Hermes rows, as the module docs describe them. Refused past Hermes's
/// limit on how many rows a session holds.
fn messages(session: &RehydrateSession) -> Result<Vec<Value>, RehydrateError> {
    let ordered = parent_order(session.messages.clone());
    let (rows, hosts) = merge_uncaptured_runs(&ordered);
    let mut writer = Writer::default();
    for row in &rows {
        writer.row(row);
    }
    writer.answer_pending();
    // A row merged into its run's last went there, not into whatever was written before it.
    let left_out: Vec<(String, String)> =
        writer.left_out.into_iter().filter(|(row, _)| !hosts.contains_key(row)).collect();
    let mut merged: Vec<(String, String)> = hosts.into_iter().chain(left_out).collect();
    merged.sort();
    record_merged(&mut writer.rows, &merged);
    if writer.rows.len() > MAX_MESSAGES {
        return Err(RehydrateError::Other(format!(
            "the session has {} rows, more than the {MAX_MESSAGES} Hermes takes in one session",
            writer.rows.len()
        )));
    }
    Ok(writer.rows)
}

/// Record each of `merged`, a row and the row it went into, under `display_metadata` of the row it
/// went into, else of the first row.
fn record_merged(rows: &mut [Value], merged: &[(String, String)]) {
    for (row, into) in merged {
        let at = rows.iter().position(|r| r["message_uid"] == into.as_str()).unwrap_or(0);
        let Some(target) = rows.get_mut(at) else {
            continue;
        };
        let metadata = target
            .as_object_mut()
            .expect("rows are objects")
            .entry("display_metadata")
            .or_insert_with(|| json!({}));
        metadata[MERGED_KEY][row] = json!(into);
    }
}

/// The Hermes rows written so far, and what they leave to answer.
#[derive(Debug, Default)]
struct Writer {
    rows: Vec<Value>,
    /// The rows written nothing for, each with the last row written before it. That is `""` when
    /// nothing was written before it.
    left_out: Vec<(String, String)>,
    /// Each call written so far, by id, with its tool's name.
    calls: HashMap<ToolCallId, String>,
    /// The calls of the last reply not answered yet, and that reply's row, which the results
    /// atuin makes for them are named after.
    pending: Vec<ToolCallId>,
    pending_of: Option<(String, usize)>,
    answered: HashSet<ToolCallId>,
}

impl Writer {
    fn last_uid(&self) -> String {
        self.rows.last().and_then(|row| row["message_uid"].as_str()).unwrap_or_default().to_owned()
    }

    fn row(&mut self, m: &RehydrateMessage) {
        let before = self.rows.len();
        let results: Vec<&ToolResult> = m
            .content
            .iter()
            .filter_map(|c| match c {
                Content::ToolResult(result) => Some(result),
                _ => None,
            })
            .collect();
        match m.role {
            Role::User => {
                self.answer_pending();
                if let Some(content) = user_content(&m.content) {
                    self.push(m, 0, json!({"role": "user", "content": content}));
                }
            }
            Role::Assistant => self.reply(m),
            _ => {}
        }
        for result in results {
            self.result(m, result);
        }
        if self.rows.len() == before {
            let into = self.last_uid();
            self.left_out.push((m.source_id.clone(), into));
        }
    }

    /// Push `row` as the `n`th Hermes row of `m`.
    fn push(&mut self, m: &RehydrateMessage, n: usize, mut row: Value) {
        let uid = if n == 0 {
            m.source_id.clone()
        } else {
            format!("{}#{n}", m.source_id)
        };
        row["message_uid"] = json!(uid);
        row["timestamp"] = json!(secs(m.timestamp));
        self.rows.push(row);
    }

    /// The Hermes rows already written for `m`.
    fn written_for(&self, m: &RehydrateMessage) -> usize {
        let own = m.source_id.as_str();
        self.rows
            .iter()
            .rev()
            .take_while(|row| {
                row["message_uid"].as_str().is_some_and(|uid| {
                    uid == own || uid.strip_prefix(own).is_some_and(|rest| rest.starts_with('#'))
                })
            })
            .count()
    }

    fn reply(&mut self, m: &RehydrateMessage) {
        let texts: Vec<&str> = m
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text(text) if !text.trim().is_empty() => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let reasoning: Vec<&str> = m
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Reasoning(text) if !text.trim().is_empty() => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let calls: Vec<&ToolUse> = m
            .content
            .iter()
            .filter_map(|c| match c {
                Content::ToolUse(call) if !call.input.is_null() => Some(call),
                _ => None,
            })
            .collect();
        let error = m.content.iter().find_map(|c| match c {
            Content::Error(error) => Some(error.as_str()),
            _ => None,
        });
        if texts.is_empty() && reasoning.is_empty() && calls.is_empty() {
            if let Some(error) = error {
                self.answer_pending();
                // As Hermes records a model call that failed, such as a provider's 401. The error
                // is the row's text too, because the reader keeps no row without text.
                self.push(
                    m,
                    0,
                    json!({
                        "role": "assistant",
                        "content": error,
                        "display_kind": "failed_turn",
                        "display_metadata": {"error": error},
                    }),
                );
            }
            return;
        }
        self.answer_pending();
        let mut row = json!({"role": "assistant", "content": texts.join("\n\n")});
        if !reasoning.is_empty() {
            row["reasoning"] = json!(reasoning.join("\n\n"));
        }
        if !calls.is_empty() {
            let written: Vec<Value> = calls
                .iter()
                .map(|call| {
                    let arguments = match &call.input {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": {"name": call.name, "arguments": arguments},
                    })
                })
                .collect();
            row["tool_calls"] = Value::Array(written);
            for call in &calls {
                self.calls.insert(call.id.clone(), call.name.clone());
                self.pending.push(call.id.clone());
            }
        }
        let finish = match &m.stop_reason {
            _ if !calls.is_empty() => Some("tool_calls".to_owned()),
            Some(StopReason::EndTurn) => Some("stop".to_owned()),
            Some(StopReason::ToolUse) => Some("tool_calls".to_owned()),
            Some(StopReason::MaxTokens) => Some("length".to_owned()),
            Some(StopReason::Refusal) => Some("content_filter".to_owned()),
            Some(StopReason::Other(other)) => Some(other.clone()),
            Some(_) | None => None,
        };
        if let Some(finish) = finish {
            row["finish_reason"] = json!(finish);
        }
        self.push(m, 0, row);
        if !calls.is_empty() {
            self.pending_of = Some((m.source_id.clone(), 1));
        }
    }

    /// A `tool` row for `result`, when it answers a call the last reply made that has no
    /// answer yet: Hermes sends a tool row only right after the call it answers.
    fn result(&mut self, m: &RehydrateMessage, result: &ToolResult) {
        let Some(at) = self.pending.iter().position(|id| *id == result.call) else {
            return;
        };
        self.pending.remove(at);
        self.answered.insert(result.call.clone());
        let output =
            result.output_text().map_or_else(|| UNCAPTURED_OUTPUT.to_owned(), Cow::into_owned);
        let n = self.written_for(m);
        self.push(
            m,
            n,
            json!({
                "role": "tool",
                "tool_call_id": result.call,
                "tool_name": self.calls.get(&result.call),
                "content": output,
            }),
        );
    }

    /// Answer every call of the last reply still unanswered with a result saying its output
    /// wasn't captured, named after the reply's row.
    fn answer_pending(&mut self) {
        let Some((of, mut n)) = self.pending_of.take() else {
            return;
        };
        let at = self
            .rows
            .iter()
            .rev()
            .find(|row| row["message_uid"] == of.as_str())
            .and_then(|row| row["timestamp"].as_f64())
            .unwrap_or_default();
        for call in std::mem::take(&mut self.pending) {
            self.rows.push(json!({
                "role": "tool",
                "tool_call_id": call,
                "tool_name": self.calls.get(&call),
                "content": UNCAPTURED_OUTPUT,
                "message_uid": format!("{of}#{n}"),
                "timestamp": at,
            }));
            n += 1;
        }
    }
}

/// A prompt's content as the import takes it: one text as a string, else a list of its parts. The
/// parts are its text blocks, and whatever else it holds as it is. Hermes stores the list as its
/// multimodal content, `\x00json:` and the list. A string is stored as given, so parts handed over
/// as one string would be read back as a single text. `None` for a prompt with nothing in it.
fn user_content(content: &[Content]) -> Option<Value> {
    let parts: Vec<Value> = content
        .iter()
        .filter_map(|c| match c {
            Content::Text(text) if !text.trim().is_empty() => {
                Some(json!({"type": "text", "text": text}))
            }
            Content::Summary(summary) if !summary.trim().is_empty() => {
                Some(json!({"type": "text", "text": summary}))
            }
            Content::Other(other) => Some(other.clone()),
            _ => None,
        })
        .collect();
    match parts.as_slice() {
        [] => None,
        [part] if part["type"] == "text" => Some(part["text"].clone()),
        _ => Some(Value::Array(parts)),
    }
}

#[cfg(test)]
pub mod tests;
