//! pi: the session file `<session dir>/<started>_<session>.jsonl`.
//!
//! **Tip.** pi continues from the last entry of the file, whatever it is (`_buildIndex` in
//! pi-coding-agent 0.85 `session-manager.ts`: the leaf is the last entry after the header), and
//! builds the context from it up through `parentId`.
//!
//! **Append.** The rows are written by the rehydrate writer, each hanging from the entry its row
//! was written after. The last of them is then the file's last entry, so pi resumes from it with
//! nothing more said. Making an entry the file already holds the tip (a branch this machine
//! already has) takes an entry after it: a `label` entry under it (`appendLabelChange`), which
//! keeps whatever label the entry had and adds nothing to the model's context. It is the one
//! thing written that capture has not seen: it is captured as an empty row of its own.
//!
//! **Liveness.** pi records no running session anywhere, and replaces its own command line with
//! its name, so a pi process never says which session it has open: one running in the session's
//! directory may be writing it.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use super::liveness::{pi_claim, scan};
use super::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, Processes, SessionSync, Stamp, SyncError,
    append_jsonl, blocking, check_segment, jsonl_lines, merged_in, require_idle,
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
        tokio::task::spawn_blocking(move || {
            let path = locate(&root, &id);
            let cwd = cwd.or_else(|| {
                let file = File::read(path.as_deref()?).ok()?;
                file.cwd.map(Into::into)
            });
            liveness(&Processes::here(), &id, cwd.as_deref(), path.as_deref())
        })
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
        let cwd = File::read(&base.native_path)?.cwd.map(std::path::PathBuf::from);
        require_idle(self.is_live(id, cwd.as_deref()).await)?;
        let (base, lines) = (base.clone(), lines.to_vec());
        let (make_tip, head) = (options.make_tip, options.head.map(str::to_owned));
        let taken = options.taken_ids.cloned();
        blocking(move || {
            let options = AppendOptions {
                make_tip,
                head: head.as_deref(),
                taken_ids: taken.as_ref(),
            };
            append_to(&base, &lines, &options)
        })
        .await
    }
}

/// Whether a pi process may be writing session `id`, of the file `path`, working in `cwd`.
pub(super) fn liveness(
    procs: &Processes,
    id: &str,
    cwd: Option<&Path>,
    path: Option<&Path>,
) -> Liveness {
    match procs.list() {
        Ok(processes) => scan(&processes, cwd, |p| pi_claim(p, id, path)),
        Err(_) => Liveness::Unknown,
    }
}

/// A session file, as pi and capture read it.
pub(super) struct File {
    bytes: Vec<u8>,
    /// The header's `id`, `version` (1 when it has none) and `cwd`.
    session: Option<String>,
    version: u64,
    cwd: Option<String>,
    /// Every entry after the header, as pi indexes it: `(id, parentId, type)`.
    entries: Vec<(String, Option<String>, String)>,
    /// The label each entry has (pi's `labelsById`).
    labels: HashMap<String, String>,
    /// The id capture gives each line.
    known: HashSet<String>,
}

impl File {
    pub(super) fn read(path: &Path) -> Result<Self, SyncError> {
        let bytes = std::fs::read(path)?;
        let mut file = Self {
            bytes: Vec::new(),
            session: None,
            version: 1,
            cwd: None,
            entries: Vec::new(),
            labels: HashMap::new(),
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
            if kind == "label"
                && let Some(target) = entry["targetId"].as_str()
            {
                match entry["label"].as_str().filter(|l| !l.is_empty()) {
                    Some(label) => file.labels.insert(target.to_owned(), label.to_owned()),
                    None => file.labels.remove(target),
                };
            }
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
        let merged = merged_in(&self.bytes, &self.known);
        let mut known = self.known.clone();
        known.extend(merged.keys().cloned());
        LocalTip {
            native_path: path.to_path_buf(),
            tip_source_id: self.leaf().map(str::to_owned),
            stamp: Stamp::of(&self.bytes),
            known_source_ids: known,
            merged,
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

/// An id pi would mint (8 hex digits) that none of `taken` holds.
fn fresh_id(taken: &dyn Fn(&str) -> bool) -> String {
    loop {
        let id = crate::utils::uuid_v7().as_simple().to_string();
        // The tail of a v7 uuid is random; its head is the time.
        let id = id[id.len() - 8..].to_owned();
        if !taken(&id) {
            return id;
        }
    }
}

/// [`SessionSync::append`] to the file `base` read, once the session is known to be idle.
pub(super) fn append_to(
    base: &LocalTip,
    lines: &[RehydrateMessage],
    options: &AppendOptions<'_>,
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
        // pi gives a version 1 file's entries fresh ids when it next opens it.
        return Err(SyncError::Unsupported(
            "an old pi session file: open it in pi once before it can be caught up",
        ));
    }
    let present = &base.known_source_ids;
    let in_file: HashSet<&str> = file.entries.iter().map(|(id, ..)| id.as_str()).collect();
    let taken = |id: &str| {
        in_file.contains(id)
            || present.contains(id)
            || options.taken_ids.is_some_and(|taken| taken.contains(id))
    };
    if let Some(clash) = lines.iter().find(|row| in_file.contains(row.source_id.as_str())) {
        return Err(SyncError::IdTaken(clash.source_id.clone()));
    }
    check_segment(lines, present, options.taken_ids, file.entries.is_empty())?;
    let held = base.held();

    let first_parent = lines.first().and_then(|row| row.parent_source_id.as_deref());
    let first_parent = first_parent.map(|p| held.line_of(p).unwrap_or(p)).filter(|p| !p.is_empty());
    let kept_from = file.kept_from(first_parent);
    let session = RehydrateSession {
        id: session_id,
        title: None,
        cwd: file.cwd.clone().map(Into::into).unwrap_or_default(),
        original_cwd: None,
        git_branch: None,
        model: None,
        started_at: time::OffsetDateTime::now_utc(),
        messages: lines.to_vec(),
    };
    let mut out = rehydrate::lines(
        &session,
        &Continuing {
            present: Some(held),
            after: None,
            kept_from: kept_from.as_deref(),
        },
        false,
    );
    let appended: Vec<String> =
        out.iter().filter_map(|e| e["id"].as_str().map(str::to_owned)).collect();
    let head = match (appended.last(), options.head.map(|h| held.line_of(h).unwrap_or(h))) {
        (Some(last), _) => Some(last.clone()),
        (None, Some(head)) if in_file.contains(head) => Some(head.to_owned()),
        (None, Some(head)) => return Err(SyncError::Disconnected(head.to_owned())),
        (None, None) => None,
    };
    let last_entry = file.entries.last().map(|(id, ..)| id.as_str());
    // Only an entry that is not the file's last already needs one after it.
    let label =
        head.filter(|head| appended.is_empty() && last_entry != Some(head.as_str())).map(|head| {
            let mut label = json!({
                "type": "label",
                "id": fresh_id(&taken),
                "parentId": head,
                "timestamp": rehydrate::timestamp(time::OffsetDateTime::now_utc()),
                "targetId": head,
            });
            if let Some(existing) = file.labels.get(&head) {
                label["label"] = json!(existing);
            }
            label
        });
    let tip = match &label {
        Some(label) => label["id"].as_str().map(str::to_owned),
        None => appended.last().cloned().or_else(|| file.leaf().map(str::to_owned)),
    };
    let marked = label.is_some();
    out.extend(label);
    if !out.is_empty() {
        append_jsonl(path, base.stamp, &out)?;
    }
    Ok(AppendOutcome {
        native_path: path.clone(),
        appended,
        tip_source_id: tip,
        marked_tip: marked,
    })
}

#[cfg(test)]
mod tests;
