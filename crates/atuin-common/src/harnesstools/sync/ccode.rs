//! Claude Code: the transcript `<config dir>/projects/<project>/<session>.jsonl`.
//!
//! **Tip.** `claude --resume` continues from the leaf its transcript loader picks (`G7t` in CC
//! 2.1.284), which this follows: the last main-chain line in file order, unless the last
//! `last-prompt` line names a leaf that line does not descend from, or names it explicitly
//! (`explicit: true`, as a rewind or a branch switch writes it, until another line follows);
//! then from that line up to the nearest user or assistant line, and on to the last (by
//! timestamp) of the attachments and system lines hanging from that one, which the loader puts
//! after it and the next line then hangs from. A `last-prompt` whose `leafUuid` is an explicit
//! `null` clears the session. A compaction boundary forgets the `last-prompt` before it, and
//! progress lines are skipped over as the loader skips them. Not followed: the relinking of a
//! compaction's preserved segment, and transcripts shared with a remote storage backend.
//!
//! **Append.** The rows are written by the rehydrate writer (the same line shapes, and calls
//! captured without their input flattened into notes the same way), each hanging from the line
//! its row was written after, the first from the tip, under the file's own session id. When the
//! loader would then not resume from the last of them, nothing is written
//! ([`SyncError::Unsupported`]): making it the tip would take a `last-prompt` marker, which is a
//! branch switch, not a fast-forward.
//!
//! **Liveness.** Claude Code registers each running session (interactive, `--print` and
//! background alike) in `<config dir>/sessions/<pid>.json`: `{pid, sessionId, cwd, startedAt,
//! procStart, pidDomain, kind, ...}` (`ND` in CC 2.1.284),<!-- codespell:ignore nd -->
//! rewriting `sessionId` when the process moves to another session, and removing the file on
//! exit. When the process started (`GRe`, `HWr` in CC 2.1.285) is:
//! - Linux: `procStart`, the `starttime` of `/proc/<pid>/stat` (clock ticks after boot);
//! - macOS and the other Unixes: `procStart`, what `LC_ALL=C TZ=UTC ps -o lstart= -p <pid>`
//!   prints (`Mon Oct  5 12:34:56 2026`), to the second;
//! - Windows: `procStartFt`, the creation time `GetProcessTimes` gives (a `FILETIME`: 100 ns
//!   ticks since 1601), and no `procStart`.
//!
//! Each is compared with the running process at what both sides have: Linux's ticks as they
//! are, the others as seconds since the epoch. One that can't be read or compared is not: the
//! process running under that pid is taken for the one registered.
//!
//! `pidDomain` is `linux:<machine id>:<pid namespace>` on Linux: a file from another pid
//! namespace (a container sharing the config directory) names a pid this machine cannot check.
//! Elsewhere it says only which platform wrote it (`cBo` in CC 2.1.285), so there only a Linux
//! namespace's domain is another machine's.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use time::PrimitiveDateTime;
use time::macros::format_description;

use super::liveness::Start;
use super::{
    AppendOptions, AppendOutcome, Links, Liveness, LocalTip, Processes, ReplaceOutcome,
    SessionSync, Stamp, SyncError, append_jsonl, blocking, branch_point, capture_key,
    check_segment, continuing_from, descends, jsonl_lines, kept_lines, modified, past,
    replace_jsonl, require_idle, resumes_from, switched_to, with_merged,
};
use crate::harnesstools::ccode::session::{CcodeMessage, default_root, locate, spawner_of};
use crate::harnesstools::ccode::{Ccode, rehydrate};
use crate::harnesstools::rehydrate::{RehydrateMessage, RehydrateSession};
use crate::harnesstools::session::{Message, SessionId};

impl SessionSync for Ccode {
    async fn local_tip(&self, id: &str) -> Result<Option<LocalTip>, SyncError> {
        let (root, id) = (default_root(), id.to_owned());
        blocking(move || locate(&root, &id).map(|path| read_tip(&path)).transpose()).await
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

/// `<config dir>/sessions`, beside the projects directory.
fn sessions_dir() -> PathBuf {
    default_root().with_file_name("sessions")
}

/// This process's pid domain, as Claude Code writes it on Linux (`SBo` in CC 2.1.284), else
/// the platform's name, which [`foreign`] only tells from Linux's.
fn pid_domain() -> String {
    if cfg!(target_os = "linux") {
        let machine = std::fs::read_to_string("/etc/machine-id").unwrap_or_default();
        let namespace = std::fs::read_link("/proc/self/ns/pid")
            .map(|ns| ns.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("linux:{}:{namespace}", machine.trim())
    } else {
        std::env::consts::OS.to_owned()
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
        let running = procs.running(pid);
        let Some(record) = record else {
            // Being rewritten, or not Claude Code's: whose it is cannot be told while it runs.
            unknown |= !matches!(running, Ok(None));
            continue;
        };
        if record["sessionId"].as_str() != Some(id) {
            continue;
        }
        if record["pidDomain"].as_str().is_some_and(|theirs| foreign(theirs, domain)) {
            unknown = true;
            continue;
        }
        let running = match running {
            Ok(Some(running)) => running,
            // Left behind by a process that crashed.
            Ok(None) => continue,
            Err(_) => {
                unknown = true;
                continue;
            }
        };
        if running.start.is_some_and(|start| same_start(&record, &start) == Some(false)) {
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

/// Whether a record's `pidDomain` is another machine's (or pid namespace's) than this one's
/// (`ours`): on Linux, any other; elsewhere, a Linux namespace's (see the module docs).
fn foreign(theirs: &str, ours: &str) -> bool {
    if ours.starts_with("linux:") {
        theirs != ours
    } else {
        theirs.split(':').count() >= 3
    }
}

/// Whether the process `record` registers started at `start`; `None` when that can't be told.
fn same_start(record: &Value, start: &Start) -> Option<bool> {
    match start {
        Start::BootTicks(ticks) => Some(record["procStart"].as_str()? == ticks),
        Start::Epoch(secs) => {
            let recorded = match record["procStartFt"].as_str() {
                Some(filetime) => filetime_secs(filetime)?,
                None => lstart_secs(record["procStart"].as_str()?)?,
            };
            Some(recorded == *secs)
        }
    }
}

/// A Windows `FILETIME` (100 ns ticks since 1601), as seconds since the Unix epoch.
fn filetime_secs(filetime: &str) -> Option<u64> {
    (filetime.parse::<u64>().ok()? / 10_000_000).checked_sub(11_644_473_600)
}

/// What `LC_ALL=C TZ=UTC ps -o lstart=` prints (`Mon Oct  5 12:34:56 2026`), as seconds since
/// the Unix epoch.
fn lstart_secs(lstart: &str) -> Option<u64> {
    let format = format_description!(
        "[weekday repr:short] [month repr:short] [day padding:none] [hour]:[minute]:[second] \
         [year]"
    );
    let words = lstart.split_whitespace().collect::<Vec<_>>().join(" ");
    let at = PrimitiveDateTime::parse(&words, format).ok()?;
    u64::try_from(at.assume_utc().unix_timestamp()).ok()
}

/// A line as Claude Code parses it, leniently (see `CcodeMessage::decode`).
fn parse(line: &[u8]) -> Option<Value> {
    let line = line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line);
    crate::json::js::from_slice(line).ok()
}

/// What the transcript at `path` holds.
pub(super) fn read_tip(path: &Path) -> Result<LocalTip, SyncError> {
    let bytes = std::fs::read(path)?;
    Ok(tip_of(path, &bytes))
}

/// What the transcript at `path`, holding `bytes`, holds: each line under the id capture gives
/// it, its own (`uuid`), else (titles, summaries: lines with no id) one keyed on its content, as
/// capture keys a session's lines under the session its file is named for.
fn tip_of(path: &Path, bytes: &[u8]) -> LocalTip {
    let session = SessionId::from(
        path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
    );
    // A nested subagent's lines name the subagent that spawned it, as capture reads them.
    let spawner = spawner_of(path);
    let mut occurrences = HashMap::new();
    let mut known = HashSet::new();
    let mut cwd = None;
    for line in jsonl_lines(bytes) {
        let Ok(message) = CcodeMessage::decode(line) else {
            continue;
        };
        let message = message.with_spawner(spawner.clone());
        if let Some(key) = capture_key(&session, &message, &mut occurrences) {
            known.insert(key);
        }
        cwd = message.cwd().or(cwd);
    }
    let (known, merged) = with_merged(known, bytes);
    LocalTip {
        native_path: path.to_path_buf(),
        known_source_ids: known,
        merged,
        tip_source_id: tip(bytes),
        stamp: Stamp::of(bytes),
        modified: modified(path),
        cwd,
        unswitchable: None,
    }
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
    taken: Option<&HashSet<String>>,
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
    let (out, appended) = extend(session_id, &current, base, lines, taken)?;
    let Some(last) = appended.last().cloned() else {
        // Nothing the transcript can carry: it holds the rows already, as far as it can.
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

/// The lines that append `lines` to the transcript of session `session_id` holding `current`
/// (read as `base`), and the source ids of the rows written as lines of their own (see
/// [`append_to`]); refused, as it is, unless they fast-forward it and Claude Code would resume
/// from the last of them.
fn extend(
    session_id: &str,
    current: &[u8],
    base: &LocalTip,
    lines: &[RehydrateMessage],
    taken: Option<&HashSet<String>>,
) -> Result<(Vec<Value>, Vec<String>), SyncError> {
    check_segment(lines, base, taken, Links::Parents)?;

    let cwd = working_dir(current).or_else(|| lines.iter().find_map(|m| m.cwd.clone()));
    let session = RehydrateSession {
        id: session_id.to_owned(),
        title: None,
        cwd: cwd.unwrap_or_default(),
        original_cwd: lines.iter().find_map(|m| m.cwd.clone()),
        git_branch: None,
        model: None,
        started_at: time::OffsetDateTime::now_utc(),
        messages: lines.to_vec(),
        fork_of: None,
    };
    let out = rehydrate::lines(&session, base.tip_source_id.as_deref());
    let appended: Vec<String> =
        out.iter().filter_map(|l| l["uuid"].as_str().map(str::to_owned)).collect();
    if let Some(last) = appended.last()
        && tip(&joined(current, &out)).as_ref() != Some(last)
    {
        return Err(SyncError::Unsupported(
            "Claude Code would not resume from the last row appended to this transcript",
        ));
    }
    Ok((out, appended))
}

/// The transcript holding `current`, with `lines` appended (as [`append_jsonl`] appends them).
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

/// The line kinds that name the session (`/rename`, a generated title, an agent's name): kept
/// wherever they are when the transcript is switched to another branch, so it keeps its title,
/// and capture finds them where it found them before.
const TITLE_LINES: [&str; 3] = ["custom-title", "ai-title", "agent-name"];

/// [`SessionSync::replace`] of the transcript `base` read, once the session is known to be idle.
///
/// The lines kept ([`shared_lines`]) are the transcript's but for those that went on from the
/// line `branch` leaves it at; the branch's rows past it are appended to them as
/// [`append_to`] appends rows, hanging from that line, under the file's own session id. Claude
/// Code then resumes from the last line, which must be the branch's head (or the line it was
/// merged into).
pub(super) fn replace_to(
    id: &str,
    base: &LocalTip,
    branch: &[RehydrateMessage],
    taken: Option<&HashSet<String>>,
    backups: &Path,
) -> Result<ReplaceOutcome, SyncError> {
    let path = &base.native_path;
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|stem| *stem == id)
        .ok_or_else(|| SyncError::Other(format!("{} is not session {id}", path.display())))?;
    if branch.is_empty() {
        return Err(SyncError::Unsupported("the branch has no rows"));
    }
    let current = std::fs::read(path)?;
    if Stamp::of(&current) != base.stamp {
        return Err(SyncError::Changed);
    }
    let (shared, from) = branch_point(base, branch, Links::Parents)?;
    let kept = shared_lines(&current, &from);
    let continuing = continuing_from(tip_of(path, &kept), &from)?;
    let rest = past(branch, shared, &continuing, Links::Parents);
    let (lines, appended) = extend(session_id, &kept, &continuing, &rest, taken)?;
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

/// The lines of the transcript `bytes` a switch to a branch that leaves it at line `from` keeps:
/// all but those that went on from `from` here. Those are the lines descending from it (by
/// `parentUuid`, or across a compaction boundary `logicalParentUuid`), and the lines that name
/// one of them (a `last-prompt`'s or a summary's `leafUuid`, a file history snapshot's
/// `messageId`); and any other line after the first of them, but for title lines
/// ([`TITLE_LINES`]), kept wherever they are.
fn shared_lines(bytes: &[u8], from: &str) -> Vec<u8> {
    let parsed: Vec<Option<Value>> = jsonl_lines(bytes).map(parse).collect();
    let mut parents: HashMap<&str, &str> = HashMap::new();
    for line in parsed.iter().flatten() {
        let parent = line["parentUuid"].as_str().or_else(|| line["logicalParentUuid"].as_str());
        if let (Some(uuid), Some(parent)) = (line["uuid"].as_str(), parent) {
            parents.entry(uuid).or_insert(parent);
        }
    }
    let mut memo = HashMap::new();
    let uuids: HashSet<&str> = parsed.iter().flatten().filter_map(|l| l["uuid"].as_str()).collect();
    let mut lines = parsed.iter();
    kept_lines(bytes, |_| {
        let line = lines.next().and_then(Option::as_ref)?;
        if TITLE_LINES.contains(&line["type"].as_str().unwrap_or_default()) {
            return Some(true);
        }
        if let Some(uuid) = line["uuid"].as_str() {
            return Some(!descends(&parents, &mut memo, uuid, from));
        }
        let named = line["leafUuid"].as_str().or_else(|| line["messageId"].as_str())?;
        uuids.contains(named).then(|| !descends(&parents, &mut memo, named, from))
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
