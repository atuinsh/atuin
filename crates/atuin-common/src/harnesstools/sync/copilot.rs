//! Copilot: the log `<sessions>/<session>/events.jsonl`.
//!
//! **Tip.** Copilot continues a session from the end of its log, whatever the last event is: the
//! row it continues from is the last the reader yields (an event after it is bookkeeping, which
//! says nothing of the conversation). Each row names the row before it ([`Links::Parents`]); a
//! title row, the sidecar's, none, and neither does a row captured before the reader linked
//! them, which follows the one before it (`chained`), so a copy catches up from a machine whose
//! atuin reads Copilot the old way.
//!
//! **Append.** The rows are written by the rehydrate writer, chained to the log's last event as
//! Copilot chains its own, under the log's own session id; Copilot then resumes from the last
//! of them. The sidecar is left as it is: a title among the rows is the session's name, which
//! the sidecar holds already when it is the same, and which a switch keeps.
//!
//! **Switch.** The events kept are the log's up to the event the branch leaves it at; the rest
//! (the events that went on from there, bookkeeping included) are dropped, and the branch's rows
//! past it are appended as an append appends them. Copilot then resumes from the last of them.
//!
//! **Liveness.** A Copilot process that has a session open keeps `inuse.<pid>.lock` in the
//! session's directory, with its pid as the content, and removes it on exit: a lock whose
//! process runs says the session is open, wherever the process works (`copilot --resume`
//! reopens a session from anywhere). A lock a crash left behind names a process that is gone,
//! and a log without one is no one's, however new.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::Value;

use super::{
    AppendOptions, AppendOutcome, Links, Liveness, LocalTip, Processes, ReplaceOutcome,
    SessionSync, Stamp, SyncError, append_jsonl, blocking, blocking_liveness, branch_point,
    capture_key, check_segment, continuing_from, jsonl_lines, kept_lines, modified, past,
    replace_jsonl, require_idle, resumes_from, switched_to, with_merged,
};
use crate::harnesstools::copilot::session::{default_root, locate, read_log, session_dir};
use crate::harnesstools::copilot::{Copilot, rehydrate};
use crate::harnesstools::rehydrate::{RehydrateMessage, RehydrateSession};
use crate::harnesstools::session::{Message, SessionId};

impl SessionSync for Copilot {
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError> {
        let (root, id) = (default_root(), id.to_owned());
        blocking(move || locate(&root, &id).map(|path| read_tip(&path)).transpose()).await
    }

    async fn is_live(&self, id: &str, _cwd: Option<&Path>) -> Liveness {
        let Some(dir) = session_dir(&default_root(), id) else {
            return Liveness::NotLive;
        };
        blocking_liveness(move || liveness(&dir, &Processes::here())).await
    }

    async fn append(
        &self,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        require_idle(self.is_live(id, None).await)?;
        let (id, base, lines) = (id.to_owned(), base.clone(), lines.to_vec());
        let taken = options.taken_ids.cloned();
        blocking(move || append_to(&id, &base, &lines, taken.as_ref())).await
    }

    async fn replace(
        &self,
        id: &str,
        base: &LocalTip,
        branch: &[RehydrateMessage],
        options: &AppendOptions<'_>,
        backups: &Path,
    ) -> Result<ReplaceOutcome, SyncError> {
        require_idle(self.is_live(id, None).await)?;
        let (id, base, branch) = (id.to_owned(), base.clone(), branch.to_vec());
        let (taken, backups) = (options.taken_ids.cloned(), backups.to_path_buf());
        blocking(move || replace_to(&id, &base, &branch, taken.as_ref(), &backups)).await
    }
}

/// Whether a Copilot process has the session in `dir` open (see the module docs).
fn liveness(dir: &Path, procs: &Processes) -> Liveness {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Liveness::NotLive;
    };
    let mut unreadable = false;
    for pid in entries.filter_map(Result::ok).filter_map(|entry| lock_pid(&entry.path())) {
        match procs.running(pid) {
            Ok(Some(_)) => return Liveness::Live { pid: Some(pid) },
            Ok(None) => {}
            Err(_) => unreadable = true,
        }
    }
    if unreadable {
        Liveness::Unknown
    } else {
        Liveness::NotLive
    }
}

/// The pid an `inuse.<pid>.lock` file names, from its name.
fn lock_pid(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("inuse.")?.strip_suffix(".lock")?.parse().ok()
}

/// The session id a log's directory is named with.
fn session_of(path: &Path) -> SessionId {
    let dir = path.parent().and_then(Path::file_name);
    SessionId::from(dir.map(|d| d.to_string_lossy().into_owned()).unwrap_or_default())
}

/// What the log at `path` holds.
pub(super) fn read_tip(path: &Path) -> Result<LocalTip, SyncError> {
    let bytes = std::fs::read(path)?;
    Ok(tip_of(path, &bytes))
}

/// What the log at `path`, holding `bytes`, holds: each row under the id capture gives it (its
/// event's; a title row's, keyed on the sidecar's name, which a log read whole yields with its
/// first line), and the row Copilot continues from, the last.
fn tip_of(path: &Path, bytes: &[u8]) -> LocalTip {
    let session = session_of(path);
    let mut occurrences = HashMap::new();
    let mut known = HashSet::new();
    let mut tip = None;
    let mut cwd = None;
    for message in read_log(path, bytes) {
        cwd = message.cwd().or(cwd);
        if let Some(key) = capture_key(&session, &message, &mut occurrences) {
            // A title row is the sidecar's, no event Copilot continues from.
            if message.title().is_none() {
                tip = Some(key.clone());
            }
            known.insert(key);
        }
    }
    let (known, merged) = with_merged(known, bytes);
    LocalTip {
        native_path: path.to_path_buf(),
        known_source_ids: known,
        merged,
        tip_source_id: tip,
        stamp: Stamp::of(bytes),
        modified: modified(path),
        cwd,
        unswitchable: None,
    }
}

/// The id of the log's last event, which an appended event is chained to.
fn last_event(bytes: &[u8]) -> Option<String> {
    jsonl_lines(bytes)
        .rev()
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .find_map(|line| line["id"].as_str().map(str::to_owned))
}

/// [`SessionSync::append`] to the log `base` read, once the session is known to be idle.
pub(super) fn append_to(
    id: &str,
    base: &LocalTip,
    lines: &[RehydrateMessage],
    taken: Option<&HashSet<String>>,
) -> Result<AppendOutcome, SyncError> {
    let path = &base.native_path;
    if session_of(path).as_ref() != id {
        return Err(SyncError::Other(format!("{} is not session {id}", path.display())));
    }
    let current = std::fs::read(path)?;
    if Stamp::of(&current) != base.stamp {
        return Err(SyncError::Changed);
    }
    let (out, appended) = extend(id, &current, base, lines, taken)?;
    let Some(last) = appended.last().cloned() else {
        return Ok(AppendOutcome {
            native_path: path.clone(),
            appended,
            tip_source_id: base.tip_source_id.clone(),
        });
    };
    append_jsonl(path, base.stamp, &out)?;
    Ok(AppendOutcome {
        native_path: path.clone(),
        appended,
        tip_source_id: Some(last),
    })
}

/// The events that append `lines` to the log of session `id` holding `current` (read as
/// `base`), and the source ids of the rows written as events of their own; refused unless they
/// fast-forward it.
fn extend(
    id: &str,
    current: &[u8],
    base: &LocalTip,
    lines: &[RehydrateMessage],
    taken: Option<&HashSet<String>>,
) -> Result<(Vec<Value>, Vec<String>), SyncError> {
    check_segment(lines, base, taken, Links::Parents { chained: true })?;
    let session = RehydrateSession {
        id: id.to_owned(),
        title: None,
        cwd: base.cwd.clone().unwrap_or_default(),
        original_cwd: lines.iter().find_map(|m| m.cwd.clone()),
        git_branch: None,
        model: None,
        started_at: time::OffsetDateTime::now_utc(),
        messages: lines.to_vec(),
        fork_of: None,
    };
    let after = last_event(current);
    let out = rehydrate::lines(&session, after.as_deref());
    let rows: HashSet<&str> = lines.iter().map(|m| m.source_id.as_str()).collect();
    let appended = out
        .iter()
        .filter_map(|l| l["id"].as_str())
        .filter(|id| rows.contains(id))
        .map(str::to_owned)
        .collect();
    Ok((out, appended))
}

/// The log holding `current`, with `lines` appended (as [`append_jsonl`] appends them).
fn joined(current: &[u8], lines: &[Value]) -> Vec<u8> {
    let mut after = current.to_vec();
    if !after.is_empty() && !after.ends_with(b"\n") {
        after.push(b'\n');
    }
    for line in lines {
        after.extend(line.to_string().bytes());
        after.push(b'\n');
    }
    after
}

/// [`SessionSync::replace`] of the log `base` read, once the session is known to be idle: the
/// events up to the one the branch leaves it at are kept, and the branch's rows past it
/// appended to them.
pub(super) fn replace_to(
    id: &str,
    base: &LocalTip,
    branch: &[RehydrateMessage],
    taken: Option<&HashSet<String>>,
    backups: &Path,
) -> Result<ReplaceOutcome, SyncError> {
    let path = &base.native_path;
    if session_of(path).as_ref() != id {
        return Err(SyncError::Other(format!("{} is not session {id}", path.display())));
    }
    if branch.is_empty() {
        return Err(SyncError::Unsupported("the branch has no rows"));
    }
    let current = std::fs::read(path)?;
    if Stamp::of(&current) != base.stamp {
        return Err(SyncError::Changed);
    }
    let (shared, from) = branch_point(base, branch, Links::Parents { chained: true })?;
    let kept = shared_lines(&current, &from);
    let continuing = continuing_from(tip_of(path, &kept), &from)?;
    let rest = past(branch, shared, &continuing, Links::Parents { chained: true });
    let (lines, appended) = extend(id, &kept, &continuing, &rest, taken)?;
    let out = joined(&kept, &lines);
    let written = tip_of(path, &out);
    resumes_from(written.tip_source_id.as_deref(), &written.merged, switched_to(&rest, &from))?;
    let (backup, warning) = replace_jsonl(path, base.stamp, &current, &out, backups)?;
    Ok(ReplaceOutcome {
        native_path: path.clone(),
        appended,
        tip_source_id: written.tip_source_id,
        backup,
        warning,
    })
}

/// The events of the log `bytes` a switch to a branch that leaves it at the event `from` keeps:
/// all up to and including `from`'s. Rows the reader keys on their content (a title's) are no
/// event of the log, and are no `from`.
fn shared_lines(bytes: &[u8], from: &str) -> Vec<u8> {
    let mut past = false;
    kept_lines(bytes, |line| {
        if past {
            return Some(false);
        }
        let id = serde_json::from_slice::<Value>(line).ok()?["id"].as_str()?.to_owned();
        if id == from {
            past = true;
        }
        Some(true)
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;
    use crate::harnesstools::copilot::session::EVENTS_FILE;
    use crate::harnesstools::rehydrate::RehydrateSession;
    use crate::harnesstools::session::{Content, Role};

    const SESSION: &str = "5e55e55e-0000-4000-8000-000000000001";

    /// A synced row, `role` `id`, written at second `at`.
    fn row(id: &str, role: Role, at: i64) -> RehydrateMessage {
        RehydrateMessage {
            source_id: id.to_owned(),
            parent_source_id: None,
            timestamp: OffsetDateTime::from_unix_timestamp(1_790_000_000 + at).unwrap(),
            role,
            content: vec![Content::Text(format!("said {id}"))],
            model: Some("gpt-x".to_owned()),
            provider: None,
            usage: None,
            stop_reason: None,
            turn_id: None,
            cwd: Some(std::path::PathBuf::from("/work/proj")),
            git_branch: None,
        }
    }

    /// Synced rows `ids`, each naming the one before it as its parent, the first `after`.
    fn rows(ids: &[&str], after: Option<&str>) -> Vec<RehydrateMessage> {
        let mut parent = after.map(str::to_owned);
        ids.iter()
            .enumerate()
            .map(|(n, id)| {
                let role = if id.starts_with('u') {
                    Role::User
                } else {
                    Role::Assistant
                };
                let mut row = row(id, role, i64::try_from(n).unwrap());
                row.parent_source_id = parent.replace((*id).to_owned());
                row
            })
            .collect()
    }

    /// A log of `rows`, written as a restore writes one, under `root`.
    fn written(root: &Path, rows: Vec<RehydrateMessage>) -> std::path::PathBuf {
        let session = RehydrateSession {
            id: SESSION.to_owned(),
            title: Some("the session".to_owned()),
            cwd: std::path::PathBuf::from("/work/proj"),
            original_cwd: None,
            git_branch: None,
            model: None,
            started_at: OffsetDateTime::from_unix_timestamp(1_790_000_000).unwrap(),
            messages: rows,
            fork_of: None,
        };
        rehydrate::rehydrate_into(root, &session).unwrap()
    }

    /// The ids capture reads from the log at `path`, in order, the sidecar's title aside.
    fn read_back(path: &Path) -> Vec<String> {
        read_log(path, &std::fs::read(path).unwrap())
            .iter()
            .filter(|m| m.title().is_none())
            .filter_map(|m| m.id().map(String::from))
            .collect()
    }

    /// The tip is the last row, and every row and the sidecar's title are known.
    #[rstest]
    fn the_tip_is_the_last_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = written(dir.path(), rows(&["u1", "a1", "u2", "a2"], None));
        let tip = read_tip(&path).unwrap();
        assert_eq!(tip.tip_source_id.as_deref(), Some("a2"));
        for id in ["u1", "a1", "u2", "a2"] {
            assert!(tip.known_source_ids.contains(id), "{id}");
        }
        assert!(tip.known_source_ids.iter().any(|id| id.starts_with("syn-title-")), "the title");
        assert_eq!(tip.cwd.as_deref(), Some(Path::new("/work/proj")));
        assert_eq!(tip.unswitchable, None);
        assert!(tip.merged.is_empty());
    }

    /// Rows appended continue the log from its end: Copilot then resumes from the last of
    /// them, and capture reads them under their ids after the ones it holds.
    #[rstest]
    fn appending_fast_forwards_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = written(dir.path(), rows(&["u1", "a1"], None));
        let base = read_tip(&path).unwrap();
        let more = rows(&["u2", "a2"], Some("a1"));
        let out = append_to(SESSION, &base, &more, None).unwrap();
        assert_eq!(out.appended, ["u2", "a2"]);
        assert_eq!(out.tip_source_id.as_deref(), Some("a2"));
        assert_eq!(read_back(&path), ["u1", "a1", "u2", "a2"]);
        let again = read_tip(&path).unwrap();
        assert_eq!(again.tip_source_id.as_deref(), Some("a2"));
        // Chained to the event before it.
        let lines: Vec<Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        for pair in lines.windows(2) {
            assert_eq!(pair[1]["parentId"], pair[0]["id"]);
        }
        // Held already: refused.
        let held = rows(&["a2"], Some("a2"));
        assert!(matches!(append_to(SESSION, &again, &held, None), Err(SyncError::IdTaken(_))));
        // Changed since read: refused.
        std::fs::write(&path, "{}\n").unwrap();
        assert!(matches!(
            append_to(SESSION, &again, &rows(&["u3"], Some("a2")), None),
            Err(SyncError::Changed)
        ));
    }

    /// Rows another machine captured before the reader linked them name no parent: they
    /// still fast-forward the log, following the one before them.
    #[rstest]
    fn rows_from_before_links_still_fast_forward() {
        let dir = tempfile::tempdir().unwrap();
        let path = written(dir.path(), rows(&["u1", "a1"], None));
        let base = read_tip(&path).unwrap();
        let mut legacy = rows(&["u2", "a2", "u3"], None);
        legacy.iter_mut().for_each(|r| r.parent_source_id = None);
        let out = append_to(SESSION, &base, &legacy, None).unwrap();
        assert_eq!(out.appended, ["u2", "a2", "u3"]);
        assert_eq!(read_back(&path), ["u1", "a1", "u2", "a2", "u3"]);
    }

    /// A switch keeps the events up to where the branch leaves the log, drops the rest, and
    /// appends the branch's rows past it; the copy as it was is backed up whole.
    #[rstest]
    fn switching_keeps_the_shared_history_and_backs_the_rest_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = written(dir.path(), rows(&["u1", "a1", "u2", "a2"], None));
        let before = std::fs::read(&path).unwrap();
        let base = read_tip(&path).unwrap();
        let mut branch = rows(&["u1", "a1"], None);
        branch.extend(rows(&["u3", "a3"], Some("a1")).into_iter().map(|mut r| {
            r.timestamp += time::Duration::seconds(10);
            r
        }));
        let backups = dir.path().join("backups");
        let out = replace_to(SESSION, &base, &branch, None, &backups).unwrap();
        assert_eq!(out.appended, ["u3", "a3"]);
        assert_eq!(out.tip_source_id.as_deref(), Some("a3"));
        assert_eq!(read_back(&path), ["u1", "a1", "u3", "a3"]);
        assert_eq!(std::fs::read(&out.backup).unwrap(), before);
        assert!(out.warning.is_none());
        // Sharing nothing: refused.
        let again = read_tip(&path).unwrap();
        assert!(matches!(
            replace_to(SESSION, &again, &rows(&["u9"], None), None, &backups),
            Err(SyncError::Unsupported(_))
        ));
    }

    #[rstest]
    #[case::lock("inuse.22629.lock", Some(22629))]
    #[case::hold("inuse.22629.hold", None)]
    #[case::other("events.jsonl", None)]
    #[case::not_a_pid("inuse.x.lock", None)]
    fn lock_files_name_their_pid(#[case] name: &str, #[case] want: Option<u32>) {
        assert_eq!(lock_pid(&Path::new("/s").join(name)), want);
    }

    /// Copilot holds a lock on every session it has open, so a log without one, however new,
    /// is no one's.
    #[rstest]
    fn a_session_with_no_lock_is_not_live() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(EVENTS_FILE), "{}\n").unwrap();
        assert_eq!(liveness(dir.path(), &Processes::here()), Liveness::NotLive);
    }

    /// A lock whose process runs says the session is open; one a crash left behind says
    /// nothing.
    #[rstest]
    fn a_lock_names_the_process_that_has_the_session_open() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(EVENTS_FILE), "{}\n").unwrap();
        std::fs::write(dir.path().join("inuse.4242.lock"), "4242").unwrap();
        let table = |pids: &[u32]| {
            Processes::Table(
                pids.iter()
                    .map(|&pid| super::super::TableEntry {
                        pid,
                        name: "copilot".into(),
                        cmd: vec!["copilot".into()],
                        start_time: 1,
                        zombie: false,
                        cwd: None,
                    })
                    .collect(),
            )
        };
        assert_eq!(liveness(dir.path(), &table(&[4242])), Liveness::Live { pid: Some(4242) });
        assert_eq!(liveness(dir.path(), &table(&[1])), Liveness::NotLive, "a stale lock");
        assert_eq!(liveness(dir.path(), &Processes::Unavailable), Liveness::Unknown);
    }
}
