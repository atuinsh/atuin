//! Kiro: the session directory `<sessions>/<bucket>/<session>/`, its log `messages.jsonl` and
//! its header `session.json`.
//!
//! **Tip.** Kiro continues a session from the end of its log, whatever the last row is: the row
//! it continues from is the last the reader yields (a row after it is a turn's end or Kiro's
//! bookkeeping, which says nothing of the conversation). Each row names the row before it
//! ([`Links::Parents`]); a title row, the header's, names none, and neither does a row captured
//! before the reader linked them, which follows the one before it (`chained`), so a copy catches
//! up from a machine whose atuin reads Kiro the old way.
//!
//! **Append.** The rows are written by the rehydrate writer, each turn framed as Kiro frames
//! one, under the log's own session id; Kiro then resumes from the last of them. The header's
//! `lastModifiedAt` is bumped, as Kiro bumps it after an append of its own; its title is left as
//! it is: a title among the rows is the session's name, which the header holds already when it
//! is the same, and which a switch keeps.
//!
//! **Switch.** The rows kept are the log's up to the row the branch leaves it at, with the
//! bookkeeping right after it (its turn's end, the usage, the system prompt Kiro writes after a
//! first turn); the rest (the rows that went on from there) are dropped, and the branch's rows
//! past it are appended as an append appends them. Kiro then resumes from the last of them.
//!
//! **Liveness.** Kiro keeps no lock on a session it has open. Its header's `status` is `idle`
//! between turns (and `completed` or `failed` once a session is over) and `in_progress` or
//! `waiting_on_user` while a turn runs (seen mid-turn in 2.36.0): a running turn whose header or
//! log changed in the last [`RECENT`](super::RECENT) says a Kiro has the session, wherever it
//! runs (`kiro-cli chat --resume-id` reopens a session from anywhere). A Kiro that died mid-turn
//! leaves `in_progress` behind for good, so a stale one says only that someone may. Kiro rewrites
//! the header in place, so one that doesn't parse is being written. Between turns, a Kiro at its
//! prompt says nothing in the session's files, so the processes say ([`agent_running`]): a chat
//! (`kiro-cli-chat`, and the agent engine it runs under node from `kiro-cli/kas/`) working in
//! the session's directory or where the caller last saw it, one naming the session on its
//! command line (`--resume-id`), or, while the log or header still changes, any at all, may have
//! it; none says no one has.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::liveness::{agent_running, changed_lately};
use super::{
    AppendOptions, AppendOutcome, Dirs, Links, Liveness, LocalTip, Processes, RehydrateMessage,
    ReplaceOutcome, Seen, SessionSync, Stamp, SyncError, append_jsonl, blocking, blocking_liveness,
    branch_point, capture_key, check_segment, continuing_from, kept_lines, modified, past,
    replace_jsonl, require_idle, resumes_from, switched_to, with_merged,
};
use crate::harnesstools::kiro::session::{
    HEADER_FILE, LOG_FILE, default_root, locate, read_log, session_dir,
};
use crate::harnesstools::kiro::{Kiro, rehydrate};
use crate::harnesstools::rehydrate::RehydrateSession;
use crate::harnesstools::session::{Message, SessionId};

/// How Kiro's rows link (see the module docs).
const LINKS: Links = Links::Parents { chained: true };

impl SessionSync for Kiro {
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError> {
        let (root, id) = (default_root(), id.to_owned());
        blocking(move || locate(&root, &id).map(|path| read_tip(&path)).transpose()).await
    }

    async fn is_live(&self, id: &str, cwd: Option<&Path>) -> Liveness {
        let root = default_root();
        let (id, cwd) = (id.to_owned(), cwd.map(Path::to_path_buf));
        blocking_liveness(move || match session_dir(&root, &id) {
            Some(dir) => liveness(&dir, &id, cwd.as_deref(), &Processes::here()),
            None => Liveness::NotLive,
        })
        .await
    }

    async fn append(
        &self,
        id: &str,
        base: &LocalTip,
        lines: &[RehydrateMessage],
        options: &AppendOptions<'_>,
    ) -> Result<AppendOutcome, SyncError> {
        require_idle(self.is_live(id, options.cwd).await)?;
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
        require_idle(self.is_live(id, options.cwd).await)?;
        let (id, base, branch) = (id.to_owned(), base.clone(), branch.to_vec());
        let (taken, backups) = (options.taken_ids.cloned(), backups.to_path_buf());
        blocking(move || replace_to(&id, &base, &branch, taken.as_ref(), &backups)).await
    }
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
/// own; a title row's, keyed on the header's title, which a log read whole yields with its first
/// line), and the row Kiro continues from, the last.
fn tip_of(path: &Path, bytes: &[u8]) -> LocalTip {
    let session = session_of(path);
    let mut occurrences = HashMap::new();
    let mut known = HashSet::new();
    let mut tip = None;
    let mut cwd = None;
    for message in read_log(path, bytes) {
        cwd = message.cwd().or(cwd);
        if let Some(key) = capture_key(&session, &message, &mut occurrences) {
            // A title row is the header's, no row Kiro continues from.
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
    touch_header(path);
    Ok(AppendOutcome {
        native_path: path.clone(),
        appended,
        tip_source_id: Some(last),
    })
}

/// Bump the header's `lastModifiedAt` beside the log at `log`, as Kiro does after writing rows;
/// a header that can't be read is left alone (the reader copes without one).
fn touch_header(log: &Path) {
    let path = log.with_file_name(HEADER_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut header) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    header["lastModifiedAt"] = Value::String(rehydrate::timestamp(time::OffsetDateTime::now_utc()));
    let Ok(bytes) = serde_json::to_vec_pretty(&header) else {
        return;
    };
    if let Err(e) = crate::fs::replace(&path, &bytes, |_| true) {
        tracing::warn!(path = %path.display(), %e, "couldn't bump a Kiro header's lastModifiedAt");
    }
}

/// The rows that append `lines` to the log of session `id` holding `current` (read as `base`),
/// and the source ids of the rows written as rows of their own; refused unless they fast-forward
/// it.
fn extend(
    id: &str,
    current: &[u8],
    base: &LocalTip,
    lines: &[RehydrateMessage],
    taken: Option<&HashSet<String>>,
) -> Result<(Vec<Value>, Vec<String>), SyncError> {
    check_segment(lines, base, taken, LINKS)?;
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
    let out = rehydrate::lines(&session, Some(current));
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
/// rows up to the one the branch leaves it at are kept, and the branch's rows past it appended
/// to them.
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
    let (shared, from) = branch_point(base, branch, LINKS)?;
    let kept = shared_lines(&current, &from);
    let continuing = continuing_from(tip_of(path, &kept), &from)?;
    let rest = past(branch, shared, &continuing, LINKS);
    let (lines, appended) = extend(id, &kept, &continuing, &rest, taken)?;
    let out = joined(&kept, &lines);
    let written = tip_of(path, &out);
    resumes_from(written.tip_source_id.as_deref(), &written.merged, switched_to(&rest, &from))?;
    let (backup, warning) = replace_jsonl(path, base.stamp, &current, &out, backups)?;
    touch_header(path);
    Ok(ReplaceOutcome {
        native_path: path.clone(),
        appended,
        tip_source_id: written.tip_source_id,
        backup,
        warning,
    })
}

/// The lines of the log `bytes` a switch to a branch that leaves it at the row `from` keeps: all
/// up to and including `from`'s, and the bookkeeping right after it (its turn's end, Kiro's own
/// rows) up to the next row of the conversation. Rows the reader keys on their content (a
/// title's) are no line of the log, and are no `from`.
fn shared_lines(bytes: &[u8], from: &str) -> Vec<u8> {
    let (mut past, mut dropped) = (false, false);
    kept_lines(bytes, |line| {
        let line = serde_json::from_slice::<Value>(line).ok()?;
        if past {
            dropped |= rehydrate::conversation(&line["payload"]);
            return Some(!dropped);
        }
        if line["id"].as_str()? == from {
            past = true;
        }
        Some(true)
    })
}

/// The program a Kiro chat runs as, and the path the agent engine it runs (a node program) is
/// kept under; neither `kiro-cli` itself (whose `kiro-cli-term` shell integration and desktop
/// helper run all day) nor the launcher, which runs only while the chat does.
const CHAT: &str = "kiro-cli-chat";
const ENGINE: &str = "kiro-cli/kas/";

/// What the header's `status` says of the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// A turn is running.
    Busy,
    /// Between turns, or over.
    Idle,
    /// No header.
    Missing,
    /// A header that can't be read, or a status this reader doesn't know.
    Unreadable,
}

/// The header's status, and the directory it says the session works in.
fn status(header: &Path) -> (Status, Option<PathBuf>) {
    let text = match std::fs::read_to_string(header) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (Status::Missing, None),
        Err(_) => return (Status::Unreadable, None),
    };
    let Ok(header) = serde_json::from_str::<Value>(&text) else {
        return (Status::Unreadable, None);
    };
    let status = match header["status"].as_str() {
        Some("in_progress" | "waiting_on_user") => Status::Busy,
        Some("idle" | "completed" | "failed") | None => Status::Idle,
        Some(_) => Status::Unreadable,
    };
    (status, header["workspacePaths"][0].as_str().map(PathBuf::from))
}

/// Whether a Kiro has session `id` in `dir`, last seen working in `cwd`, open (see the module
/// docs).
fn liveness(dir: &Path, id: &str, cwd: Option<&Path>, procs: &Processes) -> Liveness {
    let header = dir.join(HEADER_FILE);
    let changing =
        changed_lately(modified(&dir.join(LOG_FILE))) || changed_lately(modified(&header));
    let (status, works_in) = status(&header);
    match status {
        // A turn's progress rewrites the header and appends to the log.
        Status::Busy if changing => Liveness::Live { pid: None },
        Status::Busy | Status::Unreadable => Liveness::Unknown,
        Status::Idle | Status::Missing => {
            let dirs: Vec<PathBuf> = works_in
                .into_iter()
                .chain(cwd.map(Path::to_path_buf))
                .filter(|dir| dir.is_dir())
                .collect();
            let seen = Seen {
                dirs: if dirs.is_empty() {
                    Dirs::Any
                } else {
                    Dirs::Within(dirs)
                },
                names: vec![id.to_owned()],
                changing,
            };
            agent_running(procs, CHAT, &[ENGINE], &seen)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;
    use crate::harnesstools::session::{Content, Role};
    use crate::harnesstools::sync::TableEntry;
    use crate::harnesstools::sync::liveness::tests::{entry, table};

    const HEADER: &str = include_str!("../../../tests/fixtures/kiro/probe.session.json");
    const SESSION: &str = "sess_5e55e55e-0000-4000-8000-000000000001";
    /// The session the fixture's header is of.
    const PROBE: &str = "sess_0550199b-937b-4dd4-94e0-6901bd990fb0";

    /// A synced row, `role` `id`, written at second `at`.
    fn row(id: &str, role: Role, at: i64) -> RehydrateMessage {
        RehydrateMessage {
            source_id: id.to_owned(),
            parent_source_id: None,
            timestamp: OffsetDateTime::from_unix_timestamp(1_790_000_000 + at).unwrap(),
            role,
            content: vec![Content::Text(format!("said {id}"))],
            model: Some("auto".to_owned()),
            provider: None,
            usage: None,
            stop_reason: None,
            turn_id: None,
            cwd: Some(PathBuf::from("/work/proj")),
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

    /// A log of `rows`, written as a restore writes one, under the Kiro home `home`.
    fn written(home: &Path, rows: Vec<RehydrateMessage>) -> PathBuf {
        let session = RehydrateSession {
            id: SESSION.to_owned(),
            title: Some("the session".to_owned()),
            cwd: PathBuf::from("/work/proj"),
            original_cwd: None,
            git_branch: None,
            model: None,
            started_at: OffsetDateTime::from_unix_timestamp(1_790_000_000).unwrap(),
            messages: rows,
            fork_of: None,
        };
        rehydrate::rehydrate_into(home, &session).unwrap()
    }

    /// The ids capture reads from the log at `path`, in order, the header's title aside.
    fn read_back(path: &Path) -> Vec<String> {
        read_log(path, &std::fs::read(path).unwrap())
            .iter()
            .filter(|m| m.title().is_none())
            .filter_map(|m| m.id().map(String::from))
            .collect()
    }

    /// The payload types of the log at `path`, in order.
    fn kinds(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|l| {
                serde_json::from_str::<Value>(l).unwrap()["payload"]["type"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    fn last_modified(path: &Path) -> String {
        let header = std::fs::read_to_string(path.with_file_name(HEADER_FILE)).unwrap();
        serde_json::from_str::<Value>(&header).unwrap()["lastModifiedAt"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// The tip is the last row, and every row and the header's title are known.
    #[rstest]
    fn the_tip_is_the_last_row() {
        let home = tempfile::tempdir().unwrap();
        let path = written(home.path(), rows(&["u1", "a1", "u2", "a2"], None));
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

    /// Rows appended continue the log from its end, as a turn of their own after the last turn's
    /// end: Kiro then resumes from the last of them, capture reads them under their ids after
    /// the ones it holds, and the header says the session changed.
    #[rstest]
    fn appending_fast_forwards_the_log() {
        let home = tempfile::tempdir().unwrap();
        let path = written(home.path(), rows(&["u1", "a1"], None));
        let base = read_tip(&path).unwrap();
        let before = last_modified(&path);
        let more = rows(&["u2", "a2"], Some("a1"));
        let out = append_to(SESSION, &base, &more, None).unwrap();
        assert_eq!(out.appended, ["u2", "a2"]);
        assert_eq!(out.tip_source_id.as_deref(), Some("a2"));
        assert_eq!(read_back(&path), ["u1", "a1", "u2", "a2"]);
        assert_eq!(kinds(&path), [
            "user",
            "turn_start",
            "assistant",
            "turn_end",
            "user",
            "turn_start",
            "assistant",
            "turn_end"
        ]);
        assert_ne!(last_modified(&path), before, "the header says the session changed");
        let again = read_tip(&path).unwrap();
        assert_eq!(again.tip_source_id.as_deref(), Some("a2"));
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
        let home = tempfile::tempdir().unwrap();
        let path = written(home.path(), rows(&["u1", "a1"], None));
        let base = read_tip(&path).unwrap();
        let mut legacy = rows(&["u2", "a2", "u3"], None);
        legacy.iter_mut().for_each(|r| r.parent_source_id = None);
        let out = append_to(SESSION, &base, &legacy, None).unwrap();
        assert_eq!(out.appended, ["u2", "a2", "u3"]);
        assert_eq!(read_back(&path), ["u1", "a1", "u2", "a2", "u3"]);
    }

    /// A switch keeps the rows up to where the branch leaves the log, their turn's end with
    /// them, drops the rest, and appends the branch's rows past it; the copy as it was is backed
    /// up whole.
    #[rstest]
    fn switching_keeps_the_shared_history_and_backs_the_rest_up() {
        let home = tempfile::tempdir().unwrap();
        let path = written(home.path(), rows(&["u1", "a1", "u2", "a2"], None));
        let before = std::fs::read(&path).unwrap();
        let base = read_tip(&path).unwrap();
        let mut branch = rows(&["u1", "a1"], None);
        branch.extend(rows(&["u3", "a3"], Some("a1")).into_iter().map(|mut r| {
            r.timestamp += time::Duration::seconds(10);
            r
        }));
        let backups = home.path().join("backups");
        let out = replace_to(SESSION, &base, &branch, None, &backups).unwrap();
        assert_eq!(out.appended, ["u3", "a3"]);
        assert_eq!(out.tip_source_id.as_deref(), Some("a3"));
        assert_eq!(read_back(&path), ["u1", "a1", "u3", "a3"]);
        assert_eq!(kinds(&path), [
            "user",
            "turn_start",
            "assistant",
            "turn_end",
            "user",
            "turn_start",
            "assistant",
            "turn_end"
        ]);
        assert_eq!(std::fs::read(&out.backup).unwrap(), before);
        assert!(out.warning.is_none());
        // Sharing nothing: refused.
        let again = read_tip(&path).unwrap();
        assert!(matches!(
            replace_to(SESSION, &again, &rows(&["u9"], None), None, &backups),
            Err(SyncError::Unsupported(_))
        ));
    }

    /// A session directory whose log last changed an hour ago, with `header` beside it, written
    /// just now.
    fn session(header: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join(LOG_FILE);
        std::fs::write(&log, "{}\n").unwrap();
        age(&log);
        if let Some(header) = header {
            std::fs::write(dir.path().join(HEADER_FILE), header).unwrap();
        }
        dir
    }

    /// Make `path` an hour old.
    fn age(path: &Path) {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(old).unwrap();
    }

    fn with_status(status: &str) -> String {
        HEADER.replace(r#""status": "idle""#, &format!(r#""status": "{status}""#))
    }

    /// No Kiro runs.
    fn nobody() -> Processes {
        table(&[])
    }

    /// A Kiro chat with its engine, run from `cwd` with `args`; beside them the shell
    /// integration and the desktop helper, which run all day and are no chat.
    fn chat(cwd: Option<&Path>, args: &[&str]) -> Processes {
        let mut command = vec!["/Applications/Kiro CLI.app/Contents/MacOS/kiro-cli-chat", "chat"];
        command.extend(args);
        let engine = "/Users/u/Library/Application \
                      Support/kiro-cli/kas/2.36.0-abc/node_modules/@kiro/agent/dist/server/\
                      acp-server.js";
        table(&[
            TableEntry {
                cwd: cwd.map(Path::to_path_buf),
                ..entry(10, "kiro-cli-chat", &command, 1)
            },
            TableEntry {
                cwd: cwd.map(Path::to_path_buf),
                ..entry(11, "node", &["node", "--experimental-wasm-modules", engine], 1)
            },
            entry(12, "zsh", &["zsh (kiro-cli-term)"], 1),
            entry(
                13,
                "kiro_cli_desktop",
                &["/Applications/Kiro CLI.app/Contents/MacOS/kiro_cli_desktop"],
                1,
            ),
        ])
    }

    /// The header was just rewritten with the status, as a turn's progress rewrites it.
    #[rstest]
    #[case::running("in_progress", Liveness::Live { pid: None })]
    #[case::asking("waiting_on_user", Liveness::Live { pid: None })]
    #[case::idle("idle", Liveness::NotLive)]
    #[case::over("completed", Liveness::NotLive)]
    #[case::failed("failed", Liveness::NotLive)]
    #[case::unknown_status("paused", Liveness::Unknown)]
    fn the_headers_status_says_whether_a_turn_runs(#[case] status: &str, #[case] want: Liveness) {
        assert!(HEADER.contains(r#""status": "idle""#));
        let dir = session(Some(&with_status(status)));
        assert_eq!(liveness(dir.path(), PROBE, None, &nobody()), want);
    }

    /// A Kiro that died mid-turn leaves `in_progress` behind: with nothing written for a while,
    /// it says only that someone may have the session.
    #[rstest]
    #[case::running("in_progress")]
    #[case::asking("waiting_on_user")]
    fn a_stale_running_turn_may_or_may_not_have_a_writer(#[case] status: &str) {
        let dir = session(Some(&with_status(status)));
        age(&dir.path().join(HEADER_FILE));
        assert_eq!(liveness(dir.path(), PROBE, None, &nobody()), Liveness::Unknown);
        // The log still changing says the turn goes on.
        std::fs::write(dir.path().join(LOG_FILE), "{}\n{}\n").unwrap();
        assert_eq!(liveness(dir.path(), PROBE, None, &nobody()), Liveness::Live { pid: None });
    }

    /// Kiro rewrites the header in place: one that doesn't parse is being written.
    #[rstest]
    fn a_half_written_header_is_a_writer() {
        let dir = session(Some(&HEADER[..HEADER.len() / 2]));
        assert_eq!(liveness(dir.path(), PROBE, None, &nobody()), Liveness::Unknown);
    }

    /// Between turns, the processes say: with no Kiro chat running, no one has the session,
    /// however new the log (a copy just restored); with one running, it may: one working in the
    /// session's directory or where the caller saw the session, one naming the session, or any
    /// while the log still changes. The shell integration and the desktop helper are no chat.
    #[rstest]
    fn between_turns_the_processes_say() {
        let dir = session(Some(HEADER));
        let works = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let probe_dir = "/tmp/kiro-play";
        let header = HEADER.replace(probe_dir, works.path().to_str().unwrap());
        assert_ne!(header, HEADER);
        std::fs::write(dir.path().join(HEADER_FILE), header).unwrap();
        let live = |cwd: Option<&Path>, procs: &Processes| liveness(dir.path(), PROBE, cwd, procs);
        let (here, there) = (Some(works.path()), Some(elsewhere.path()));

        assert_eq!(live(None, &nobody()), Liveness::NotLive);
        std::fs::write(dir.path().join(LOG_FILE), "{}\n{}\n").unwrap();
        assert_eq!(live(None, &nobody()), Liveness::NotLive, "a new log, and no Kiro");
        assert_eq!(live(None, &chat(there, &[])), Liveness::Unknown, "changing");
        age(&dir.path().join(LOG_FILE));
        age(&dir.path().join(HEADER_FILE));
        assert_eq!(live(None, &chat(there, &[])), Liveness::NotLive);
        assert_eq!(live(None, &chat(here, &[])), Liveness::Unknown, "where it works");
        assert_eq!(live(there, &chat(there, &[])), Liveness::Unknown, "where the caller saw it");
        assert_eq!(live(None, &chat(None, &[])), Liveness::Unknown, "working who knows where");
        let named = chat(there, &["--resume-id", PROBE]);
        assert_eq!(live(None, &named), Liveness::Unknown, "named");
        assert_eq!(live(None, &Processes::Unavailable), Liveness::Unknown);
        // With no header, a chat anywhere may have it.
        std::fs::remove_file(dir.path().join(HEADER_FILE)).unwrap();
        assert_eq!(live(None, &chat(there, &[])), Liveness::Unknown);
        assert_eq!(live(None, &nobody()), Liveness::NotLive);
    }
}
