//! opencode: the session's rows in opencode's database.
//!
//! **Tip.** opencode keeps a session as one line of messages, and `opencode --session`
//! continues after the last: the last row in the order it reads them (messages by creation time
//! and id, parts by id; see `projection`). Its projection is what counts, not the event log
//! capture follows, which still holds what a revert removed since.
//!
//! **Append.** `opencode import` of the rows past the local tail only, built by the rehydrate
//! export builder. Import inserts what it is given (each message and part skipped when opencode
//! already holds its id), so importing the whole synced session would bring back messages a
//! revert removed here. A part of a message this copy already holds goes into that message, matched
//! on its opencode id where a row names it, not a new one. A reply no row names (most: capture keys
//! a reply on the prompt it answers and when it was created) is never taken for one held here: if
//! this copy holds a reply to the same prompt from the same millisecond, which it is can't be told,
//! and it is no fast-forward ([`SyncError::Unsupported`]: a copy that stopped mid-reply is forked).
//! Neither is a message that would land before the local last one (opencode would read it in among
//! the others by creation time and id), nor a session with a revert pending, which would remove
//! what is appended.
//!
//! **Liveness.** opencode records no running session anywhere: any opencode running here in the
//! session's directory (or below it, or where that can't be told) may be writing it; so may one
//! anywhere whose command line names the session (`opencode -s <id>`), and while opencode updated
//! the session in the last [`RECENT`](super::RECENT) (the latest `time_updated` of it and its
//! messages and parts), any opencode at all. While one does, a session whose last reply opencode
//! has not stamped `time.completed` is being written now ([`Liveness::Live`]). An unfinished last
//! reply alone is not: a copy restore or catch-up wrote from a reply synced half-way keeps it
//! unfinished for good.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use super::liveness::{agent_running, changed_lately};
use super::{
    AppendOptions, AppendOutcome, Dirs, Links, Liveness, LocalTip, Processes, Seen, SessionSync,
    Stamp, SyncError, blocking_liveness, check_segment, require_idle,
};
use crate::harnesstools::opencode::Opencode;
use crate::harnesstools::opencode::rehydrate::{export_minting, run_import};
use crate::harnesstools::opencode::session::projection::{Projected, projected};
use crate::harnesstools::opencode::session::{self, default_db};
use crate::harnesstools::rehydrate::{RehydrateMessage, RehydrateSession};

impl SessionSync for Opencode {
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError> {
        let Some(db) = default_db().filter(|db| db.is_file()) else {
            return Ok(None);
        };
        local_tip_in(&db, id).await
    }

    async fn is_live(&self, id: &str, cwd: Option<&Path>) -> Liveness {
        let db = default_db().filter(|db| db.is_file());
        liveness_in(db.as_deref(), id, Processes::here(), cwd).await
    }

    async fn append(
        &self,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        require_idle(running(Processes::here(), appending(base, id, options.cwd)).await)?;
        let into = base.native_path.clone();
        append_in(id, base, lines, options, move |cwd: PathBuf, export: Value| async move {
            run_import(Path::new("opencode"), &into, &cwd, &export)
                .await
                .map(drop)
                .map_err(|e| SyncError::Other(e.to_string()))
        })
        .await
    }
}

/// Whether an opencode process may be writing the session `seen`: [`Liveness::Unknown`] while
/// any that may be runs ([`agent_running`]).
pub(super) fn liveness(procs: &Processes, seen: &Seen) -> Liveness {
    agent_running(procs, "opencode", &[], seen)
}

/// Session `id`, as [`SessionSync::append`] to the database `base` read takes it, last seen
/// working in `cwd`: an opencode names it on its command line (`opencode -s <id>`).
fn appending(base: &LocalTip, id: &str, cwd: Option<&Path>) -> Seen {
    base.seen(cwd, vec![id.to_owned()])
}

/// [`liveness`], read off the async runtime.
async fn running(procs: Processes, seen: Seen) -> Liveness {
    blocking_liveness(move || liveness(&procs, &seen)).await
}

/// When opencode last updated the session `found`.
fn updated(found: &Projected) -> Option<SystemTime> {
    let ms = u64::try_from(found.updated?).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(ms))
}

/// [`SessionSync::is_live`] of session `id` in the database `db`: [`liveness`], and
/// [`Liveness::Live`] rather than [`Liveness::Unknown`] when the session's last reply is
/// unfinished ([`Projected::replying`]) while an opencode that may be writing it runs.
pub(super) async fn liveness_in(
    db: Option<&Path>,
    id: &str,
    procs: Processes,
    cwd: Option<&Path>,
) -> Liveness {
    let found = match db {
        Some(db) => projected(db, id).await,
        None => Ok(None),
    };
    let seen = Seen {
        dirs: Dirs::of(cwd.map(Path::to_path_buf)),
        names: vec![id.to_owned()],
        // A database that can't be read may hold it changing.
        changing: match &found {
            Ok(Some(found)) => changed_lately(updated(found)),
            Ok(None) => false,
            Err(_) => true,
        },
    };
    let running = running(procs, seen).await;
    if running == Liveness::Unknown && found.is_ok_and(|found| found.is_some_and(|f| f.replying()))
    {
        return Liveness::Live { pid: None };
    }
    running
}

async fn read(db: &Path, id: &str) -> Result<Projected, SyncError> {
    match projected(db, id).await {
        Ok(Some(found)) => Ok(found),
        Ok(None) => Err(SyncError::NotFound),
        Err(err) => Err(SyncError::Other(format!("reading opencode's database: {err}"))),
    }
}

/// What `found` holds: its directory, whether a revert is pending, and every message and part
/// with its content ([`Projected::content`]), so a row edited in place changes it too.
fn stamp(found: &Projected) -> Stamp {
    let mut text =
        format!("{}\n{}\n{:016x}\n", found.directory.display(), found.reverting, found.content);
    for message in &found.messages {
        text.push_str(&format!("{}@{}\n", message.id, message.created));
    }
    for row in &found.rows {
        text.push_str(row);
        text.push('\n');
    }
    Stamp::of(text.as_bytes())
}

/// [`SessionSync::local_tip`] of session `id` in the database `db`.
pub(super) async fn local_tip_in(db: &Path, id: &str) -> Result<Option<LocalTip>, SyncError> {
    match projected(db, id).await {
        Ok(Some(found)) => Ok(Some(LocalTip {
            native_path: db.to_path_buf(),
            known_source_ids: found.rows.iter().cloned().collect(),
            merged: HashMap::new(),
            tip_source_id: found.rows.last().cloned(),
            stamp: stamp(&found),
            modified: updated(&found),
            cwd: Some(found.directory.clone()),
        })),
        Ok(None) if session::locate(db, id).await.is_some() => {
            Err(SyncError::Unsupported("opencode 2.0 sessions can't be caught up yet"))
        }
        Ok(None) => Ok(None),
        Err(err) => Err(SyncError::Other(format!("reading opencode's database: {err}"))),
    }
}

/// [`SessionSync::append`] to session `id` in the database `base` read, once it is known to be
/// idle, with `import` standing for `opencode import` (of an export, from a directory).
pub(super) async fn append_in<F, Fut>(
    id: &str,
    base: &LocalTip,
    lines: &[RehydrateMessage],
    options: &AppendOptions<'_>,
    import: F,
) -> Result<AppendOutcome, SyncError>
where
    F: FnOnce(PathBuf, Value) -> Fut,
    Fut: Future<Output = Result<(), SyncError>>,
{
    let db = &base.native_path;
    let current = read(db, id).await?;
    if stamp(&current) != base.stamp {
        return Err(SyncError::Changed);
    }
    if current.reverting {
        return Err(SyncError::Unsupported(
            "the session has a revert pending in opencode, which would remove what is caught up",
        ));
    }
    // opencode's rows name no parent: the tail continues the session as a line.
    check_segment(lines, base, options.taken_ids, Links::Linear)?;
    if lines.is_empty() {
        return Ok(AppendOutcome {
            native_path: db.clone(),
            appended: Vec::new(),
            tip_source_id: base.tip_source_id.clone(),
        });
    }
    if !current.directory.is_dir() {
        return Err(SyncError::Other(format!(
            "{} is gone: opencode would move the session to wherever it is imported from",
            current.directory.display()
        )));
    }
    let session = RehydrateSession {
        id: id.to_owned(),
        title: None,
        cwd: current.directory.clone(),
        original_cwd: None,
        git_branch: None,
        model: None,
        started_at: time::OffsetDateTime::from_unix_timestamp_nanos(
            i128::from(current.created) * 1_000_000,
        )
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH),
        messages: lines.to_vec(),
        fork_of: None,
    };
    let (mut export, minted) = export_minting(&session);
    continue_messages(&mut export, &current, &minted)?;
    import(current.directory.clone(), export).await?;

    let after = read(db, id).await?;
    let now: HashSet<&str> = after.rows.iter().map(String::as_str).collect();
    let appended: Vec<String> = lines
        .iter()
        .map(|row| row.source_id.clone())
        .filter(|id| now.contains(id.as_str()) && !base.known_source_ids.contains(id))
        .collect();
    if appended.is_empty() {
        return Err(SyncError::Other("opencode import wrote none of the rows".to_owned()));
    }
    Ok(AppendOutcome {
        native_path: db.clone(),
        appended,
        tip_source_id: after.rows.last().cloned(),
    })
}

/// Point the parts `export` rebuilt of a message opencode already holds at that message, and
/// check that every new message comes after the session's last.
///
/// A message is known by its opencode id, which `export` recovers where a row names it, and only
/// by it. One whose id it had to mint (none of its rows named it, in `minted`) is never taken for
/// a message opencode holds: if an assistant's, and opencode holds a reply to the same prompt
/// created in the same millisecond (what capture's turn key keeps) that no other message of the
/// export is by id, it may be that reply or a distinct one, which can't be told
/// ([`SyncError::Unsupported`]: the session is forked instead); otherwise it is a new message.
fn continue_messages(
    export: &mut Value,
    current: &Projected,
    minted: &HashSet<String>,
) -> Result<(), SyncError> {
    let ids: HashSet<&str> = current.messages.iter().map(|m| m.id.as_str()).collect();
    let messages = export["messages"].as_array_mut().map(Vec::as_mut_slice).unwrap_or_default();
    // Messages of the export known by their own id: none of them is a minted one.
    let claimed: HashSet<&str> = messages
        .iter()
        .filter_map(|m| m["info"]["id"].as_str())
        .filter(|id| !minted.contains(*id) && ids.contains(id))
        .collect();
    let held: HashSet<(Option<&str>, i64)> = current
        .messages
        .iter()
        .filter(|m| m.assistant && !claimed.contains(m.id.as_str()))
        .map(|m| (m.parent.as_deref(), m.created))
        .collect();
    // opencode reads messages by creation time, then id (`ORDER BY time_created, id`, compared
    // bytewise as Rust compares strings): a new message sorting before the last one in that
    // order would be read in among the others.
    let last = current.messages.last().map(|m| (m.created, m.id.as_str()));
    let mut found = Vec::with_capacity(messages.len());
    for message in messages.iter() {
        let info = &message["info"];
        let id = info["id"].as_str().unwrap_or_default();
        let created = info["time"]["created"].as_i64().unwrap_or_default();
        if minted.contains(id)
            && info["role"] == "assistant"
            && held.contains(&(info["parentID"].as_str(), created))
        {
            return Err(SyncError::Unsupported(
                "opencode holds a reply to the prompt from the same millisecond: whether these \
                 rows belong to it can't be told",
            ));
        }
        let existing = ids.contains(id).then(|| id.to_owned());
        if existing.is_none() && last.is_some_and(|last| (created, id) < last) {
            return Err(SyncError::Unsupported(
                "opencode keeps a session as one line: these messages would land before its last",
            ));
        }
        found.push(existing);
    }
    for (message, existing) in messages.iter_mut().zip(found) {
        if let Some(existing) = existing {
            message["info"]["id"] = Value::from(existing.clone());
            for part in message["parts"].as_array_mut().into_iter().flatten() {
                part["messageID"] = Value::from(existing.clone());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
