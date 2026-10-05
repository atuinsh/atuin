//! pi: the session file `<session dir>/<started>_<session>.jsonl`.
//!
//! **Tip.** pi continues from the last entry of the file, whatever it is (`_buildIndex` in
//! pi-coding-agent 0.85 `session-manager.ts`: the leaf is the last entry after the header), and
//! builds the context from it up through `parentId`.
//!
//! **Append.** The rows are written by the rehydrate writer, each hanging from the entry its row
//! was written after, the first from the tip. The last of them is then the file's last entry, so
//! pi resumes from it with nothing more said. A compaction among them keeps from the first entry
//! since the last compaction on the tip's path, as pi's own would. A version 1 file is refused:
//! pi gives its entries new ids when it next opens it.
//!
//! **Liveness.** pi records no running session anywhere, and replaces its own command line with
//! its name, so a pi process seldom says which session it has open: any pi running here in the
//! session's directory (or below it, or where that can't be told) may be writing it; so may one
//! anywhere whose command line can be read and names the session (`pi --session <file>`), and
//! while the session file was modified in the last [`RECENT`](super::RECENT), any pi at all.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::liveness::{agent_running, changed_lately};
use super::{
    AppendOptions, AppendOutcome, Dirs, Links, Liveness, LocalTip, Processes, Seen, SessionSync,
    Stamp, SyncError, append_jsonl, blocking, blocking_liveness, check_segment, jsonl_lines,
    modified, require_idle, with_merged,
};
use crate::harnesstools::pi::Pi;
use crate::harnesstools::pi::rehydrate::{self, Continuing};
use crate::harnesstools::pi::session::{PiMessage, default_root, locate};
use crate::harnesstools::rehydrate::{RehydrateMessage, RehydrateSession};
use crate::harnesstools::session::Message;

impl SessionSync for Pi {
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError> {
        let (root, id) = (default_root(), id.to_owned());
        blocking(move || match locate(&root, &id) {
            Some(path) => Ok(Some(File::read(&path)?.tip(&path))),
            None => Ok(None),
        })
        .await
    }

    async fn is_live(&self, id: &str, cwd: Option<&Path>) -> Liveness {
        let (root, id, cwd) = (default_root(), id.to_owned(), cwd.map(Path::to_path_buf));
        blocking_liveness(move || {
            let file = locate(&root, &id);
            let seen = Seen {
                dirs: Dirs::of(cwd),
                changing: file.as_deref().is_some_and(|f| changed_lately(modified(f))),
                names: names(&id, file.as_deref()),
            };
            liveness(&Processes::here(), &seen)
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
        let seen = appending(base, id, options.cwd);
        require_idle(blocking_liveness(move || liveness(&Processes::here(), &seen)).await)?;
        let (base, lines) = (base.clone(), lines.to_vec());
        let taken = options.taken_ids.cloned();
        blocking(move || append_to(&base, &lines, taken.as_ref())).await
    }
}

/// Whether a pi process may be writing the session `seen`: [`Liveness::Unknown`] while any that
/// may be runs ([`agent_running`]).
pub(super) fn liveness(procs: &Processes, seen: &Seen) -> Liveness {
    agent_running(procs, "pi", &["pi-coding-agent"], seen)
}

/// Session `id`, as [`SessionSync::append`] to the file `base` read takes it, last seen working
/// in `cwd`.
fn appending(base: &LocalTip, id: &str, cwd: Option<&Path>) -> Seen {
    base.seen(cwd, names(id, Some(&base.native_path)))
}

/// What a pi command line opening session `id`, kept in `file`, would name: its id, and its
/// file, whole or by name.
fn names(id: &str, file: Option<&Path>) -> Vec<String> {
    let file = file.into_iter().flat_map(|f| [Some(f.as_os_str()), f.file_name()]).flatten();
    std::iter::once(id.to_owned()).chain(file.map(|f| f.to_string_lossy().into_owned())).collect()
}

/// A session file, as pi and capture read it.
pub(super) struct File {
    bytes: Vec<u8>,
    /// When the file was last modified, read before its content.
    modified: Option<std::time::SystemTime>,
    /// The header's `id`, `version` (1 when it has none) and `cwd`.
    session: Option<String>,
    version: u64,
    cwd: Option<String>,
    /// Every entry after the header, as pi indexes it: `(id, parentId, type)`.
    entries: Vec<(String, Option<String>, String)>,
    /// The id capture gives each line.
    known: HashSet<String>,
}

impl File {
    pub(super) fn read(path: &Path) -> Result<Self, SyncError> {
        let modified = modified(path);
        let bytes = std::fs::read(path)?;
        let mut file = Self {
            bytes: Vec::new(),
            modified,
            session: None,
            version: 1,
            cwd: None,
            entries: Vec::new(),
            known: HashSet::new(),
        };
        for line in jsonl_lines(&bytes) {
            // pi skips a line it cannot parse, and capture reports it and reads on.
            let Ok(entry) = crate::json::js::from_slice::<Value>(line) else {
                continue;
            };
            if let Some(id) =
                crate::json::js::from_slice::<PiMessage>(line).ok().and_then(|m| m.id())
            {
                file.known.insert(String::from(id));
            }
            let kind = entry["type"].as_str().unwrap_or_default();
            if kind == "session" {
                if file.session.is_none() {
                    file.session = entry["id"].as_str().map(str::to_owned);
                    file.version = entry["version"].as_u64().unwrap_or(1);
                    file.cwd = entry["cwd"].as_str().map(str::to_owned);
                }
                continue;
            }
            let Some(id) = entry["id"].as_str() else {
                continue;
            };
            file.entries.push((
                id.to_owned(),
                entry["parentId"].as_str().map(str::to_owned),
                kind.to_owned(),
            ));
        }
        file.bytes = bytes;
        Ok(file)
    }

    /// The entry pi continues from, as capture keys it: the last one.
    fn leaf(&self) -> Option<&str> {
        self.entries.last().map(|(id, ..)| id.as_str()).filter(|id| self.known.contains(*id))
    }

    pub(super) fn tip(self, path: &Path) -> LocalTip {
        let tip_source_id = self.leaf().map(str::to_owned);
        let (known, merged) = with_merged(self.known, &self.bytes);
        LocalTip {
            native_path: path.to_path_buf(),
            tip_source_id,
            stamp: Stamp::of(&self.bytes),
            modified: self.modified,
            known_source_ids: known,
            merged,
            cwd: self.cwd.map(PathBuf::from),
        }
    }

    /// The first entry since the last compaction on the path from the root to `leaf`: what a
    /// compaction appended under `leaf` keeps from.
    fn kept_from(&self, leaf: Option<&str>) -> Option<String> {
        let parents: HashMap<&str, (Option<&str>, &str)> = self
            .entries
            .iter()
            .map(|(id, parent, kind)| (id.as_str(), (parent.as_deref(), kind.as_str())))
            .collect();
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        let mut at = leaf;
        while let Some(id) = at {
            let Some((parent, kind)) = parents.get(id) else {
                break;
            };
            if !seen.insert(id) {
                break;
            }
            path.push((id, *kind));
            at = *parent;
        }
        path.reverse();
        let start = path.iter().rposition(|(_, kind)| *kind == "compaction").map_or(0, |n| n + 1);
        path.get(start).map(|(id, _)| (*id).to_owned())
    }
}

/// [`SessionSync::append`] to the file `base` read, once the session is known to be idle.
pub(super) fn append_to(
    base: &LocalTip,
    lines: &[RehydrateMessage],
    taken: Option<&HashSet<String>>,
) -> Result<AppendOutcome, SyncError> {
    let path = &base.native_path;
    let file = File::read(path)?;
    if Stamp::of(&file.bytes) != base.stamp {
        return Err(SyncError::Changed);
    }
    let Some(session_id) = file.session.clone() else {
        return Err(SyncError::Other(format!("{} has no session header", path.display())));
    };
    if file.version < 2 {
        return Err(SyncError::Unsupported(
            "an old pi session file: open it in pi once before it can be caught up",
        ));
    }
    // An entry without a row of its own (one capture keys on its content) is still an id taken.
    if let Some(clash) =
        lines.iter().find(|row| file.entries.iter().any(|(id, ..)| *id == row.source_id))
    {
        return Err(SyncError::IdTaken(clash.source_id.clone()));
    }
    check_segment(lines, base, taken, Links::Parents)?;
    let tip = base.tip_source_id.as_deref();
    let kept_from = file.kept_from(tip);
    let session = RehydrateSession {
        id: session_id,
        title: None,
        cwd: file.cwd.map(Into::into).unwrap_or_default(),
        original_cwd: None,
        git_branch: None,
        model: None,
        started_at: time::OffsetDateTime::now_utc(),
        messages: lines.to_vec(),
        fork_of: None,
    };
    let continuing = Continuing {
        after: tip,
        kept_from: kept_from.as_deref(),
    };
    let out = rehydrate::lines(&session, &continuing, false);
    let appended: Vec<String> =
        out.iter().filter_map(|e| e["id"].as_str().map(str::to_owned)).collect();
    if !out.is_empty() {
        append_jsonl(path, base.stamp, &out)?;
    }
    Ok(AppendOutcome {
        native_path: path.clone(),
        tip_source_id: appended.last().cloned().or_else(|| base.tip_source_id.clone()),
        appended,
    })
}

#[cfg(test)]
mod tests;
