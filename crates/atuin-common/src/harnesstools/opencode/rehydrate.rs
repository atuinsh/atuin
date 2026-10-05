//! An opencode session written back out from captured messages (see [`rehydrate`]), through
//! opencode's own `opencode import`.
//!
//! opencode keeps its sessions in one SQLite database, whose schema is its own business; `opencode
//! import <file>` takes the JSON `opencode export` writes and inserts it the way opencode itself
//! does (`cli/cmd/import.ts`): the session under its own id, each message and part under theirs.
//! So the session keeps its id, and each part the id capture keyed its row on.
//!
//! # What survives
//!
//! Every part capture delivered, under its own id: text (the user's, the model's, and what
//! opencode injected, `synthetic`), reasoning, tool calls with their result or error, retries,
//! each model call's `step-finish` with its token counts and finish reason, and the parts capture
//! kept whole (`step-start`, files, compaction markers, subtasks...) as they were. A failed
//! model call is its assistant message's `error`, under the message id capture keyed it on.
//!
//! What capture keeps of a message is less than opencode's info for it, so the rest is rebuilt:
//!
//! - **Which message a part belongs to.** Rows carry no message id, so parts are grouped back
//!   into messages: an assistant's by the user message it answers and the model call it was
//!   (`MessageInfo::turn_of`, which also gives back its creation time, provider and model), the
//!   rest in the order they came. A message's id is recovered where a row names it -- a part
//!   kept whole carries its `messageID`, a failure is keyed on it, an assistant message names the
//!   user message it answers -- and minted otherwise. Only part and failure ids are row keys, so a
//!   minted message id costs nothing on re-capture.
//! - **Agent and mode** (`build`), cost (`0`), and a tool call's title and metadata (empty).
//!
//! The session takes the directory `opencode import` runs in, its project, and its title.
//!
//! # What is not the same
//!
//! - **Tool calls captured without their input** (capture keeps only a call's name now) are not
//!   written as tool parts: opencode would send the model a call on no input (`{"input": null}`,
//!   the object its state needs) with an output it never had. Each becomes a text part under the
//!   call's own part id, reading as a [note](crate::harnesstools::note::tool_note) (`[ran a shell command]`), or joins a
//!   text part of the same message before it, with nothing but reasoning or other notes between
//!   (the same note several times in a row counted, `×3`; see [`Flatten::Notes`]). Re-captured,
//!   the part reads back under a part id capture already holds, so nothing is pushed; a part
//!   merged away is not there to capture again.
//! - **Tool output**: capture keeps none now; a call kept with its input (older records) is
//!   written `completed` (or `error`) with [`UNCAPTURED_OUTPUT`] as its output.
//! - opencode stamps each part row with the time of the import. Text, reasoning, tool and retry
//!   parts carry their own clock and read back with the captured timestamp; the rest (`step-start`,
//!   `step-finish`, whole parts) read back with the import's.
//! - opencode reads a message's parts in id order, where capture delivered them as they finished:
//!   parallel tool calls that finished out of order come back in id order.
//! - A tool call captured without a result (one opencode stopped mid-way) is written `pending`,
//!   which opencode shows as interrupted and capture skips as unfinished.
//! - The session's parent, for a subagent's, is not kept. A [fork](crate::harnesstools::fork)
//!   names its original in a marker instead, and gets fresh message and part ids.
//! - Rows opencode 2.0 wrote (`session_v2`, the experimental event system) name no part id of the
//!   older layout `opencode import` writes; their parts get minted ids and would be captured again
//!   as new rows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Map, Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{Connection, Sqlite};
use time::OffsetDateTime;

use super::session;
use crate::db::query_scalar;
use crate::harnesstools::rehydrate::{
    Flatten, ForkOf, RehydrateError, RehydrateMessage, RehydrateSession, UNCAPTURED_OUTPUT,
    flatten_uncaptured_calls,
};
use crate::harnesstools::session::{Content, Role, StopReason, ToolResult, ToolUse, Usage};
use crate::harnesstools::{AnyHarness, continuation};

/// How long `opencode import` may take before it is given up on.
const IMPORT_TIMEOUT: Duration = Duration::from_secs(120);

/// The environment `opencode import` runs with, on top of this process's: nothing it does on
/// start-up may reach the network (a model catalogue refresh, an update check, an LSP download,
/// plugins it would install) or share the session. `--pure` keeps external plugins out too.
const QUIET_ENV: &[(&str, &str)] = &[
    ("OPENCODE_DISABLE_AUTOUPDATE", "1"),
    ("OPENCODE_DISABLE_MODELS_FETCH", "1"),
    ("OPENCODE_DISABLE_LSP_DOWNLOAD", "1"),
    ("OPENCODE_DISABLE_DEFAULT_PLUGINS", "1"),
    ("OPENCODE_DISABLE_SHARE", "1"),
    ("OPENCODE_PURE", "1"),
];

/// Import `session` into opencode's database with `opencode import`, run from the session's
/// directory ([`RehydrateSession::cwd`], which becomes the session's directory and project), and
/// hand back the database: the path `locate` finds the session in.
///
/// The database is the one opencode resolves from this process's environment (`OPENCODE_DB`,
/// `XDG_DATA_HOME`, its channel databases; see `session::default_db`), and `opencode import` is
/// pointed at it explicitly (`OPENCODE_DB`), so the two cannot disagree. opencode must be on
/// `PATH`. It is never asked to call a model: import only writes the database.
///
/// Fails with [`RehydrateError::AlreadyExists`] when opencode already holds the session whole.
/// The export goes to opencode in one file, but opencode inserts it a row at a time (the session
/// first), so an import that fails part-way leaves the session partly written, where
/// [`locate`](crate::harnesstools::Harness::locate) finds it. Such a session is imported again:
/// import inserts each message and part only if its id is new, so what is there stays as it is
/// and what is missing is added. A session that has been worked on since (see `Held`) is left
/// alone, so that a message a revert deleted never comes back.
pub async fn rehydrate(session: &RehydrateSession) -> Result<PathBuf, RehydrateError> {
    let db = session::default_db().ok_or(RehydrateError::NoDataDir)?;
    import(Path::new("opencode"), &db, session).await
}

/// [`rehydrate`] into `db`, with the `opencode` executable `program`.
pub(crate) async fn import(
    program: &Path,
    db: &Path,
    session: &RehydrateSession,
) -> Result<PathBuf, RehydrateError> {
    if !session.id.starts_with("ses") || session.id.contains(['/', '\\', '\0']) {
        return Err(RehydrateError::Other(format!(
            "{:?} is not an opencode session id",
            session.id
        )));
    }
    let exported = export(session);
    if session::locate(db, &session.id).await.is_some()
        && held(db, &session.id, &exported).await != Held::Partly
    {
        return Err(RehydrateError::AlreadyExists(db.to_path_buf()));
    }
    if !session.cwd.is_dir() {
        return Err(RehydrateError::Other(format!(
            "{} is not a directory to import the session into",
            session.cwd.display()
        )));
    }
    let file = export_file(&exported)?;
    let mut command = tokio::process::Command::new(program);
    command
        .arg("import")
        .arg("--pure")
        .arg(&*file)
        .current_dir(&session.cwd)
        .env("OPENCODE_DB", db)
        .env_remove("OPENCODE_AUTO_SHARE")
        .envs(QUIET_ENV.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(IMPORT_TIMEOUT, command.output())
        .await
        .map_err(|_| RehydrateError::Other("opencode import timed out".to_owned()))??;
    if !output.status.success() {
        return Err(RehydrateError::Other(format!(
            "opencode import failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    session::locate(db, &session.id).await.ok_or_else(|| {
        RehydrateError::Other(format!(
            "opencode import did not write the session: {}",
            String::from_utf8_lossy(&output.stdout).trim()
        ))
    })
}

/// How much of an export opencode's database holds of a session it has a row for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    /// Every message and part of the export, under its id.
    Whole,
    /// Some of them are missing, and nothing else is there: an import that failed part-way.
    Partly,
    /// The session has been worked on since it was imported, or can't be read: it has a message
    /// or part the export doesn't, or a revert pending (`/undo`, which deletes the messages it
    /// takes back when the next prompt commits it). What is missing may be what a revert took,
    /// so nothing is imported into it again.
    Changed,
}

/// How much of `exported` session `id` in `db` holds, comparing the ids of its messages and
/// parts with the export's.
async fn held(db: &Path, id: &str, exported: &Value) -> Held {
    use std::collections::HashSet;

    let as_ids = |values: Vec<&Value>| -> HashSet<String> {
        values.into_iter().filter_map(Value::as_str).map(str::to_owned).collect()
    };
    let messages = || exported["messages"].as_array().into_iter().flatten();
    let want_messages = as_ids(messages().map(|m| &m["info"]["id"]).collect());
    let want_parts = as_ids(
        messages()
            .flat_map(|m| m["parts"].as_array().into_iter().flatten())
            .map(|p| &p["id"])
            .collect(),
    );

    let read = async {
        let opts = SqliteConnectOptions::new()
            .filename(db)
            .read_only(true)
            .busy_timeout(Duration::from_secs(2));
        let mut conn = SqliteConnection::connect_with(&opts).await?;
        // Only the `session` table is what `opencode import` writes (opencode 2.0 keeps its own
        // sessions elsewhere).
        let exists = query_scalar::<Sqlite, i64>("SELECT 1 FROM session WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut conn)
            .await?
            .is_some();
        if !exists {
            return Err(sqlx::Error::RowNotFound);
        }
        // A database whose `session` table has no `revert` column has nothing to revert.
        let revert =
            query_scalar::<Sqlite, Option<String>>("SELECT revert FROM session WHERE id = ?")
                .bind(id)
                .fetch_optional(&mut conn)
                .await
                .unwrap_or_default();
        let messages: Vec<String> =
            query_scalar::<Sqlite, String>("SELECT id FROM message WHERE session_id = ?")
                .bind(id)
                .fetch_all(&mut conn)
                .await?;
        let parts: Vec<String> =
            query_scalar::<Sqlite, String>("SELECT id FROM part WHERE session_id = ?")
                .bind(id)
                .fetch_all(&mut conn)
                .await?;
        let _ = conn.close().await;
        Ok::<_, sqlx::Error>((revert.flatten(), messages, parts))
    };
    let Ok((revert, messages, parts)) = read.await else {
        return Held::Changed;
    };
    if revert.is_some_and(|r| !r.is_empty()) {
        return Held::Changed;
    }
    let (messages, parts): (HashSet<_>, HashSet<_>) =
        (messages.into_iter().collect(), parts.into_iter().collect());
    if !messages.is_subset(&want_messages) || !parts.is_subset(&want_parts) {
        Held::Changed
    } else if messages == want_messages && parts == want_parts {
        Held::Whole
    } else {
        Held::Partly
    }
}

/// `export`, written to a file of its own in the temporary directory for `opencode import` to
/// read (readable by this user alone: it is the whole transcript), removed again when dropped.
fn export_file(export: &Value) -> Result<crate::fs::RemoveOnDropPath, RehydrateError> {
    let dir = std::env::temp_dir();
    let bytes = serde_json::to_vec(export).map_err(|err| RehydrateError::Other(err.to_string()))?;
    for n in 0u32.. {
        let path = dir.join(format!("atuin-opencode-import-{}-{n}.json", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&path) {
            Ok(mut file) => {
                let path = crate::fs::RemoveOnDropPath(path);
                std::io::Write::write_all(&mut file, &bytes)?;
                return Ok(path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err.into()),
        }
    }
    unreachable!("an unbounded range always finds a free name")
}

/// Milliseconds since the epoch, opencode's clock.
fn millis(at: OffsetDateTime) -> i64 {
    i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or_default()
}

/// An id opencode would accept for a row it did not get one for: its prefix, then (as opencode's
/// own ascending ids) the time it stands for in hex, then characters derived from `seed`, so the
/// same session always mints the same ids.
fn mint(prefix: &str, at: i64, seed: &str) -> String {
    mint_at(prefix, u64::try_from(at).unwrap_or_default().wrapping_mul(0x1000), seed)
}

/// A new part's id, as [`mint`] makes them: parts of one message sort in time order.
pub(crate) fn mint_part(at: OffsetDateTime, seed: &str) -> String {
    mint("prt", millis(at), seed)
}

/// A new session's id, as opencode's own (`Identifier.descending`): [`mint`]'s, its time
/// inverted so the newest sorts first.
pub(crate) fn mint_session(at: OffsetDateTime, seed: &str) -> String {
    mint_at("ses", !u64::try_from(millis(at)).unwrap_or_default().wrapping_mul(0x1000), seed)
}

/// The digits of opencode's ids, in the order they sort in.
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

fn mint_at(prefix: &str, time: u64, seed: &str) -> String {
    let mut hash = xxhash_rust::xxh3::xxh3_128(seed.as_bytes());
    let tail: String = (0..14)
        .map(|_| {
            let c = BASE62[usize::try_from(hash % 62).unwrap_or_default()];
            hash /= 62;
            char::from(c)
        })
        .collect();
    let time = time & 0xffff_ffff_ffff;
    format!("{prefix}_{time:012x}{tail}")
}

/// The model call an assistant row's turn names (`MessageInfo::turn_of`): when its message was
/// created, and by which provider and model; or, for a message that did not say, its id.
enum Turn<'a> {
    Call {
        created: i64,
        provider: &'a str,
        model: &'a str,
    },
    Message(&'a str),
}

impl<'a> Turn<'a> {
    /// The turn an assistant row belongs to, without a `step-finish`'s counts.
    fn of(turn: &'a str) -> Option<Self> {
        let turn = turn.split_once('#').map_or(turn, |(turn, _)| turn);
        if let Some((created, rest)) = turn.split_once(':')
            && let Ok(created) = created.parse()
            && let Some((provider, model)) = rest.split_once('/')
        {
            return Some(Self::Call {
                created,
                provider,
                model,
            });
        }
        turn.starts_with("msg").then_some(Self::Message(turn))
    }
}

/// A message being rebuilt from its rows.
struct Draft {
    id: Option<String>,
    assistant: bool,
    /// The user message an assistant's answers, and its turn (without step counts): what tells
    /// its rows from the next message's.
    parent: Option<String>,
    turn: Option<String>,
    created: i64,
    completed: i64,
    model: Option<String>,
    cwd: Option<PathBuf>,
    summary: bool,
    error: Option<Value>,
    /// The last `step-finish`'s reason and counts, which opencode also keeps on the message.
    finish: Option<(String, Value)>,
    parts: Vec<(Option<String>, Value)>,
}

impl Draft {
    fn new(assistant: bool, row: &RehydrateMessage) -> Self {
        let at = millis(row.timestamp);
        Self {
            id: None,
            assistant,
            parent: assistant.then(|| row.parent_source_id.clone()).flatten(),
            turn: assistant.then(|| turn_base(row)).flatten(),
            created: at,
            completed: at,
            model: row.model.clone(),
            cwd: row.cwd.clone(),
            summary: false,
            error: None,
            finish: None,
            parts: Vec::new(),
        }
    }

    /// Whether assistant `row` is another row of this message.
    fn holds(&self, row: &RehydrateMessage) -> bool {
        self.assistant && self.parent == row.parent_source_id && self.turn == turn_base(row)
    }
}

fn turn_base(row: &RehydrateMessage) -> Option<String> {
    let turn = row.turn_id.as_deref()?;
    Some(turn.split_once('#').map_or(turn, |(turn, _)| turn).to_owned())
}

/// The message a row names as its own: a part kept whole carries its `messageID`, a row of
/// opencode 2.0 is keyed `<message>/<part>`, and a failure is keyed on its message.
fn named_message(row: &RehydrateMessage) -> Option<String> {
    for content in &row.content {
        if let Content::Other(part) = content
            && let Some(id) = part["messageID"].as_str()
        {
            return Some(id.to_owned());
        }
    }
    if let Some((message, _)) = row.source_id.split_once('/') {
        return Some(message.to_owned());
    }
    is_failure(row).then(|| row.source_id.clone())
}

/// A failed assistant message's row: keyed on the message, saying only why.
fn is_failure(row: &RehydrateMessage) -> bool {
    row.role == Role::Assistant
        && row.source_id.starts_with("msg")
        && matches!(row.content.as_slice(), [Content::Error(_)])
}

/// The row for the session itself (`<session>:title:<title>`, `<session>:session`), which the
/// session's own info stands for; for a fork, the original's too.
fn is_session_row(session: &RehydrateSession, row: &RehydrateMessage) -> bool {
    let of = |id: &str| row.source_id.strip_prefix(id).is_some_and(|rest| rest.starts_with(':'));
    row.content.is_empty()
        && row.usage.is_none()
        && (of(&session.id) || session.fork_of.as_ref().is_some_and(|f| of(&f.id)))
}

/// The JSON `opencode export` writes for `session`, which `opencode import` reads back.
pub(crate) fn export(session: &RehydrateSession) -> Value {
    let mut drafts: Vec<Draft> = Vec::new();
    // A note joins a text part of its own message: the one answering the same prompt in the
    // same model call.
    let rows = flatten_uncaptured_calls(&session.messages, &Flatten::Notes {
        host: &|host, row| {
            host.parent_source_id == row.parent_source_id && turn_base(host) == turn_base(row)
        },
        own: &|_| true,
    });
    for row in &rows {
        if is_session_row(session, row) {
            continue;
        }
        let assistant = row.role == Role::Assistant;
        let named = named_message(row);
        let same = drafts.last().is_some_and(|draft| {
            match (&named, &draft.id) {
                (Some(named), Some(id)) => named == id,
                // A named row starts its own message unless this one could be it.
                (Some(_), None) => {
                    if assistant {
                        draft.holds(row)
                    } else {
                        !draft.assistant
                    }
                }
                (None, _) => {
                    if assistant {
                        draft.holds(row)
                    } else {
                        // What the user said and what opencode injected into it sit in one
                        // message until the model answers it.
                        !draft.assistant
                    }
                }
            }
        });
        if !same {
            drafts.push(Draft::new(assistant, row));
        }
        let draft = drafts.last_mut().expect("a draft was just ensured");
        if draft.id.is_none() {
            draft.id.clone_from(&named);
        }
        draft.completed = draft.completed.max(millis(row.timestamp));
        if draft.model.is_none() {
            draft.model.clone_from(&row.model);
        }
        if draft.cwd.is_none() {
            draft.cwd.clone_from(&row.cwd);
        }
        if is_failure(row) {
            if let [Content::Error(why)] = row.content.as_slice() {
                draft.error = Some(error_of(why, row.stop_reason.as_ref()));
            }
            continue;
        }
        if let Some(part) = part_of(row, draft) {
            let id = row.source_id.starts_with("prt").then(|| row.source_id.clone());
            draft.parts.push((id, part));
        }
    }

    // An assistant message whose info lacked what its turn is keyed on was keyed on its id.
    for draft in drafts.iter_mut().filter(|draft| draft.assistant && draft.id.is_none()) {
        if let Some(Turn::Message(id)) = draft.turn.as_deref().and_then(Turn::of) {
            draft.id = Some(id.to_owned());
        }
    }
    // A user message's id is the one the assistant message answering it names.
    let mut used: std::collections::HashSet<String> =
        drafts.iter().filter_map(|draft| draft.id.clone()).collect();
    for at in 0..drafts.len() {
        if drafts[at].id.is_some() || drafts[at].assistant {
            continue;
        }
        let answered = drafts[at + 1..]
            .iter()
            .take_while(|draft| draft.assistant)
            .find_map(|draft| draft.parent.clone())
            .filter(|id| id.starts_with("msg") && !used.contains(id));
        if let Some(id) = answered {
            used.insert(id.clone());
            drafts[at].id = Some(id);
        }
    }
    for (n, draft) in drafts.iter_mut().enumerate() {
        if draft.id.is_none() {
            let created = draft_created(draft);
            draft.id = Some(mint("msg", created, &format!("{}/message/{n}", session.id)));
        }
    }

    let mut messages = Vec::with_capacity(drafts.len());
    let mut last_user: Option<String> = None;
    for (n, draft) in drafts.iter().enumerate() {
        let id = draft.id.clone().expect("every draft has an id by now");
        let info = if draft.assistant {
            assistant_info(session, draft, &id, last_user.as_deref())
        } else {
            last_user = Some(id.clone());
            let model = drafts[n + 1..]
                .iter()
                .take_while(|next| next.assistant)
                .find_map(|next| provider_model(session, next));
            user_info(session, draft, &id, model)
        };
        let parts: Vec<Value> = draft
            .parts
            .iter()
            .enumerate()
            .map(|(k, (part_id, part))| {
                let mut part = part.clone();
                let part_id = part_id.clone().unwrap_or_else(|| {
                    mint("prt", draft.created, &format!("{}/part/{id}/{k}", session.id))
                });
                part["id"] = json!(part_id);
                part["sessionID"] = json!(session.id);
                part["messageID"] = json!(id);
                part
            })
            .collect();
        messages.push(json!({"info": info, "parts": parts}));
    }

    let updated = session.messages.iter().map(|m| millis(m.timestamp)).max();
    let started = millis(session.started_at);
    if let Some(of) = &session.fork_of {
        fork(&mut messages, session, of);
    }
    json!({
        "info": {
            "id": session.id,
            "slug": session.id,
            "version": "0.0.0",
            // Import replaces both with those of the directory it runs in.
            "projectID": "",
            "directory": session.cwd,
            "title": session.title.clone().unwrap_or_else(|| title_of(session)),
            "time": {"created": started, "updated": updated.unwrap_or(started).max(started)},
        },
        "messages": messages,
    })
}

/// Make the messages of `session`'s export a fork's of `of`: its first user message (one of its
/// own, ahead of the rest, when it has none) starts with a
/// [fork marker](continuation::fork_marker_text), an `ignored` text part, which opencode keeps
/// from the model and capture reads as the fork's link; and every message and part gets a fresh
/// id. opencode's ids are keys of its whole database, which `opencode import` skips when it
/// holds them already: under the original's, the fork would import empty.
///
/// A fresh id keeps the old one's prefix and time (opencode orders by id), then a tail of the
/// fork's own (from its session id, so a retried import mints the same) and the old id's rank:
/// the fork's ids sort as the original's did.
fn fork(messages: &mut Vec<Value>, session: &RehydrateSession, of: &ForkOf) {
    let source = AnyHarness::from_name("opencode").expect("opencode is a harness");
    let marker = continuation::fork_marker_text(source, &of.id, of.atuin_id.as_deref());
    // With no user message (only the model's replies were captured), the marker gets one of its
    // own ahead of the rest. Not in an assistant message: opencode sends an assistant's text to
    // the model, `ignored` or not, and a user message of only ignored parts not at all.
    if !messages.iter().any(|m| m["info"]["role"] == "user")
        && let Some(first) = messages.first()
    {
        let created = first["info"]["time"]["created"].as_i64().unwrap_or_default();
        let time = first["info"]["id"]
            .as_str()
            .and_then(id_time)
            .unwrap_or_else(|| u64::try_from(created).unwrap_or_default().wrapping_mul(0x1000));
        let model = |field: &str, or: &str| first["info"][field].as_str().unwrap_or(or).to_owned();
        // The user message the first reply answers, which isn't there: this is it now.
        let id = first["info"]["parentID"].as_str().map_or_else(
            || mint_at("msg", time.saturating_sub(1), &format!("{}/fork", session.id)),
            str::to_owned,
        );
        let info = json!({
            "id": id,
            "sessionID": session.id,
            "role": "user",
            "time": {"created": created},
            "agent": "build",
            "model": {
                "providerID": model("providerID", "opencode"),
                "modelID": model("modelID", "unknown"),
            },
        });
        messages.insert(0, json!({"info": info, "parts": []}));
    }
    if let Some(first) = messages.iter_mut().find(|m| m["info"]["role"] == "user") {
        let created = first["info"]["time"]["created"].as_i64().unwrap_or_default();
        let message = first["info"]["id"].clone();
        let parts = first["parts"].as_array_mut().expect("an export's parts are an array");
        // Ahead of every part of the message, the time (and so the order) of each in its id.
        let earliest = parts
            .iter()
            .filter_map(|p| id_time(p["id"].as_str()?))
            .chain([u64::try_from(created).unwrap_or_default().wrapping_mul(0x1000)])
            .min()
            .unwrap_or_default();
        parts.insert(
            0,
            json!({
                "id": mint_at("prt", earliest.saturating_sub(1), &format!("{}/fork", session.id)),
                "sessionID": session.id,
                "messageID": message,
                "type": "text",
                "text": marker,
                "ignored": true,
                "time": {"start": created, "end": created},
            }),
        );
    }

    let mut old: Vec<String> = messages
        .iter()
        .flat_map(|m| {
            let parts = m["parts"].as_array().into_iter().flatten().map(|p| &p["id"]);
            std::iter::once(&m["info"]["id"]).chain(parts)
        })
        .filter_map(|id| id.as_str().map(str::to_owned))
        .collect();
    old.sort();
    old.dedup();
    let tail = mint_at("", 0, &session.id);
    let tail = &tail[tail.len() - 8..];
    let fresh: HashMap<String, String> = old
        .into_iter()
        .enumerate()
        .map(|(rank, id)| {
            // An id not in opencode's own format keeps all of itself, so it sorts as it did.
            let kept = match id_time(&id) {
                Some(_) => &id[..16],
                None => id.as_str(),
            };
            let new = format!("{kept}{tail}{}", base62(rank));
            (id, new)
        })
        .collect();
    for message in messages {
        replace_ids(message, &fresh);
    }
}

/// The time an opencode id holds (its 12 hex digits after the prefix), as [`mint_at`] takes it.
fn id_time(id: &str) -> Option<u64> {
    let (_, rest) = id.split_once('_')?;
    u64::from_str_radix(rest.get(..12)?, 16).ok()
}

/// `n` in six base-62 digits, which sort as the numbers do.
fn base62(mut n: usize) -> String {
    let mut digits = [b'0'; 6];
    for digit in digits.iter_mut().rev() {
        *digit = BASE62[n % 62];
        n /= 62;
    }
    String::from_utf8_lossy(&digits).into_owned()
}

/// The ids of an exported `message` that are keys of `fresh` (its own, the message it answers,
/// and each part's own and its message's), replaced by their values. Only the fields the export
/// writes ids into: what was said, or a tool's input, may hold an id too, and stays as it was.
fn replace_ids(message: &mut Value, fresh: &HashMap<String, String>) {
    let swap = |value: &mut Value, field: &str| {
        if let Some(id) = value.get_mut(field)
            && let Some(new) = id.as_str().and_then(|old| fresh.get(old))
        {
            *id = Value::String(new.clone());
        }
    };
    if let Some(info) = message.get_mut("info") {
        swap(info, "id");
        swap(info, "parentID");
    }
    for part in message.get_mut("parts").and_then(Value::as_array_mut).into_iter().flatten() {
        swap(part, "id");
        swap(part, "messageID");
    }
}

/// When a message was created: an assistant's, as its turn says (to the millisecond, which the
/// turn key of every row of it repeats), else its first row's time.
fn draft_created(draft: &Draft) -> i64 {
    match draft.turn.as_deref().and_then(Turn::of) {
        Some(Turn::Call { created, .. }) => created,
        _ => draft.created,
    }
}

/// The provider and model an assistant message ran on: its turn's, else what its rows or the
/// session say (a model with no provider is taken for opencode's own).
fn provider_model(session: &RehydrateSession, draft: &Draft) -> Option<(String, String)> {
    if let Some(Turn::Call {
        provider, model, ..
    }) = draft.turn.as_deref().and_then(Turn::of)
    {
        return Some((provider.to_owned(), model.to_owned()));
    }
    let model = draft.model.clone().or_else(|| session.model.clone())?;
    Some(match model.split_once('/') {
        Some((provider, model)) => (provider.to_owned(), model.to_owned()),
        None => ("opencode".to_owned(), model),
    })
}

fn user_info(
    session: &RehydrateSession,
    draft: &Draft,
    id: &str,
    model: Option<(String, String)>,
) -> Value {
    let (provider, model) = model
        .or_else(|| provider_model(session, draft))
        .unwrap_or_else(|| ("opencode".to_owned(), "unknown".to_owned()));
    json!({
        "id": id,
        "sessionID": session.id,
        "role": "user",
        "time": {"created": draft.created},
        "agent": "build",
        "model": {"providerID": provider, "modelID": model},
    })
}

fn assistant_info(
    session: &RehydrateSession,
    draft: &Draft,
    id: &str,
    last_user: Option<&str>,
) -> Value {
    let (provider, model) = provider_model(session, draft)
        .unwrap_or_else(|| ("opencode".to_owned(), "unknown".to_owned()));
    let created = draft_created(draft);
    // An assistant message answers a user message; one whose rows did not say answers the last.
    let parent =
        draft.parent.clone().or_else(|| last_user.map(str::to_owned)).unwrap_or_else(|| {
            mint("msg", created.saturating_sub(1), &format!("{}/parent/{id}", session.id))
        });
    let cwd = draft.cwd.as_ref().unwrap_or(&session.cwd);
    let mut info = json!({
        "id": id,
        "sessionID": session.id,
        "role": "assistant",
        "time": {"created": created, "completed": draft.completed.max(created)},
        "parentID": parent,
        "modelID": model,
        "providerID": provider,
        "mode": "build",
        "agent": "build",
        "path": {"cwd": cwd, "root": "/"},
        "cost": 0,
        "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}},
    });
    if let Some((reason, tokens)) = &draft.finish {
        info["finish"] = json!(reason);
        info["tokens"] = tokens.clone();
    }
    if draft.summary {
        info["summary"] = json!(true);
    }
    if let Some(error) = &draft.error {
        info["error"] = error.clone();
    }
    info
}

/// A title for a session capture has none for: the start of the first thing the user said.
fn title_of(session: &RehydrateSession) -> String {
    session
        .messages
        .iter()
        .filter(|row| row.role == Role::User)
        .flat_map(|row| &row.content)
        .find_map(|content| match content {
            Content::Text(text) if !text.trim().is_empty() => {
                Some(text.trim().chars().take(60).collect())
            }
            _ => None,
        })
        .unwrap_or_else(|| "Rehydrated session".to_owned())
}

/// The error a failed message's info reports, as opencode names it (`MessageV2.Assistant.error`),
/// saying `why`.
fn error_of(why: &str, stop: Option<&StopReason>) -> Value {
    match stop {
        Some(StopReason::Aborted) => {
            json!({"name": "MessageAbortedError", "data": {"message": why}})
        }
        Some(StopReason::Refusal) => {
            json!({"name": "ContentFilterError", "data": {"message": why}})
        }
        // Its only field is its name, which is what capture read as the reason.
        Some(StopReason::MaxTokens) if why == "MessageOutputLengthError" => {
            json!({"name": "MessageOutputLengthError", "data": {}})
        }
        _ => json!({"name": "UnknownError", "data": {"message": why}}),
    }
}

/// A `step-finish`'s `reason`, from the stop reason capture read from it.
fn finish_reason(stop: Option<&StopReason>) -> String {
    match stop {
        None | Some(StopReason::EndTurn) => "stop".to_owned(),
        Some(StopReason::MaxTokens) => "length".to_owned(),
        Some(StopReason::ToolUse) => "tool-calls".to_owned(),
        Some(StopReason::Refusal) => "content-filter".to_owned(),
        Some(StopReason::Error) => "error".to_owned(),
        Some(StopReason::Aborted) => "abort".to_owned(),
        Some(StopReason::StopSequence) => "stop".to_owned(),
        Some(StopReason::Other(other)) => other.clone(),
    }
}

/// A `step-finish`'s token counts: those its turn key spells out
/// (`<turn>#input/output/reasoning/cache read/cache write`, opencode's own numbers), else what
/// its usage says, opencode's `output` leaving out the reasoning that usage counts in.
fn step_tokens(row: &RehydrateMessage, usage: &Usage) -> Value {
    let counts: Option<Vec<u64>> =
        row.turn_id.as_deref().and_then(|turn| turn.rsplit_once('#')).and_then(|(_, counts)| {
            counts.split('/').map(str::parse).collect::<Result<_, _>>().ok()
        });
    let [input, output, reasoning, read, write] = match counts.as_deref() {
        Some(&[input, output, reasoning, read, write]) => [input, output, reasoning, read, write],
        _ => {
            let reasoning = usage.reasoning.unwrap_or_default();
            [
                usage.input.unwrap_or_default(),
                usage.output.unwrap_or_default().saturating_sub(reasoning),
                reasoning,
                usage.cache_read.unwrap_or_default(),
                usage.cache_write.unwrap_or_default(),
            ]
        }
    };
    json!({
        "input": input,
        "output": output,
        "reasoning": reasoning,
        "cache": {"read": read, "write": write},
    })
}

/// The part a row is, without its ids; `None` for a row no part can carry.
fn part_of(row: &RehydrateMessage, draft: &mut Draft) -> Option<Value> {
    let at = millis(row.timestamp);
    if row.content.is_empty() {
        let usage = row.usage.as_ref()?;
        let tokens = step_tokens(row, usage);
        let reason = finish_reason(row.stop_reason.as_ref());
        draft.finish = Some((reason.clone(), tokens.clone()));
        return Some(json!({"type": "step-finish", "reason": reason, "cost": 0, "tokens": tokens}));
    }
    match row.content.as_slice() {
        [Content::Text(text)] => {
            let mut part = json!({"type": "text", "text": text, "time": {"start": at, "end": at}});
            // A fork's marker stays kept from the model (see `fork`).
            if row.role == Role::System && continuation::is_fork_marker(text) {
                part["ignored"] = json!(true);
            } else if row.role == Role::System {
                part["synthetic"] = json!(true);
            }
            Some(part)
        }
        [Content::Summary(text)] => {
            draft.summary = true;
            Some(json!({"type": "text", "text": text, "time": {"start": at, "end": at}}))
        }
        [Content::Reasoning(text)] => {
            Some(json!({"type": "reasoning", "text": text, "time": {"start": at, "end": at}}))
        }
        [Content::ToolUse(call)] => Some(tool_part(call, None, at)),
        [Content::ToolUse(call), Content::ToolResult(result)] => {
            Some(tool_part(call, Some(result), at))
        }
        // An API error the call was retried after.
        [Content::Error(why)] => Some(json!({
            "type": "retry",
            "attempt": 1,
            "error": {"name": "APIError", "data": {"message": why, "isRetryable": true}},
            "time": {"created": at},
        })),
        [Content::Other(part)] if part["type"].is_string() => Some(part.clone()),
        _ => None,
    }
}

/// A tool part: `completed` or `error` with its result, `pending` without one.
fn tool_part(call: &ToolUse, result: Option<&ToolResult>, at: i64) -> Value {
    // opencode's tool input is always an object.
    let input = match &call.input {
        Value::Object(_) => call.input.clone(),
        other => Value::Object(Map::from_iter([("input".to_owned(), other.clone())])),
    };
    let text = |output: &Value| match output {
        Value::String(text) => text.clone(),
        Value::Null => UNCAPTURED_OUTPUT.to_owned(),
        other => other.to_string(),
    };
    let state = match result {
        Some(result) if result.error => json!({
            "status": "error",
            "input": input,
            "error": text(&result.output),
            "time": {"start": at, "end": at},
        }),
        Some(result) => json!({
            "status": "completed",
            "input": input,
            "output": text(&result.output),
            "title": "",
            "metadata": {},
            "time": {"start": at, "end": at},
        }),
        None => json!({"status": "pending", "input": input, "raw": ""}),
    };
    json!({"type": "tool", "callID": call.id.as_ref(), "tool": call.name, "state": state})
}

#[cfg(test)]
pub(crate) mod tests {
    use futures::StreamExt;
    use rstest::rstest;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
    use sqlx::{Connection, Executor};

    use super::*;
    use crate::harnesstools::opencode::session::OpencodeSessions;
    use crate::harnesstools::rehydrate::testing;
    use crate::harnesstools::session::{Message, Session, Sessions};

    /// The part of opencode's schema the import writes and capture reads.
    const DDL: &[&str] = &[
        "CREATE TABLE event (id TEXT PRIMARY KEY, aggregate_id TEXT NOT NULL, seq INTEGER NOT \
         NULL, type TEXT NOT NULL, data TEXT NOT NULL)",
        "CREATE UNIQUE INDEX event_aggregate_seq_idx ON event (aggregate_id, seq)",
        "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL, \
         title TEXT NOT NULL, revert TEXT, time_created INTEGER NOT NULL, time_updated INTEGER \
         NOT NULL)",
        "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created \
         INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL)",
        "CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT \
         NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL)",
    ];

    pub async fn database(path: &Path) -> SqliteConnection {
        let opts = SqliteConnectOptions::new().filename(path).create_if_missing(true);
        let mut conn = SqliteConnection::connect_with(&opts).await.unwrap();
        for ddl in DDL {
            conn.execute(*ddl).await.unwrap();
        }
        conn
    }

    /// What `opencode import` does with an export (opencode 1.18.32, `Cli.import.body`): the
    /// session under its id, in the directory import runs in; each message under its id, stamped
    /// with its creation time, its info without `id` and `sessionID` as its data; each part
    /// under its id, in the message its `messageID` names, stamped with the time of the import.
    pub async fn opencode_import(conn: &mut SqliteConnection, export: &Value, cwd: &str) {
        let info = &export["info"];
        let now = millis(OffsetDateTime::now_utc());
        crate::db::query::<sqlx::Sqlite>(
            "INSERT INTO session (id, directory, title, time_created, time_updated) VALUES (?1, \
             ?2, ?3, ?4, ?5) ON CONFLICT DO NOTHING",
        )
        .bind(info["id"].as_str())
        .bind(cwd)
        .bind(info["title"].as_str())
        .bind(info["time"]["created"].as_i64())
        .bind(info["time"]["updated"].as_i64())
        .execute(&mut *conn)
        .await
        .unwrap();
        for message in export["messages"].as_array().unwrap() {
            let mut data = message["info"].clone();
            let data = data.as_object_mut().unwrap();
            let id = data.remove("id").unwrap();
            data.remove("sessionID");
            let created = data["time"]["created"].as_i64();
            crate::db::query::<sqlx::Sqlite>(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES \
                 (?1, ?2, ?3, ?3, ?4) ON CONFLICT DO NOTHING",
            )
            .bind(id.as_str())
            .bind(info["id"].as_str())
            .bind(created)
            .bind(Value::Object(data.clone()).to_string())
            .execute(&mut *conn)
            .await
            .unwrap();
            for part in message["parts"].as_array().unwrap() {
                let mut data = part.clone();
                let data = data.as_object_mut().unwrap();
                let id = data.remove("id").unwrap();
                let message = data.remove("messageID").unwrap();
                data.remove("sessionID");
                crate::db::query::<sqlx::Sqlite>(
                    "INSERT INTO part (id, message_id, session_id, time_created, time_updated, \
                     data) VALUES (?1, ?2, ?3, ?4, ?4, ?5) ON CONFLICT DO NOTHING",
                )
                .bind(id.as_str())
                .bind(message.as_str())
                .bind(info["id"].as_str())
                .bind(now)
                .bind(Value::Object(data.clone()).to_string())
                .execute(&mut *conn)
                .await
                .unwrap();
            }
        }
    }

    /// The rows capture makes of every session in the database at `path`, keyed as it keys them
    /// (opencode gives every row an id of its own).
    pub async fn captured(path: &Path) -> Vec<RehydrateMessage> {
        let sessions: Vec<_> = OpencodeSessions::builder()
            .db(path)
            .build()
            .existing()
            .unwrap()
            .map(Result::unwrap)
            .collect()
            .await;
        let mut rows = Vec::new();
        for session in sessions {
            let messages: Vec<_> = session.read().collect().await;
            for m in messages {
                let m = m.unwrap();
                rows.push(RehydrateMessage {
                    source_id: String::from(m.id().unwrap()),
                    parent_source_id: m.parent_id().map(String::from),
                    timestamp: m.timestamp().unwrap_or(OffsetDateTime::UNIX_EPOCH),
                    role: m.role(),
                    content: m.content(),
                    model: m.model(),
                    usage: m.usage(),
                    stop_reason: m.stop_reason(),
                    turn_id: m.turn_id(),
                    cwd: m.cwd(),
                    git_branch: m.git_branch(),
                });
            }
        }
        rows
    }

    fn keys(rows: &[RehydrateMessage]) -> Vec<(String, Role, Vec<Content>)> {
        rows.iter().map(|r| (r.source_id.clone(), r.role.clone(), r.content.clone())).collect()
    }

    fn session(id: &str, title: Option<&str>, messages: Vec<RehydrateMessage>) -> RehydrateSession {
        RehydrateSession {
            id: id.to_owned(),
            title: title.map(str::to_owned),
            cwd: PathBuf::from("/elsewhere/proj"),
            original_cwd: Some(PathBuf::from("/work/proj")),
            git_branch: None,
            model: None,
            started_at: messages.first().map_or(OffsetDateTime::UNIX_EPOCH, |m| m.timestamp),
            messages,
            fork_of: None,
        }
    }

    /// Captures `original` from `original_db`, writes it back as `opencode import` would and
    /// captures it again: the rows (bar the session's own) and the session's title row.
    async fn round_trip(
        original_db: &Path,
        id: &str,
        title: Option<&str>,
    ) -> (Vec<RehydrateMessage>, Vec<RehydrateMessage>) {
        let original = captured(original_db).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let mut conn = database(&path).await;
        opencode_import(&mut conn, &export(&session(id, title, original.clone())), "/here").await;
        conn.close().await.unwrap();
        (original, captured(&path).await)
    }

    fn is_session(id: &str, row: &RehydrateMessage) -> bool {
        row.source_id.starts_with(&format!("{id}:"))
    }

    /// The session opencode 1.18.32 wrote (the projection fixture), stored in a new database at
    /// `path` as opencode stores it; its id.
    pub async fn load_projection(path: &Path) -> String {
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        let mut conn = database(path).await;
        let id = fixture["session"]["id"].as_str().unwrap();
        let s = &fixture["session"];
        crate::db::query::<sqlx::Sqlite>(
            "INSERT INTO session (id, directory, title, time_created, time_updated) VALUES (?1, \
             ?2, ?3, ?4, ?5) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(s["directory"].as_str())
        .bind(s["title"].as_str())
        .bind(s["time_created"].as_i64())
        .bind(s["time_updated"].as_i64())
        .execute(&mut conn)
        .await
        .unwrap();
        for m in fixture["messages"].as_array().unwrap() {
            crate::db::query::<sqlx::Sqlite>(
                "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES \
                 (?1, ?2, ?3, ?3, ?4)",
            )
            .bind(m["id"].as_str())
            .bind(id)
            .bind(m["time_created"].as_i64())
            .bind(m["data"].to_string())
            .execute(&mut conn)
            .await
            .unwrap();
        }
        for p in fixture["parts"].as_array().unwrap() {
            crate::db::query::<sqlx::Sqlite>(
                "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) \
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            )
            .bind(p["id"].as_str())
            .bind(p["message_id"].as_str())
            .bind(id)
            .bind(p["time_created"].as_i64())
            .bind(p["data"].to_string())
            .execute(&mut conn)
            .await
            .unwrap();
        }
        conn.close().await.unwrap();
        id.to_owned()
    }

    const FIXTURE: &str = include_str!("../../../tests/fixtures/opencode/projection.json");

    /// A session opencode 1.18.32 wrote (the projection fixture capture's own tests read),
    /// captured, written back and captured again, keys the same rows in the same order: its
    /// parts, its failed message, and the title it has now.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_real_session_recaptures_as_the_same_rows() {
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let id = load_projection(&path).await;
        let id = id.as_str();
        let s = &fixture["session"];

        let title = s["title"].as_str();
        let (original, again) = round_trip(&path, id, title).await;
        assert!(original.len() > 5);
        let rows = |rows: &[RehydrateMessage]| -> Vec<RehydrateMessage> {
            rows.iter().filter(|r| !is_session(id, r)).cloned().collect()
        };
        pretty_assertions::assert_eq!(keys(&rows(&again)), keys(&rows(&original)));
        let titles = |rows: &[RehydrateMessage]| -> Vec<String> {
            rows.iter().filter(|r| is_session(id, r)).map(|r| r.source_id.clone()).collect()
        };
        assert_eq!(titles(&again), titles(&original));

        // What the rows say beyond their content comes back too: the model call each belongs
        // to (and so its usage, counted once), the message it answers, the model.
        let calls = |rows: &[RehydrateMessage]| -> Vec<_> {
            rows.iter()
                .filter(|r| !is_session(id, r))
                .map(|r| {
                    (r.turn_id.clone(), r.usage, r.stop_reason.clone(), r.parent_source_id.clone())
                })
                .collect()
        };
        assert_eq!(calls(&again), calls(&original));
    }

    fn row(source_id: &str, at: i64, role: Role, content: Vec<Content>) -> RehydrateMessage {
        RehydrateMessage {
            source_id: source_id.to_owned(),
            parent_source_id: None,
            timestamp: OffsetDateTime::from_unix_timestamp(at).unwrap(),
            role,
            content,
            model: None,
            usage: None,
            stop_reason: None,
            turn_id: None,
            cwd: None,
            git_branch: None,
        }
    }

    /// An assistant row of the call `turn` answering `parent`.
    fn answer(
        source_id: &str,
        at: i64,
        parent: &str,
        turn: &str,
        content: Vec<Content>,
    ) -> RehydrateMessage {
        let mut row = row(source_id, at, Role::Assistant, content);
        row.parent_source_id = Some(parent.to_owned());
        row.turn_id = Some(turn.to_owned());
        row.model = Some("gpt-x".to_owned());
        row.cwd = Some(PathBuf::from("/work/proj"));
        row
    }

    const SES: &str = "ses_0d148d4b0001WQdGo06CVdqA3u";
    const U1: &str = "msg_0d148d4b9001WQdGo06CVdqA3u";
    const U2: &str = "msg_0d149b05a001c4o2UTJB1vX46n";
    const A2: &str = "msg_0d149b2d5001gkfmc9bUMDXLJ5";
    const TURN1: &str = "1790217606979:openai/gpt-x";
    const TURN2: &str = "1790217663189:openai/gpt-x";

    /// A session with every kind of row capture makes of opencode's parts.
    fn every_kind() -> Vec<RehydrateMessage> {
        let mut finish = answer(
            "prt_0d148db35001Zcq9788cC4lx83",
            1_790_217_608,
            U1,
            &format!("{TURN1}#802/42/10/200/0"),
            vec![],
        );
        finish.usage = Some(Usage {
            input: Some(802),
            output: Some(52),
            cache_read: Some(200),
            cache_write: Some(0),
            reasoning: Some(10),
        });
        finish.stop_reason = Some(StopReason::ToolUse);
        let mut failure =
            answer(A2, 1_790_217_663, U2, TURN2, vec![Content::Error("mock bad request".into())]);
        failure.stop_reason = Some(StopReason::Error);
        let step_start = json!({"type": "step-start", "id": "prt_0d148db28001TznIjdIVqXAvJ3",
                                "messageID": "msg_0d148d743001qhQZH5hIpZVrlb", "sessionID": SES});
        vec![
            row(&format!("{SES}:title:Fix the build"), 1_790_217_606, Role::System, vec![]),
            row("prt_0d148d4bf001JaxA9TwaMgU5vE", 1_790_217_606, Role::User, vec![Content::Text(
                "fix the build".into(),
            )]),
            row("prt_0d148d4c0001JaxA9TwaMgU5vF", 1_790_217_606, Role::System, vec![
                Content::Text("Called the Read tool".into()),
            ]),
            answer("prt_0d148db28001TznIjdIVqXAvJ3", 1_790_217_607, U1, TURN1, vec![
                Content::Other(step_start),
            ]),
            answer("prt_0d148db29001TznIjdIVqXAvJ4", 1_790_217_607, U1, TURN1, vec![
                Content::Reasoning("thinking it over".into()),
            ]),
            answer("prt_0d148db2a001TznIjdIVqXAvJ5", 1_790_217_607, U1, TURN1, vec![
                Content::Error("overloaded".into()),
            ]),
            answer("prt_0d148db2c001S3pTr5s7fXT3RP", 1_790_217_607, U1, TURN1, vec![
                Content::Text("On it.".into()),
            ]),
            answer("prt_0d148db2d001S3pTr5s7fXT3RQ", 1_790_217_607, U1, TURN1, vec![
                Content::ToolUse(ToolUse {
                    id: "call_1".to_owned().into(),
                    name: "bash".into(),
                    input: json!({"command": "cargo build"}),
                }),
                Content::ToolResult(ToolResult {
                    call: "call_1".to_owned().into(),
                    output: json!("ok"),
                    error: false,
                }),
            ]),
            answer("prt_0d148db2e001S3pTr5s7fXT3RR", 1_790_217_607, U1, TURN1, vec![
                Content::ToolUse(ToolUse {
                    id: "call_2".to_owned().into(),
                    name: "read".into(),
                    input: json!({"path": "nope"}),
                }),
                Content::ToolResult(ToolResult {
                    call: "call_2".to_owned().into(),
                    output: json!("no such file"),
                    error: true,
                }),
            ]),
            finish,
            row("prt_0d149b05c0015TIlARpqWlDXNK", 1_790_217_662, Role::User, vec![Content::Text(
                "and again".into(),
            )]),
            failure,
        ]
    }

    /// Every kind of row comes back under its own id, in its message: the user message the
    /// assistant's answers is named for it, the assistant's for the part that carried its id
    /// or the failure keyed on it.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_kind_of_row_recaptures_as_itself() {
        let rows = every_kind();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let exported = export(&session(SES, Some("Fix the build"), rows.clone()));
        let mut conn = database(&path).await;
        opencode_import(&mut conn, &exported, "/here").await;
        conn.close().await.unwrap();
        let again = captured(&path).await;
        pretty_assertions::assert_eq!(keys(&again), keys(&rows));
        assert_eq!(again[9].usage, rows[9].usage);
        assert_eq!(again[9].turn_id, rows[9].turn_id);
        assert_eq!(again[11].stop_reason, Some(StopReason::Error));

        let ids: Vec<&str> = exported["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["info"]["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [U1, "msg_0d148d743001qhQZH5hIpZVrlb", U2, A2]);
        let assistant = &exported["messages"][1]["info"];
        assert_eq!(assistant["parentID"], U1);
        assert_eq!(assistant["providerID"], "openai");
        assert_eq!(assistant["time"]["created"], 1_790_217_606_979_i64);
        assert_eq!(assistant["finish"], "tool-calls");
        assert_eq!(exported["messages"][0]["info"]["model"]["providerID"], "openai");
    }

    /// Checks an export as strictly as the APIs behind opencode check what it sends from it:
    /// every tool part calls on an input capture kept (an object, never the `{"input": null}`
    /// standing in for none), and a finished one's output says something.
    fn assert_the_api_takes(exported: &Value) {
        for part in exported["messages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|m| m["parts"].as_array().unwrap().iter().filter(|p| p["type"] == "tool"))
        {
            let state = &part["state"];
            assert!(state["input"].is_object(), "{part}");
            assert_ne!(state["input"], json!({"input": null}), "a call on no input: {part}");
            if state["status"] == "completed" {
                assert!(state["output"].as_str().is_some_and(|o| !o.is_empty()), "{part}");
            }
        }
    }

    /// What capture syncs keeps no call's input and no output. Written back, each such call is a
    /// text part saying what was done, which the APIs take; re-captured, every part reads back
    /// under an id already synced (so nothing is pushed), a text part it joined with its note.
    #[rstest]
    #[tokio::test]
    async fn synced_calls_come_back_as_notes_the_api_takes() {
        let dir = tempfile::tempdir().unwrap();
        let synced = testing::synced(every_kind());
        assert!(testing::uncaptured(&synced) > 0, "the session makes calls");
        let exported = export(&session(SES, Some("Fix the build"), synced.clone()));
        assert_the_api_takes(&exported);
        let path = dir.path().join("opencode.db");
        let mut conn = database(&path).await;
        opencode_import(&mut conn, &exported, "/here").await;
        conn.close().await.unwrap();

        let again = captured(&path).await;
        testing::assert_nothing_new(&synced, again.iter().map(|m| m.source_id.as_str()));
        let flattened = flatten_uncaptured_calls(&synced, &Flatten::Notes {
            host: &|host, row| {
                host.parent_source_id == row.parent_source_id && turn_base(host) == turn_base(row)
            },
            own: &|_| true,
        });
        // A part left with nothing is not written, but for a model call's `step-finish`; nor is
        // reasoning capture kept no text of.
        let expected: Vec<RehydrateMessage> = flattened
            .into_iter()
            .filter(|m| !m.content.is_empty() || m.usage.is_some() || is_session(SES, m))
            .filter(|m| !matches!(m.content.as_slice(), [Content::ReasoningSummary { .. }]))
            .collect();
        pretty_assertions::assert_eq!(keys(&testing::synced(again)), keys(&expected));
    }

    /// A tool call opencode never finished is written as one it shows interrupted; minted ids
    /// are opencode's shape and the same every time.
    #[rstest]
    fn an_unfinished_call_is_pending_and_minted_ids_are_stable() {
        let rows = vec![
            row("prt_a", 10, Role::User, vec![Content::Text("go".into())]),
            answer("prt_b", 11, "msg_x", "msg_y", vec![Content::ToolUse(ToolUse {
                id: "call_1".to_owned().into(),
                name: "bash".into(),
                input: json!("ls"),
            })]),
        ];
        let s = session(SES, None, rows);
        let exported = export(&s);
        assert_eq!(exported, export(&s));
        let part = &exported["messages"][1]["parts"][0];
        assert_eq!(part["state"]["status"], "pending");
        assert_eq!(part["state"]["input"], json!({"input": "ls"}));
        // The assistant message said its id (its turn fell back to it), the user message's is
        // the one it answers.
        assert_eq!(exported["messages"][1]["info"]["id"], "msg_y");
        assert_eq!(exported["messages"][0]["info"]["id"], "msg_x");
        assert_eq!(exported["info"]["title"], "go");

        let minted = mint("msg", 1_790_217_606_979, "seed");
        assert!(minted.starts_with("msg_"));
        assert_eq!(minted.len(), "msg_".len() + 26);
        assert!(minted < mint("msg", 1_790_217_606_980, "other"));
    }

    /// A fork's messages and parts get fresh ids, and only its ids: a prompt, or a tool's input,
    /// that names an id of the original keeps it as written.
    #[rstest]
    fn a_fork_renames_ids_not_what_was_said() {
        let said = format!("why did {U1} and prt_a fail?");
        let rows = vec![
            row("prt_a", 10, Role::User, vec![Content::Text(said.clone())]),
            answer("prt_b", 11, U1, "msg_y", vec![Content::ToolUse(ToolUse {
                id: "call_1".to_owned().into(),
                name: "bash".into(),
                input: json!(U1),
            })]),
        ];
        let mut s = session(SES, None, rows);
        s.fork_of = Some(ForkOf {
            id: "ses_original".to_owned(),
            atuin_id: None,
            path: None,
        });
        let exported = export(&s);
        let messages = exported["messages"].as_array().unwrap();
        let (user, assistant) = (&messages[0], &messages[1]);

        let ids: Vec<&str> = messages
            .iter()
            .flat_map(|m| {
                let parts = m["parts"].as_array().unwrap().iter();
                std::iter::once(&m["info"]["id"]).chain(parts.map(|p| &p["id"]))
            })
            .map(|id| id.as_str().unwrap())
            .collect();
        for old in [U1, "msg_y", "prt_a", "prt_b"] {
            assert!(!ids.contains(&old), "{old} renamed: {ids:?}");
        }
        assert_eq!(assistant["info"]["parentID"], user["info"]["id"]);
        let prompt = user["parts"].as_array().unwrap().iter().find(|p| p["ignored"].is_null());
        assert_eq!(prompt.unwrap()["text"], said.as_str());
        let call = &assistant["parts"][0];
        assert_eq!(call["messageID"], assistant["info"]["id"]);
        assert_eq!(call["state"]["input"], json!({"input": U1}));
    }

    /// A directory programs can be run from, next to the test binary: CI's temporary directory is
    /// mounted without exec, so a fake program written there could not be run.
    #[cfg(unix)]
    fn exec_dir() -> tempfile::TempDir {
        let exe = std::env::current_exe().unwrap();
        tempfile::tempdir_in(exe.parent().unwrap()).unwrap()
    }

    /// A fake `opencode`: records how it was run, then does what `$FAKE_EXIT` says.
    #[cfg(unix)]
    fn fake_opencode(dir: &Path) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let bin = exec_dir();
        let path = bin.path().join("opencode");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$@\" > {log}/args\npwd > {log}/cwd\necho \"$OPENCODE_DB \
                 $OPENCODE_DISABLE_MODELS_FETCH $OPENCODE_PURE\" > {log}/env\ncp \"$3\" \
                 {log}/export.json\necho boom >&2\nexit 3\n",
                log = dir.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin, path)
    }

    /// `opencode import` runs from the session's directory, against the database it was given,
    /// with the network-free environment; its failure is reported, and nothing is left behind.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn import_runs_opencode_from_the_session_directory() {
        let dir = tempfile::tempdir().unwrap();
        let (_bin, program) = fake_opencode(dir.path());
        let db = dir.path().join("opencode.db");
        let mut s = session(SES, Some("t"), every_kind());
        s.cwd = dir.path().canonicalize().unwrap();

        let err = import(&program, &db, &s).await.unwrap_err();
        assert!(matches!(&err, RehydrateError::Other(why) if why.contains("boom")), "{err}");
        let read = |name: &str| std::fs::read_to_string(dir.path().join(name)).unwrap();
        let args = read("args");
        assert!(args.starts_with("import --pure "), "{args}");
        assert_eq!(read("cwd").trim(), s.cwd.to_str().unwrap());
        assert_eq!(read("env").trim(), format!("{} 1 1", db.display()));
        let sent: Value = serde_json::from_str(&read("export.json")).unwrap();
        assert_eq!(sent, export(&s));
        let file = args.trim().rsplit(' ').next().unwrap();
        assert!(!Path::new(file).exists(), "the export file is removed");
    }

    #[rstest]
    #[tokio::test]
    async fn a_session_opencode_already_holds_is_not_imported_again() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let mut conn = database(&db).await;
        opencode_import(&mut conn, &export(&session(SES, None, every_kind())), "/here").await;
        conn.close().await.unwrap();
        let missing = dir.path().join("no-such-opencode");
        let err = import(&missing, &db, &session(SES, None, vec![])).await.unwrap_err();
        assert!(matches!(err, RehydrateError::AlreadyExists(path) if path == db));
    }

    /// An import that failed part-way (the session written, not all of its messages and parts)
    /// is told apart from a whole one, and from one worked on since.
    #[rstest]
    #[tokio::test]
    async fn how_much_of_a_session_opencode_holds() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let exported = export(&session(SES, None, every_kind()));
        let mut conn = database(&db).await;
        opencode_import(&mut conn, &exported, "/here").await;
        assert_eq!(held(&db, SES, &exported).await, Held::Whole);

        // The import failed after the first message: the rest of it never came.
        let first = exported["messages"][0]["info"]["id"].as_str().unwrap();
        let kept_part = exported["messages"][0]["parts"][0]["id"].as_str().unwrap();
        for sql in ["DELETE FROM message WHERE id != ?1", "DELETE FROM part WHERE message_id != ?1"]
        {
            crate::db::query::<sqlx::Sqlite>(sql).bind(first).execute(&mut conn).await.unwrap();
        }
        crate::db::query::<sqlx::Sqlite>("DELETE FROM part WHERE id != ?1")
            .bind(kept_part)
            .execute(&mut conn)
            .await
            .unwrap();
        assert_eq!(held(&db, SES, &exported).await, Held::Partly);

        // Imported again, it is whole.
        opencode_import(&mut conn, &exported, "/here").await;
        assert_eq!(held(&db, SES, &exported).await, Held::Whole);

        // A revert pending: what is missing may be what it takes back.
        crate::db::query::<sqlx::Sqlite>("DELETE FROM part WHERE id = ?1")
            .bind(kept_part)
            .execute(&mut conn)
            .await
            .unwrap();
        crate::db::query::<sqlx::Sqlite>("UPDATE session SET revert = '{}' WHERE id = ?1")
            .bind(SES)
            .execute(&mut conn)
            .await
            .unwrap();
        assert_eq!(held(&db, SES, &exported).await, Held::Changed);

        // A revert committed and the session carried on: a message the export doesn't have.
        crate::db::query::<sqlx::Sqlite>("UPDATE session SET revert = NULL WHERE id = ?1")
            .bind(SES)
            .execute(&mut conn)
            .await
            .unwrap();
        assert_eq!(held(&db, SES, &exported).await, Held::Partly);
        crate::db::query::<sqlx::Sqlite>(
            "INSERT INTO message (id, session_id, time_created, time_updated, data) VALUES \
             ('msg_new', ?1, 0, 0, '{}')",
        )
        .bind(SES)
        .execute(&mut conn)
        .await
        .unwrap();
        assert_eq!(held(&db, SES, &exported).await, Held::Changed);
        conn.close().await.unwrap();
    }

    /// A fake `opencode` that succeeds, saying it ran in `ran`.
    #[cfg(unix)]
    fn succeeding_opencode(dir: &Path) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let bin = exec_dir();
        let path = bin.path().join("opencode");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh
touch {}/ran
",
                dir.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin, path)
    }

    /// A session an earlier import left partly written is imported again, rather than taken for
    /// one already there; a whole one is not.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn a_partly_imported_session_is_imported_again() {
        let dir = tempfile::tempdir().unwrap();
        let (_bin, program) = succeeding_opencode(dir.path());
        let db = dir.path().join("opencode.db");
        let mut s = session(SES, None, every_kind());
        s.cwd = dir.path().canonicalize().unwrap();
        let exported = export(&s);
        let mut conn = database(&db).await;
        opencode_import(&mut conn, &exported, "/here").await;

        let err = import(&program, &db, &s).await.unwrap_err();
        assert!(matches!(err, RehydrateError::AlreadyExists(ref path) if *path == db), "{err}");
        assert!(!dir.path().join("ran").exists(), "a whole session is not imported again");

        crate::db::query::<sqlx::Sqlite>("DELETE FROM part").execute(&mut conn).await.unwrap();
        conn.close().await.unwrap();
        assert_eq!(import(&program, &db, &s).await.unwrap(), db);
        assert!(dir.path().join("ran").exists(), "opencode import ran again");
    }

    #[rstest]
    #[case::not_opencodes("abc")]
    #[case::a_path("ses/../x")]
    #[tokio::test]
    async fn an_id_opencode_would_not_take_is_refused(#[case] id: &str) {
        let dir = tempfile::tempdir().unwrap();
        let err = import(Path::new("opencode"), &dir.path().join("db"), &session(id, None, vec![]))
            .await
            .unwrap_err();
        assert!(matches!(err, RehydrateError::Other(_)));
    }
}
