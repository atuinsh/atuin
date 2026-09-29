//! opencode: the session's rows in opencode's database.
//!
//! **Tip.** opencode keeps a session as one line of messages, and `opencode --session`
//! continues after the last: the last row in the order it reads them (messages by creation time
//! and id, parts by id; see `projection`). Its projection is what counts, not the event log
//! capture follows, which still holds what a revert removed since.
//!
//! **Append.** `opencode import` of the rows past the local tail only, built by the rehydrate
//! export builder. Import inserts what it is given (the session updated in place, each message
//! and part skipped when opencode already holds its id), so importing the whole synced session
//! would bring back messages a revert removed here. A part of a message this copy already holds
//! goes into that message (matched on the prompt it answers and when it was created, which
//! capture's turn key keeps), not a new one. The session's `time_updated` is not taken from the
//! export for a session opencode already holds: import updates the session row in place, which
//! stamps it with the time of the import (seen with opencode 1.18.32), so the caught-up session
//! lists first and `--continue` picks it. `opencode --session <id>` opens a session by its id
//! whatever it says.
//!
//! A branch that leaves this copy's line before its end can't be appended: opencode would read
//! its messages in with the others by creation time. The caller forks instead.
//!
//! Import writes the session row a row at a time, so a failed import can leave some rows
//! written; appending the same rows again imports what is missing and skips the rest.
//!
//! **Liveness.** opencode records no running session anywhere; its processes are looked for.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::liveness::{opencode_claim, scan};
use super::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, Processes, SessionSync, Stamp, SyncError,
    require_idle,
};
use crate::harnesstools::opencode::Opencode;
use crate::harnesstools::opencode::rehydrate::{export, run_import};
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
        let mut cwd = cwd.map(Path::to_path_buf);
        if cwd.is_none()
            && let Some(db) = default_db()
            && let Ok(Some(found)) = projected(&db, id).await
        {
            cwd = Some(found.directory);
        }
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || liveness(&Processes::here(), &id, cwd.as_deref()))
            .await
            .unwrap_or(Liveness::Unknown)
    }

    async fn append(
        &self,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        let db = base.native_path.clone();
        let current = read(&db, id).await?;
        require_idle(self.is_live(id, Some(&current.directory)).await)?;
        let into = db.clone();
        append_in(&db, id, base, lines, options, move |cwd: PathBuf, export: Value| async move {
            run_import(Path::new("opencode"), &into, &cwd, &export)
                .await
                .map(drop)
                .map_err(|e| SyncError::Other(e.to_string()))
        })
        .await
    }
}

/// Whether an opencode process may be writing session `id`, which works in `cwd`.
pub(super) fn liveness(procs: &Processes, id: &str, cwd: Option<&Path>) -> Liveness {
    match procs.list() {
        Ok(processes) => scan(&processes, cwd, |p| opencode_claim(p, id)),
        Err(_) => Liveness::Unknown,
    }
}

async fn read(db: &Path, id: &str) -> Result<Projected, SyncError> {
    match projected(db, id).await {
        Ok(Some(found)) => Ok(found),
        Ok(None) => Err(SyncError::NotFound),
        Err(err) => Err(SyncError::Other(format!("reading opencode's database: {err}"))),
    }
}

fn stamp(found: &Projected) -> Stamp {
    let mut text = format!("{}\n{}\n", found.directory.display(), found.reverting);
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
            merged: std::collections::HashMap::new(),
            tip_source_id: found.rows.last().cloned(),
            stamp: stamp(&found),
        })),
        Ok(None) if session::locate(db, id).await.is_some() => {
            Err(SyncError::Unsupported("opencode 2.0 sessions can't be caught up yet"))
        }
        Ok(None) => Ok(None),
        Err(err) => Err(SyncError::Other(format!("reading opencode's database: {err}"))),
    }
}

/// [`SessionSync::append`] to session `id` in the database `db`, once it is known to be idle,
/// with `import` standing for `opencode import` (of an export, from a directory).
pub(super) async fn append_in<F, Fut>(
    db: &Path,
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
    let current = read(db, id).await?;
    if stamp(&current) != base.stamp {
        return Err(SyncError::Changed);
    }
    if current.reverting {
        return Err(SyncError::Unsupported(
            "the session has a revert pending in opencode, which would remove what is caught up",
        ));
    }
    let known: HashSet<&str> = current.rows.iter().map(String::as_str).collect();
    let taken =
        |id: &str| known.contains(id) || options.taken_ids.is_some_and(|taken| taken.contains(id));
    if let Some(clash) = lines.iter().find(|row| taken(&row.source_id)) {
        return Err(SyncError::IdTaken(clash.source_id.clone()));
    }
    let tip = current.rows.last().cloned();
    if lines.is_empty() {
        return match options.head {
            Some(head) if Some(head) != tip.as_deref() => Err(SyncError::Unsupported(
                "opencode keeps a session as one line: only its last message can be continued",
            )),
            _ => Ok(AppendOutcome {
                native_path: db.to_path_buf(),
                appended: Vec::new(),
                tip_source_id: tip,
                marked_tip: false,
            }),
        };
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
    };
    let mut export = export(&session);
    continue_messages(&mut export, &current)?;
    if !current.directory.is_dir() {
        return Err(SyncError::Other(format!(
            "{} is gone: opencode would move the session to wherever it is imported from",
            current.directory.display()
        )));
    }
    import(current.directory.clone(), export).await?;

    let after = read(db, id).await?;
    let now: HashSet<&str> = after.rows.iter().map(String::as_str).collect();
    let appended: Vec<String> = lines
        .iter()
        .map(|row| row.source_id.clone())
        .filter(|id| now.contains(id.as_str()) && !known.contains(id.as_str()))
        .collect();
    if appended.is_empty() {
        return Err(SyncError::Other("opencode import wrote none of the rows".to_owned()));
    }
    Ok(AppendOutcome {
        native_path: db.to_path_buf(),
        appended,
        tip_source_id: after.rows.last().cloned(),
        marked_tip: false,
    })
}

/// Point the parts `export` rebuilt of a message opencode already holds at that message, and
/// check that every new message comes after the session's last.
fn continue_messages(export: &mut Value, current: &Projected) -> Result<(), SyncError> {
    let held: HashMap<(Option<&str>, i64), &str> = current
        .messages
        .iter()
        .filter(|m| m.assistant)
        .map(|m| ((m.parent.as_deref(), m.created), m.id.as_str()))
        .collect();
    let ids: HashSet<&str> = current.messages.iter().map(|m| m.id.as_str()).collect();
    let last = current.messages.last().map(|m| m.created);
    for message in export["messages"].as_array_mut().into_iter().flatten() {
        let info = &message["info"];
        let created = info["time"]["created"].as_i64().unwrap_or_default();
        let key = (info["parentID"].as_str(), created);
        let existing = (info["role"] == "assistant")
            .then(|| held.get(&key).copied())
            .flatten()
            .or_else(|| info["id"].as_str().filter(|id| ids.contains(id)));
        match existing {
            Some(existing) => {
                let existing = existing.to_owned();
                message["info"]["id"] = Value::from(existing.clone());
                for part in message["parts"].as_array_mut().into_iter().flatten() {
                    part["messageID"] = Value::from(existing.clone());
                }
            }
            None if last.is_some_and(|last| created < last) => {
                return Err(SyncError::Unsupported(
                    "opencode keeps a session as one line: these messages would land before its \
                     last",
                ));
            }
            None => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
