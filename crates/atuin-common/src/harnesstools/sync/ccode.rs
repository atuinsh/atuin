//! Claude Code: the transcript `<config dir>/projects/<project>/<session>.jsonl`.
//!
//! **Tip.** `claude --resume` continues from the leaf its transcript loader picks (`G7t` in CC
//! 2.1.284), which this follows: the last main-chain line in file order, unless the last
//! `last-prompt` line names a leaf that line does not descend from, or names it explicitly
//! (`explicit: true`, as a rewind or a branch switch writes it, until another line follows);
//! then from that line up to the nearest user or assistant line, and on to the last (by
//! timestamp) of the attachments and system lines hanging from that one, which the loader puts
//! after it and the next line then hangs from. A `last-prompt` whose
//! `leafUuid` is an explicit `null` clears the session. A compaction boundary forgets the
//! `last-prompt` before it, and progress lines are skipped over as the loader skips them. Not
//! followed: the relinking of a compaction's preserved segment, and transcripts shared with a
//! remote storage backend.
//!
//! **Append.** The rows are written by the rehydrate writer (the same line shapes, and calls
//! captured without their input flattened into notes the same way), each hanging from the line
//! its row was written after, under the file's own session id. When the loader would not pick
//! the last of them by itself (the segment branches off an older line) or the caller asks, an
//! explicit `last-prompt` naming it follows: `{"type": "last-prompt", "leafUuid": <uuid>,
//! "explicit": true, "sessionId": <id>}`, the line Claude Code writes when the user picks a
//! branch (`ist` in CC 2.1.284). Capture reads nothing of it (no uuid, no content).
//!
//! **Liveness.** Claude Code registers each running session (interactive, `--print` and
//! background alike) in `<config dir>/sessions/<pid>.json`: `{pid, sessionId, cwd, startedAt,
//! procStart, pidDomain, kind, ...}` (`ND` in CC 2.1.284), rewriting `sessionId` when the
//! process moves to another session, and removing the file on exit. `procStart` is the
//! `starttime` of `/proc/<pid>/stat` on Linux, and `pidDomain` `linux:<machine id>:<pid
//! namespace>` there (`linux` elsewhere): a file from another pid namespace (a container sharing
//! the config directory) names a pid this machine cannot check.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    AppendOptions, AppendOutcome, Liveness, LocalTip, Processes, SessionSync, Stamp, SyncError,
    append_jsonl, blocking, check_segment, jsonl_lines, merged_in, require_idle,
};
use crate::harnesstools::ccode::session::{CcodeMessage, default_root, locate};
use crate::harnesstools::ccode::{Ccode, rehydrate};
use crate::harnesstools::rehydrate::{RehydrateMessage, RehydrateSession};
use crate::harnesstools::session::Message;

impl SessionSync for Ccode {
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError> {
        let (root, id) = (default_root(), id.to_owned());
        blocking(move || match locate(&root, &id) {
            Some(path) => read_tip(&path).map(Some),
            None => Ok(None),
        })
        .await
    }

    async fn is_live(&self, id: &str, _cwd: Option<&Path>) -> Liveness {
        let (sessions, id) = (sessions_dir(), id.to_owned());
        tokio::task::spawn_blocking(move || {
            liveness(&sessions, &id, &Processes::here(), &pid_domain())
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
        require_idle(self.is_live(id, None).await)?;
        let (id, base, lines) = (id.to_owned(), base.clone(), lines.to_vec());
        let (make_tip, head) = (options.make_tip, options.head.map(str::to_owned));
        let taken = options.taken_ids.cloned();
        blocking(move || {
            let options = AppendOptions {
                make_tip,
                head: head.as_deref(),
                taken_ids: taken.as_ref(),
            };
            append_to(&id, &base, &lines, &options)
        })
        .await
    }
}

/// `<config dir>/sessions`, beside the projects directory.
fn sessions_dir() -> PathBuf {
    default_root().with_file_name("sessions")
}

/// This process's pid domain, as Claude Code writes it (`SBo` in CC 2.1.284).
fn pid_domain() -> String {
    if cfg!(target_os = "linux") {
        let machine = std::fs::read_to_string("/etc/machine-id").unwrap_or_default();
        let namespace = std::fs::read_link("/proc/self/ns/pid")
            .map(|ns| ns.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("linux:{}:{namespace}", machine.trim())
    } else {
        "linux".to_owned()
    }
}

/// Whether a Claude Code process registered in `sessions` has session `id` open.
pub(super) fn liveness(sessions: &Path, id: &str, procs: &Processes, domain: &str) -> Liveness {
    let entries = match std::fs::read_dir(sessions) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Liveness::NotLive,
        Err(_) => return Liveness::Unknown,
    };
    let mut unknown = false;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(pid) =
            name.to_str().and_then(|n| n.strip_suffix(".json")).and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let record: Option<Value> =
            std::fs::read(entry.path()).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let Some(record) = record else {
            // Being rewritten, or not Claude Code's: whose it is cannot be told while it runs.
            unknown |= procs.running(pid).is_some();
            continue;
        };
        if record["sessionId"].as_str() != Some(id) {
            continue;
        }
        if record["pidDomain"].as_str().is_some_and(|theirs| theirs != domain) {
            unknown = true;
            continue;
        }
        let Some(running) = procs.running(pid) else {
            // Left behind by a process that crashed.
            continue;
        };
        let recorded = record["procStart"].as_str();
        if let (Some(recorded), Some(start)) = (recorded, running.start.as_deref())
            && recorded != start
        {
            // The pid has been reused since.
            continue;
        }
        return Liveness::Live { pid: Some(pid) };
    }
    if unknown {
        Liveness::Unknown
    } else {
        Liveness::NotLive
    }
}

/// A line as Claude Code parses it, leniently (see `CcodeMessage::decode`).
fn parse(line: &[u8]) -> Option<Value> {
    let line = line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line);
    crate::json::js::from_slice(line).ok()
}

/// What the transcript at `path` holds.
pub(super) fn read_tip(path: &Path) -> Result<LocalTip, SyncError> {
    let bytes = std::fs::read(path)?;
    let mut known = HashSet::new();
    for line in jsonl_lines(&bytes) {
        if let Some(id) = CcodeMessage::decode(line).ok().and_then(|m| m.id()) {
            known.insert(String::from(id));
        }
    }
    let merged = merged_in(&bytes, &known);
    known.extend(merged.keys().cloned());
    Ok(LocalTip {
        native_path: path.to_path_buf(),
        known_source_ids: known,
        merged,
        tip_source_id: tip(&bytes),
        stamp: Stamp::of(&bytes),
    })
}

/// The line `claude --resume` continues the transcript `bytes` from (see the module docs).
pub(super) fn tip(bytes: &[u8]) -> Option<String> {
    let mut tree = Tree::default();
    for line in jsonl_lines(bytes) {
        if let Some(value) = parse(line) {
            tree.push(&value);
        }
    }
    tree.tip()
}

struct Node {
    parent: Option<String>,
    conversation: bool,
    sidechain: bool,
    timestamp: Option<String>,
}

/// The transcript tree as Claude Code's loader builds it, and what it tracks to pick a leaf.
#[derive(Default)]
struct Tree {
    nodes: HashMap<String, Node>,
    order: Vec<String>,
    /// Progress lines, each standing for its own parent (the loader skips over them).
    progress: HashMap<String, Option<String>>,
    /// The last main-chain line (`In`).
    last: Option<String>,
    /// The main-chain line with the latest timestamp, and that timestamp (`Vn`, `Jn`).
    newest: Option<(String, String)>,
    /// The leaf the last `last-prompt` named (`Kn`), and whether it did so explicitly with no
    /// line after it (`un`).
    leaf: Option<String>,
    explicit: bool,
    /// Whether any `last-prompt` named a leaf, even `null` (`fn`).
    leaf_named: bool,
    /// The session was cleared (`Xt`).
    cleared: bool,
}

impl Tree {
    fn push(&mut self, line: &Value) {
        let kind = line["type"].as_str().unwrap_or_default();
        let uuid = line["uuid"].as_str();
        match (kind, uuid) {
            ("progress", Some(uuid)) => {
                let parent = line["parentUuid"].as_str().map(str::to_owned);
                let parent = match &parent {
                    Some(p) if self.progress.contains_key(p) => self.progress[p].clone(),
                    _ => parent,
                };
                self.progress.insert(uuid.to_owned(), parent);
            }
            ("user" | "assistant" | "system" | "attachment", Some(uuid)) => {
                if self.nodes.contains_key(uuid) {
                    return;
                }
                let parent = line["parentUuid"].as_str().and_then(|p| match self.progress.get(p) {
                    Some(skipped) => skipped.clone(),
                    None => Some(p.to_owned()),
                });
                let sidechain = line["isSidechain"] == true;
                let briefing =
                    kind == "attachment" && line["attachment"]["type"] == "fork_briefing";
                let timestamp = line["timestamp"].as_str().map(str::to_owned);
                if !sidechain && !briefing {
                    self.last = Some(uuid.to_owned());
                    if let Some(ts) = &timestamp
                        && self.newest.as_ref().is_none_or(|(_, newest)| ts > newest)
                    {
                        self.newest = Some((uuid.to_owned(), ts.clone()));
                    }
                    self.explicit = false;
                    self.cleared = false;
                }
                if kind == "system" && line["subtype"] == "compact_boundary" {
                    self.leaf = None;
                    self.explicit = false;
                }
                self.order.push(uuid.to_owned());
                self.nodes.insert(uuid.to_owned(), Node {
                    parent,
                    conversation: matches!(kind, "user" | "assistant"),
                    sidechain,
                    timestamp,
                });
            }
            ("last-prompt", _) => {
                let Some(leaf) = line.get("leafUuid") else {
                    return;
                };
                self.leaf_named = true;
                let explicit = line["explicit"] == true;
                match leaf.as_str().filter(|l| !l.is_empty()) {
                    Some(leaf) => {
                        self.explicit =
                            explicit || (self.explicit && self.leaf.as_deref() == Some(leaf));
                        self.leaf = Some(leaf.to_owned());
                        self.cleared = false;
                    }
                    None if leaf.is_null() && explicit => {
                        self.cleared = true;
                        self.leaf = None;
                        self.explicit = false;
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    /// Whether `from`, or a line it descends from, is `ancestor`.
    fn descends(&self, from: &str, ancestor: &str) -> bool {
        let mut seen = HashSet::new();
        let mut at = Some(from);
        while let Some(id) = at {
            if id == ancestor {
                return true;
            }
            if !seen.insert(id) {
                return false;
            }
            at = self.nodes.get(id).and_then(|n| n.parent.as_deref());
        }
        false
    }

    /// The main-chain line descending from `from` with the latest timestamp, else `from`.
    fn newest_below(&self, from: &str) -> String {
        let mut best = from.to_owned();
        let mut best_ts = String::new();
        let mut memo: HashMap<&str, bool> = HashMap::from([(from, true)]);
        for id in &self.order {
            let node = &self.nodes[id];
            let Some(ts) = &node.timestamp else {
                continue;
            };
            if node.sidechain || id == from || *ts < best_ts {
                continue;
            }
            // Walk up until a line whose answer is known, then record the answer on the way.
            let mut path = Vec::new();
            let mut at = Some(id.as_str());
            let found = loop {
                let Some(step) = at else {
                    break false;
                };
                if let Some(known) = memo.get(step) {
                    break *known;
                }
                memo.insert(step, false);
                path.push(step);
                at = self.nodes.get(step).and_then(|n| n.parent.as_deref());
            };
            for step in path {
                memo.insert(step, found);
            }
            if found {
                best.clone_from(id);
                best_ts.clone_from(ts);
            }
        }
        best
    }

    fn tip(&self) -> Option<String> {
        if self.cleared {
            return None;
        }
        let fallback = match (&self.last, &self.newest) {
            (Some(last), Some((newest, _))) if !self.leaf_named && newest != last => {
                Some(self.newest_below(last))
            }
            _ => self.last.clone(),
        };
        let mut from = self.leaf.clone().filter(|leaf| self.nodes.contains_key(leaf));
        if let (Some(leaf), Some(last)) = (&from, &self.last)
            && !self.explicit
            && last != leaf
            && self.nodes.contains_key(last)
            && self.descends(last, leaf)
        {
            from = Some(last.clone());
        }
        let mut at = from.or(fallback)?;
        let mut seen = HashSet::new();
        let leaf = loop {
            let node = self.nodes.get(&at)?;
            if node.conversation {
                break at;
            }
            if !seen.insert(at.clone()) {
                return None;
            }
            at = node.parent.clone()?;
        };
        Some(self.trailing(&leaf))
    }

    /// The line the conversation ending at `leaf` is continued from: the loader puts the lines
    /// hanging from it that are not the user's or the model's (attachments, system lines, and
    /// theirs in turn) after it, in timestamp order (`Nar` in CC 2.1.284), and the next line
    /// hangs from the last of them.
    fn trailing(&self, leaf: &str) -> String {
        let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
        for id in &self.order {
            let node = &self.nodes[id];
            if let Some(parent) = node.parent.as_deref()
                && !node.conversation
            {
                children.entry(parent).or_default().push(id);
            }
        }
        let mut found: Vec<&str> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::from([leaf]);
        let mut queue = std::collections::VecDeque::from([leaf]);
        while let Some(at) = queue.pop_front() {
            for child in children.get(at).into_iter().flatten() {
                if seen.insert(child) {
                    found.push(child);
                    queue.push_back(child);
                }
            }
        }
        let stamp = |id: &str| self.nodes[id].timestamp.clone().unwrap_or_default();
        // Stable, as the loader's sort is.
        found.sort_by_key(|id| stamp(id));
        found.last().copied().unwrap_or(leaf).to_owned()
    }
}

/// [`SessionSync::append`] to the transcript `base` read, once the session is known to be idle.
pub(super) fn append_to(
    id: &str,
    base: &LocalTip,
    lines: &[RehydrateMessage],
    options: &AppendOptions<'_>,
) -> Result<AppendOutcome, SyncError> {
    let path = &base.native_path;
    // The transcript's lines name the session its file is named after.
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|stem| *stem == id)
        .ok_or_else(|| SyncError::Other(format!("{} is not session {id}", path.display())))?;
    let current = std::fs::read(path)?;
    if Stamp::of(&current) != base.stamp {
        return Err(SyncError::Changed);
    }
    let present = &base.known_source_ids;
    check_segment(lines, present, options.taken_ids, present.is_empty())?;
    let held = base.held();

    let cwd = working_dir(&current).or_else(|| lines.iter().find_map(|m| m.cwd.clone()));
    let session = RehydrateSession {
        id: session_id.to_owned(),
        title: None,
        cwd: cwd.unwrap_or_default(),
        original_cwd: lines.iter().find_map(|m| m.cwd.clone()),
        git_branch: None,
        model: None,
        started_at: time::OffsetDateTime::now_utc(),
        messages: lines.to_vec(),
    };
    let mut out = rehydrate::lines(&session, Some(held), None);
    let appended: Vec<String> =
        out.iter().filter_map(|l| l["uuid"].as_str().map(str::to_owned)).collect();
    let head = match (appended.last(), options.head.map(|h| held.line_of(h).ok_or(h))) {
        (Some(last), _) => Some(last.clone()),
        (None, Some(Ok(head))) => Some(head.to_owned()),
        (None, Some(Err(head))) => return Err(SyncError::Disconnected(head.to_owned())),
        (None, None) => None,
    };

    let with = |out: &[Value]| {
        let mut bytes = current.clone();
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        for line in out {
            bytes.extend(line.to_string().bytes());
            bytes.push(b'\n');
        }
        bytes
    };
    let marked = head
        .as_ref()
        .is_some_and(|head| options.make_tip || tip(&with(&out)).as_ref() != Some(head));
    if marked {
        out.push(json!({
            "type": "last-prompt",
            "leafUuid": head,
            "explicit": true,
            "sessionId": session_id,
        }));
    }
    let tip_source_id = tip(&with(&out));
    if !out.is_empty() {
        append_jsonl(path, base.stamp, &out)?;
    }
    Ok(AppendOutcome {
        native_path: path.clone(),
        appended,
        tip_source_id,
        marked_tip: marked,
    })
}

/// The directory the transcript last worked in.
fn working_dir(bytes: &[u8]) -> Option<PathBuf> {
    jsonl_lines(bytes)
        .rev()
        .filter_map(parse)
        .find_map(|line| line["cwd"].as_str().map(PathBuf::from))
}

#[cfg(test)]
mod tests;
