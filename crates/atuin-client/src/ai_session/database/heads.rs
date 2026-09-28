//! A session's branches: its heads, where they part, and whether hosts differ.
//!
//! A session keeps one id across machines. Each machine's own transcript is a branch of it, and
//! the synced rows hold every branch at once: a session resumed on a second host from a stale
//! copy continues from an earlier row than the first host's last, and one rewound (or
//! interrupted and continued) on a single host holds a dead end. Resuming needs to know which
//! (see the design's git model): one head is fast-forwarded to; heads from different hosts are
//! branches never to be merged.
//!
//! Every row of a session is a node of one tree (a forest, really), built per harness from what
//! its rows say:
//!
//! - **Claude Code and Pi** link each line to the one before it (`parent_source_id`). Rows keyed
//!   on their content (`syn-`: titles and other lines with no id) are no node; a session holding
//!   nothing else is linear. A row whose parent is not stored (a line capture keeps no row of, or
//!   one not synced yet) follows on from the row before it.
//! - **Codex** numbers its rollout lines (`Message::seq`). A host continuing a rollout numbers its
//!   new lines on from the last it holds, so two hosts continuing the same one repeat each other's
//!   numbers: rows are strung into chains in number order, each continuing the chain it follows
//!   on from (its own host's first), and a number a chain already holds starts a new one off the
//!   rows below it. Rows with no number (captured before numbers were) come first, in time order.
//! - **opencode** is linear, and its parent pointers name a message where its rows are parts
//!   (see [`resolve_opencode_parent`]). In time order, a host whose rows come back after another
//!   host's rows have come between is interleaving: the session parts at the end of the first
//!   host's run, after which each host's rows follow on from its own.
//! - Anything else is linear, in time order.
//!
//! Time order is by timestamp, then source id; nothing depends on the order rows arrived in.
//!
//! Most forks are nothing: parallel tool calls, attachments and compaction branch the tree too.
//! A fork counts only when at least two of the subtrees below it hold a *substantive* row, a user
//! prompt or assistant text; the rest of it stays with the branch it was on. Each branch that
//! reaches no fork that counts is a head, whose tip is the newest leaf on it. The session is
//! *diverged* when the substantive sides of a fork that counts were started by different hosts;
//! heads of one host alone (a rewind, an interrupt) are not a divergence, and neither is one
//! host carrying on a line another left.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use atuin_common::db::{self};
use atuin_common::harnesstools::session::{Content, Role};
use atuin_domain::record::HostId;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::SqliteConnection;
use tracing::warn;

use super::{AiSessionDatabase, DbError};
use crate::ai_session::{HarnessKind, HarnessSession, Head, Message, SessionHeads, SourceId};

/// Rows whose `substantive` is filled in at a time, when a migration left them to decode.
const BACKFILL_CHUNK: i64 = 512;

/// The prefix of a source id capture made up from a line's content (see the daemon's
/// `message_enricher`).
const SYNTHETIC: &str = "syn-";

/// Whether a row with `role` and `content` makes a branch worth telling apart: a user prompt, or
/// assistant text.
pub(super) fn is_substantive(role: &Role, content: &[Content]) -> bool {
    match role {
        Role::User => true,
        Role::Assistant => {
            content.iter().any(|c| matches!(c, Content::Text(t) if !t.trim().is_empty()))
        }
        _ => false,
    }
}

/// One row of a session, as far as its branches go.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Node {
    source_id: String,
    parent: Option<String>,
    host: Option<String>,
    /// Timestamp, in milliseconds.
    at: i64,
    seq: Option<i64>,
    substantive: bool,
    /// Whether the row is an assistant's: never what an opencode message pointer resolves to.
    assistant: bool,
    /// The session's own header row (a Pi `session` line), never a branch's tip when anything
    /// else is.
    header: bool,
}

/// How a harness's rows string together (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Model {
    Tree,
    Ordinals,
    Interleaving,
    Linear,
}

impl Model {
    const fn of(harness: HarnessKind) -> Self {
        match harness {
            HarnessKind::ClaudeCode | HarnessKind::Pi => Self::Tree,
            HarnessKind::Codex => Self::Ordinals,
            HarnessKind::Opencode => Self::Interleaving,
            HarnessKind::Copilot | HarnessKind::Unknown => Self::Linear,
        }
    }
}

/// A session's branches worked out over its rows, by index into [`Analysis::nodes`] (which are in
/// time order).
#[derive(Debug)]
struct Analysis {
    nodes: Vec<Node>,
    /// Where each node sits in the input.
    input_index: Vec<usize>,
    /// Each node's parent in the branch tree; `None` for a root, and for a row that is no node.
    parent: Vec<Option<usize>>,
    in_tree: Vec<bool>,
    /// The stored row each node's parent pointer resolves to.
    resolved: Vec<Option<usize>>,
    depth: Vec<usize>,
    /// Tips, newest first.
    heads: Vec<usize>,
    branch_point: Option<usize>,
    diverged: bool,
}

impl Analysis {
    /// Work out the branches of a session of `harness` holding `nodes`, in any order.
    fn new(harness: HarnessKind, nodes: Vec<Node>) -> Self {
        let mut order: Vec<usize> = (0..nodes.len()).collect();
        order.sort_by(|&a, &b| {
            (nodes[a].at, &nodes[a].source_id).cmp(&(nodes[b].at, &nodes[b].source_id))
        });
        let input_index = order.clone();
        let mut slots: Vec<Option<Node>> = nodes.into_iter().map(Some).collect();
        let nodes: Vec<Node> = order.iter().map(|&i| slots[i].take().expect("each once")).collect();

        let resolved = resolve(harness, &nodes);
        let mut model = Model::of(harness);
        let mut in_tree = vec![true; nodes.len()];
        if model == Model::Tree {
            for (i, node) in nodes.iter().enumerate() {
                in_tree[i] = !node.source_id.starts_with(SYNTHETIC);
            }
            // Nothing but content-addressed rows (a Pi session from before ids): linear.
            if !in_tree.contains(&true) {
                model = Model::Linear;
                in_tree.fill(true);
            }
        }
        let mut parent = match model {
            Model::Tree => {
                let mut previous = None;
                let mut parent = vec![None; nodes.len()];
                for i in (0..nodes.len()).filter(|&i| in_tree[i]) {
                    parent[i] = match resolved[i].filter(|&p| in_tree[p]) {
                        Some(p) => Some(p),
                        // A parent that is not stored is a line capture kept no row of (or one
                        // not synced yet): the row follows on from the one before it.
                        None if nodes[i].parent.is_some() => previous,
                        None => None,
                    };
                    previous = Some(i);
                }
                parent
            }
            Model::Ordinals => ordinal_parents(&nodes),
            Model::Interleaving => interleaved_parents(&nodes),
            Model::Linear => (0..nodes.len()).map(|i| i.checked_sub(1)).collect(),
        };
        break_cycles(&mut parent, &in_tree);

        let mut analysis = Self {
            nodes,
            input_index,
            parent,
            in_tree,
            resolved,
            depth: Vec::new(),
            heads: Vec::new(),
            branch_point: None,
            diverged: false,
        };
        analysis.find_heads();
        analysis
    }

    fn find_heads(&mut self) {
        let n = self.nodes.len();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut roots = Vec::new();
        for i in (0..n).filter(|&i| self.in_tree[i]) {
            match self.parent[i] {
                Some(p) => children[p].push(i),
                None => roots.push(i),
            }
        }

        // Parents before children, so depths fill in going down and substance going up.
        let mut preorder = Vec::with_capacity(n);
        let mut stack: Vec<usize> = roots.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            preorder.push(i);
            stack.extend(children[i].iter().rev());
        }
        let mut depth = vec![0; n];
        for &i in &preorder {
            if let Some(p) = self.parent[i] {
                depth[i] = depth[p] + 1;
            }
        }
        let mut substance = vec![false; n];
        for &i in preorder.iter().rev() {
            substance[i] |= self.nodes[i].substantive;
            if let Some(p) = self.parent[i] {
                substance[p] |= substance[i];
            }
        }

        // Hand out branches going down: a fork that counts closes the branch reaching it and opens
        // one per substantive side; everything else stays on the branch it is on.
        let mut branch = vec![0_usize; n];
        let mut split = vec![false];
        let mut diverged = false;
        let mut fork = |kids: &[usize], on: usize, branch: &mut Vec<usize>| {
            let sides: Vec<usize> = kids.iter().copied().filter(|&k| substance[k]).collect();
            if sides.len() < 2 {
                for &k in kids {
                    branch[k] = on;
                }
                return;
            }
            split[on] = true;
            let hosts: HashSet<&str> =
                sides.iter().filter_map(|&k| self.nodes[k].host.as_deref()).collect();
            diverged |= hosts.len() > 1;
            for &k in kids {
                branch[k] = on;
            }
            for &k in &sides {
                branch[k] = split.len();
                split.push(false);
            }
        };
        fork(&roots, 0, &mut branch);
        for &i in &preorder {
            fork(&children[i], branch[i], &mut branch);
        }

        // Each branch no fork closed ends in the newest of its leaves, a header only if nothing
        // else.
        let mut tips: HashMap<usize, usize> = HashMap::new();
        for &i in &preorder {
            if !children[i].is_empty() || split[branch[i]] {
                continue;
            }
            let better = tips
                .get(&branch[i])
                .is_none_or(|&t| (!self.nodes[i].header, i) > (!self.nodes[t].header, t));
            if better {
                tips.insert(branch[i], i);
            }
        }
        let mut heads: Vec<usize> = tips.into_values().collect();
        heads.sort_by_key(|&i| Reverse(i));

        self.branch_point = match heads.as_slice() {
            [] | [_] => None,
            [first, rest @ ..] => {
                rest.iter().try_fold(*first, |a, &b| common_ancestor(&self.parent, &depth, a, b))
            }
        };
        self.depth = depth;
        self.heads = heads;
        self.diverged = diverged;
    }

    /// The heads, newest first, with the last row they share and whether hosts differ.
    fn session_heads(&self) -> SessionHeads {
        let heads = self
            .heads
            .iter()
            .map(|&tip| {
                let node = &self.nodes[tip];
                let rows = match self.branch_point {
                    Some(bp) => self.depth[tip] - self.depth[bp],
                    None => self.depth[tip] + 1,
                };
                Head {
                    source_id: SourceId::from(node.source_id.clone()),
                    host: host_from(node.host.as_deref()),
                    last_at: AiSessionDatabase::time_from_millis(node.at)
                        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH),
                    rows: rows as u64,
                }
            })
            .collect();
        SessionHeads {
            heads,
            branch_point: self
                .branch_point
                .map(|bp| SourceId::from(self.nodes[bp].source_id.clone())),
            diverged: self.diverged,
        }
    }

    /// The node with source id `id`.
    fn find(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.source_id == id)
    }

    /// The nodes from the root down to `tip`, following the branch tree; empty for a row that is
    /// no node.
    fn path_to(&self, tip: &str) -> Vec<usize> {
        let Some(tip) = self.find(tip).filter(|&t| self.in_tree[t]) else {
            return Vec::new();
        };
        let mut path = vec![tip];
        let mut at = tip;
        while let Some(p) = self.parent[at] {
            path.push(p);
            at = p;
        }
        path.reverse();
        path
    }
}

/// The last row `a` and `b` both descend from, or `None` in different trees.
fn common_ancestor(
    parent: &[Option<usize>],
    depth: &[usize],
    mut a: usize,
    mut b: usize,
) -> Option<usize> {
    while depth[a] > depth[b] {
        a = parent[a]?;
    }
    while depth[b] > depth[a] {
        b = parent[b]?;
    }
    while a != b {
        a = parent[a]?;
        b = parent[b]?;
    }
    Some(a)
}

/// Cut each parent cycle (a corrupt transcript's) at its earliest row, which becomes a root.
fn break_cycles(parent: &mut [Option<usize>], in_tree: &[bool]) {
    // 0: unseen, 1: on the walk in progress, 2: known to reach a root.
    let mut state = vec![0_u8; parent.len()];
    for start in (0..parent.len()).filter(|&i| in_tree[i]) {
        let mut walk = Vec::new();
        let mut at = Some(start);
        while let Some(i) = at {
            match state[i] {
                2 => break,
                1 => {
                    // Back at a row of this walk: everything from it on is the cycle.
                    let from = walk.iter().position(|&w| w == i).expect("on the walk");
                    let earliest = *walk[from..].iter().min().expect("non-empty");
                    parent[earliest] = None;
                    break;
                }
                _ => {
                    state[i] = 1;
                    walk.push(i);
                    at = parent[i];
                }
            }
        }
        for i in walk {
            state[i] = 2;
        }
    }
}

/// The stored row each node's parent pointer names: the row under that id, else for opencode,
/// the message's first part (see [`resolve_opencode_parent`]).
fn resolve(harness: HarnessKind, nodes: &[Node]) -> Vec<Option<usize>> {
    let by_id: HashMap<&str, usize> =
        nodes.iter().enumerate().map(|(i, n)| (n.source_id.as_str(), i)).collect();
    let mut parts: Vec<(&str, usize)> = Vec::new();
    if harness == HarnessKind::Opencode {
        parts = nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| Some((ascending_payload(&n.source_id, "prt_")?, i)))
            .collect();
        parts.sort_unstable();
    }
    nodes
        .iter()
        .map(|node| {
            let parent = node.parent.as_deref()?;
            by_id.get(parent).copied().or_else(|| {
                resolve_opencode_parent(parent, &parts).filter(|&i| !nodes[i].assistant)
            })
        })
        .collect()
}

/// The first part of opencode message `message`, among a session's `parts` (each part's
/// [`ascending_payload`] and index, in payload order).
///
/// opencode's rows are the parts of its messages, each under its own id (`prt_...`), where an
/// assistant message's `parentID` names the user message it answers (`msg_...`), which is no
/// row: capture keeps no message ids, and records are immutable, so the pointer is resolved here.
/// opencode mints both kinds of id ascending (`Identifier.ascending`: the creation time in
/// milliseconds times 4096 plus a counter, as 12 hex digits after the prefix, then random), and
/// creates a message's parts right after the message itself, so its first part is the first part
/// minted after it.
fn resolve_opencode_parent(message: &str, parts: &[(&str, usize)]) -> Option<usize> {
    let payload = ascending_payload(message, "msg_")?;
    let at = parts.partition_point(|(p, _)| *p <= payload);
    parts.get(at).map(|&(_, i)| i)
}

/// The time-ordered part of an opencode ascending id with `prefix` (its 12 hex digits).
fn ascending_payload<'a>(id: &'a str, prefix: &str) -> Option<&'a str> {
    let payload = id.strip_prefix(prefix)?.get(..12)?;
    payload.bytes().all(|b| b.is_ascii_hexdigit()).then_some(payload)
}

/// Codex: rows without a number in time order, then numbered ones in chains (see the module
/// docs).
fn ordinal_parents(nodes: &[Node]) -> Vec<Option<usize>> {
    struct Chain {
        last: usize,
        last_seq: i64,
    }

    let mut parent = vec![None; nodes.len()];
    let mut prefix_end = None;
    let mut numbered = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        match node.seq {
            Some(seq) => numbered.push((seq, i)),
            None => {
                parent[i] = prefix_end;
                prefix_end = Some(i);
            }
        }
    }
    numbered.sort_unstable();

    let mut chains: Vec<Chain> = Vec::new();
    for (seq, i) in numbered {
        let host = &nodes[i].host;
        // The chain it follows on from: its own host's, else the one reaching furthest (then the
        // most recent, then the oldest chain).
        let rank = |(k, c): &(usize, &Chain)| {
            (&nodes[c.last].host == host, c.last_seq, c.last, Reverse(*k))
        };
        let next = chains.iter().enumerate().filter(|(_, c)| c.last_seq < seq).max_by_key(rank);
        if let Some((k, _)) = next {
            parent[i] = Some(chains[k].last);
            chains[k] = Chain {
                last: i,
                last_seq: seq,
            };
            continue;
        }
        // Every chain holds this number already: start one off the rows below it, on this host's
        // chain if it has one, else the one reaching furthest.
        parent[i] = chains.iter().enumerate().max_by_key(rank).map_or(prefix_end, |(_, base)| {
            let mut at = Some(base.last);
            while let Some(a) = at {
                if nodes[a].seq.is_none_or(|s| s < seq) {
                    break;
                }
                at = parent[a];
            }
            at
        });
        chains.push(Chain {
            last: i,
            last_seq: seq,
        });
    }
    parent
}

/// opencode: in time order, linear unless hosts interleave (see the module docs).
fn interleaved_parents(nodes: &[Node]) -> Vec<Option<usize>> {
    let linear: Vec<Option<usize>> = (0..nodes.len()).map(|i| i.checked_sub(1)).collect();
    // Hosts in the order their substantive rows come, one entry per run.
    let mut runs: Vec<&Option<String>> = Vec::new();
    for node in nodes.iter().filter(|n| n.substantive) {
        if runs.last() != Some(&&node.host) {
            runs.push(&node.host);
        }
    }
    let seen: HashSet<&Option<String>> = runs.iter().copied().collect();
    if seen.len() == runs.len() {
        return linear;
    }

    // The first row of the second host's first substantive run ends the shared prefix.
    let second = runs[1];
    let start = nodes
        .iter()
        .position(|n| n.substantive && &n.host == second)
        .expect("the second run has a substantive row");
    let mut parent = linear;
    let prefix_end = start.checked_sub(1);
    let mut last: HashMap<&Option<String>, usize> = HashMap::new();
    for (i, node) in nodes.iter().enumerate().skip(start) {
        parent[i] = last.get(&node.host).copied().or(prefix_end);
        last.insert(&node.host, i);
    }
    parent
}

fn host_from(host: Option<&str>) -> Option<HostId> {
    host.and_then(|h| uuid::Uuid::parse_str(h).ok()).map(HostId)
}

/// [`Head`] as `sessions.heads` stores it.
#[derive(Serialize, Deserialize)]
struct StoredHead {
    source_id: String,
    host: Option<String>,
    /// Milliseconds.
    last_at: i64,
    rows: u64,
}

/// `sessions.heads` read back; a malformed value reads as none.
pub(super) fn decode(json: &str) -> Vec<Head> {
    let stored: Vec<StoredHead> = serde_json::from_str(json).unwrap_or_default();
    stored
        .into_iter()
        .map(|h| Head {
            source_id: SourceId::from(h.source_id),
            host: host_from(h.host.as_deref()),
            last_at: AiSessionDatabase::time_from_millis(h.last_at)
                .unwrap_or(time::OffsetDateTime::UNIX_EPOCH),
            rows: h.rows,
        })
        .collect()
}

fn encode(heads: &[Head]) -> Result<String, DbError> {
    let stored: Vec<StoredHead> = heads
        .iter()
        .map(|h| StoredHead {
            source_id: h.source_id.to_string(),
            host: h.host.map(AiSessionDatabase::host_repr),
            last_at: AiSessionDatabase::millis(h.last_at),
            rows: h.rows,
        })
        .collect();
    Ok(serde_json::to_string(&stored)?)
}

/// The `messages` columns a branch computation reads.
#[derive(sqlx::FromRow)]
struct BranchRow {
    source_id: String,
    parent_source_id: Option<String>,
    parent_row: Option<String>,
    host_id: Option<String>,
    timestamp: i64,
    seq: Option<i64>,
    role: String,
    substantive: Option<i64>,
}

const ASSISTANT: &str = "\"Assistant\"";
const USER: &str = "\"User\"";
const HEADER: &str = "{\"Other\":\"session\"}";

/// While alive, appends only mark the heads they move, for [`AiSessionDatabase::refresh_heads`]
/// to compute once: see [`AiSessionDatabase::defer_heads`].
#[derive(Debug)]
#[must_use = "heads are deferred only while this lives"]
pub struct HeadsDeferral(Arc<AtomicUsize>);

impl Drop for HeadsDeferral {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl AiSessionDatabase {
    /// Have appends only mark the sessions whose heads they move while the returned guard lives,
    /// for a bulk replay (a reprojection, an import) that would otherwise work out a session's
    /// heads again with each of its rows. Call [`Self::refresh_heads`] once it is dropped. Marks
    /// persist, so heads a crash leaves stale are computed when the sidecar is next opened.
    pub fn defer_heads(&self) -> HeadsDeferral {
        self.heads_deferred.fetch_add(1, Ordering::SeqCst);
        HeadsDeferral(self.heads_deferred.clone())
    }

    /// A session's rows changed: work its heads out again, or mark them to be while deferred.
    pub(super) async fn heads_changed(
        &self,
        conn: &mut SqliteConnection,
        harness: i64,
        session_id: &str,
    ) -> Result<(), DbError> {
        if self.heads_deferred.load(Ordering::SeqCst) > 0 {
            db::query("UPDATE sessions SET heads_dirty = 1 WHERE harness = ? AND session_id = ?")
                .bind(harness)
                .bind(session_id)
                .execute(conn)
                .await?;
            return Ok(());
        }
        Self::recompute_heads(conn, harness, session_id).await
    }

    /// Work out the heads of every session marked for it (see [`Self::defer_heads`]), each in a
    /// transaction of its own. Returns how many.
    pub async fn refresh_heads(&self) -> Result<u64, DbError> {
        let mut done = 0;
        loop {
            let dirty: Vec<(i64, String)> = db::query_as(
                "SELECT harness, session_id FROM sessions WHERE heads_dirty = 1 LIMIT 256",
            )
            .fetch_all(self.db.pool())
            .await?;
            if dirty.is_empty() {
                return Ok(done);
            }
            for (harness, session_id) in dirty {
                let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
                Self::recompute_heads(&mut tx, harness, &session_id).await?;
                tx.commit().await?;
                done += 1;
            }
        }
    }

    /// Work out every session's heads from scratch, as a migration or a rebuild does.
    pub async fn rebuild_heads(&self) -> Result<u64, DbError> {
        db::query("UPDATE sessions SET heads_dirty = 1").execute(self.db.pool()).await?;
        self.refresh_heads().await
    }

    /// The rows of a session, as far as its branches go.
    async fn branch_rows(
        conn: &mut SqliteConnection,
        harness: i64,
        session_id: &str,
    ) -> Result<Vec<BranchRow>, DbError> {
        Ok(db::query_as(
            "SELECT source_id, parent_source_id, parent_row, host_id, timestamp, seq, role, \
             substantive FROM messages WHERE harness = ? AND session_id = ?",
        )
        .bind(harness)
        .bind(session_id)
        .fetch_all(conn)
        .await?)
    }

    fn analyse(harness: i64, rows: &[BranchRow]) -> Result<Analysis, DbError> {
        let nodes = rows
            .iter()
            .map(|r| Node {
                source_id: r.source_id.clone(),
                parent: r.parent_source_id.clone(),
                host: r.host_id.clone(),
                at: r.timestamp,
                seq: r.seq,
                substantive: r.substantive.map_or(r.role == USER, |s| s != 0),
                assistant: r.role == ASSISTANT,
                header: r.role == HEADER,
            })
            .collect();
        Ok(Analysis::new(Self::harness_from_repr(harness)?, nodes))
    }

    /// Work a session's heads out from its rows and store them, with each row's resolved
    /// parent.
    async fn recompute_heads(
        conn: &mut SqliteConnection,
        harness: i64,
        session_id: &str,
    ) -> Result<(), DbError> {
        let rows = Self::branch_rows(&mut *conn, harness, session_id).await?;
        let analysis = Self::analyse(harness, &rows)?;
        for (at, resolved) in analysis.resolved.iter().enumerate() {
            let row = &rows[analysis.input_index[at]];
            let resolved = resolved.map(|p| analysis.nodes[p].source_id.as_str());
            if row.parent_row.as_deref() != resolved {
                db::query(
                    "UPDATE messages SET parent_row = ? WHERE harness = ? AND session_id = ? AND \
                     source_id = ?",
                )
                .bind(resolved)
                .bind(harness)
                .bind(session_id)
                .bind(&row.source_id)
                .execute(&mut *conn)
                .await?;
            }
        }

        let heads = analysis.session_heads();
        db::query(
            "UPDATE sessions SET heads = ?, branch_point = ?, diverged = ?, heads_dirty = 0 WHERE \
             harness = ? AND session_id = ?",
        )
        .bind(encode(&heads.heads)?)
        .bind(heads.branch_point.as_ref().map(|s| s.as_ref()))
        .bind(i64::from(heads.diverged))
        .bind(harness)
        .bind(session_id)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// The branches of `session`: its heads (the tips of its branches, newest first), the last
    /// row they share, and whether they were captured on different hosts. `None` for a session
    /// not stored.
    ///
    /// Heads are stored as rows arrive (or once a bulk replay is done, see
    /// [`Self::defer_heads`]); a session whose stored heads are out of date is worked out afresh
    /// without storing, so this reads a read-only database right too.
    pub async fn heads(&self, session: &HarnessSession) -> Result<Option<SessionHeads>, DbError> {
        let harness = session.harness as i64;
        let stored: Option<(Option<String>, Option<String>, i64, i64)> = db::query_as(
            "SELECT heads, branch_point, diverged, heads_dirty FROM sessions WHERE harness = ? \
             AND session_id = ?",
        )
        .bind(harness)
        .bind(session.session.as_ref())
        .fetch_optional(self.db.pool())
        .await?;
        let Some((heads, branch_point, diverged, dirty)) = stored else {
            return Ok(None);
        };
        if dirty == 0 {
            return Ok(Some(SessionHeads {
                heads: heads.as_deref().map(decode).unwrap_or_default(),
                branch_point: branch_point.map(SourceId::from),
                diverged: diverged != 0,
            }));
        }
        let mut conn = self.db.pool().acquire().await?;
        let rows = Self::branch_rows(&mut conn, harness, session.session.as_ref()).await?;
        Ok(Some(Self::analyse(harness, &rows)?.session_heads()))
    }

    /// The rows from the root of `session` down to `head` (one of its [heads](Self::heads), or
    /// any row on a branch), in order, following the branch tree: what a writer writes out for a
    /// transcript on that branch.
    ///
    /// Only rows that are nodes of the tree are on it (see the module docs): for Claude Code and
    /// Pi, the rows with an id of their own, so not titles or other content-addressed rows. Empty
    /// when `head` is not stored, or is no node.
    pub async fn branch_path(
        &self,
        session: &HarnessSession,
        head: &SourceId,
    ) -> Result<Vec<Message>, DbError> {
        let messages: Vec<Message> = self.messages(session).try_collect().await?;
        let nodes = messages
            .iter()
            .map(|m| Node {
                source_id: m.source_id.to_string(),
                parent: m.parent_source_id.as_ref().map(ToString::to_string),
                host: m.host.map(Self::host_repr),
                at: Self::millis(m.timestamp),
                seq: m.seq.map(|n| i64::try_from(n).unwrap_or(i64::MAX)),
                substantive: is_substantive(&m.role, &m.content),
                assistant: m.role == Role::Assistant,
                header: m.role == Role::Other("session".to_owned()),
            })
            .collect();
        let analysis = Analysis::new(session.harness, nodes);
        let mut slots: Vec<Option<Message>> = messages.into_iter().map(Some).collect();
        Ok(analysis
            .path_to(head.as_ref())
            .into_iter()
            .filter_map(|i| slots[analysis.input_index[i]].take())
            .collect())
    }

    /// The rows on `head`'s [branch path](Self::branch_path) after the last of them in `known`
    /// (the source ids a writer's local transcript holds), in order: what fast-forwarding that
    /// transcript to `head` appends. Every row on the path when it holds none of them; none when
    /// it holds the head.
    ///
    /// It says nothing of rows the transcript holds that are not on the path: a writer checks
    /// that its own tip is on it before appending, or it is on another branch.
    pub async fn missing_after(
        &self,
        session: &HarnessSession,
        head: &SourceId,
        known: &HashSet<SourceId>,
    ) -> Result<Vec<Message>, DbError> {
        let mut path = self.branch_path(session, head).await?;
        let shared = path.iter().rposition(|m| known.contains(&m.source_id));
        Ok(match shared {
            Some(at) => path.split_off(at + 1),
            None => path,
        })
    }

    /// Fill in `substantive` for rows a migration could not, their content being compressed.
    pub(super) async fn backfill_substantive(&self) -> Result<(), DbError> {
        loop {
            let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
            let rows: Vec<(i64, String, String, Option<Vec<u8>>)> = db::query_as(
                "SELECT rowid, role, content, content_z FROM messages WHERE substantive IS NULL \
                 LIMIT ?",
            )
            .bind(BACKFILL_CHUNK)
            .fetch_all(&mut *tx)
            .await?;
            if rows.is_empty() {
                return Ok(());
            }
            for (rowid, role, content, content_z) in rows {
                let role: Role = serde_json::from_str(&role).unwrap_or(Role::Other(role));
                let content = Self::read_content(content, content_z).unwrap_or_else(|err| {
                    warn!(?err, rowid, "failed to decode ai-session message; not substantive");
                    Vec::new()
                });
                db::query("UPDATE messages SET substantive = ? WHERE rowid = ?")
                    .bind(i64::from(is_substantive(&role, &content)))
                    .bind(rowid)
                    .execute(&mut *tx)
                    .await?;
            }
            // Their sessions' heads are still marked to compute: migration 0006 marked them all.
            tx.commit().await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use atuin_common::harnesstools::session::{Content, Role};
    use atuin_domain::record::{HostId, RecordId};
    use futures::TryStreamExt;
    use proptest::prelude::*;
    use rstest::rstest;

    use super::{Analysis, Node};
    use crate::ai_session::{
        AiSessionDatabase, HarnessKind, HarnessSession, Message, NativeSessionId, SessionHeads,
        SourceId,
    };

    fn host(n: u8) -> String {
        uuid::Uuid::from_u128(u128::from(n)).as_hyphenated().to_string()
    }

    /// What a row is, as far as branches go.
    #[derive(Clone, Copy, Debug)]
    enum Kind {
        /// A user prompt.
        Prompt,
        /// Assistant text.
        Reply,
        /// A tool call or its result: substantive neither.
        Tool,
    }

    fn node(id: &str, parent: Option<&str>, on: u8, at: i64, kind: Kind) -> Node {
        Node {
            source_id: id.to_owned(),
            parent: parent.map(str::to_owned),
            host: Some(host(on)),
            at,
            seq: None,
            substantive: !matches!(kind, Kind::Tool),
            assistant: !matches!(kind, Kind::Prompt),
            header: false,
        }
    }

    /// A chain of `ids` under `parent`, alternating prompt and reply, from `at` on.
    fn chain(ids: &[&str], parent: Option<&str>, on: u8, at: i64) -> Vec<Node> {
        let mut parent = parent.map(str::to_owned);
        ids.iter()
            .zip(at..)
            .enumerate()
            .map(|(i, (id, at))| {
                let kind = if i % 2 == 0 {
                    Kind::Prompt
                } else {
                    Kind::Reply
                };
                let n = node(id, parent.as_deref(), on, at, kind);
                parent = Some((*id).to_owned());
                n
            })
            .collect()
    }

    fn heads_of(harness: HarnessKind, nodes: Vec<Node>) -> SessionHeads {
        Analysis::new(harness, nodes).session_heads()
    }

    /// (tip, rows) of each head, newest first.
    fn tips(heads: &SessionHeads) -> Vec<(String, u64)> {
        heads.heads.iter().map(|h| (h.source_id.to_string(), h.rows)).collect()
    }

    fn bp(heads: &SessionHeads) -> Option<String> {
        heads.branch_point.as_ref().map(ToString::to_string)
    }

    fn tip(id: &str, rows: u64) -> (String, u64) {
        (id.to_owned(), rows)
    }

    #[rstest]
    fn a_linear_session_has_one_head_holding_every_row() {
        let heads = heads_of(HarnessKind::ClaudeCode, chain(&["a", "b", "c", "d"], None, 1, 0));
        assert_eq!(tips(&heads), [tip("d", 4)]);
        assert_eq!(bp(&heads), None);
        assert!(!heads.diverged);
    }

    /// Parallel tool calls fork the tree without making a branch: the side holding only a tool
    /// call stays on the line, and the newest leaf is the tip.
    #[rstest]
    #[case::continued(
        &[("t2", "a1", 3, Kind::Tool), ("r1", "t1", 4, Kind::Tool), ("a2", "r1", 5, Kind::Reply)],
        "a2"
    )]
    #[case::ended_in_tool_calls(&[("t2", "a1", 3, Kind::Tool), ("r1", "t1", 4, Kind::Tool)], "r1")]
    fn tool_call_forks_are_no_branch(#[case] rest: &[(&str, &str, i64, Kind)], #[case] tip: &str) {
        let mut nodes = chain(&["u1", "a1"], None, 1, 0);
        nodes.push(node("t1", Some("a1"), 1, 2, Kind::Tool));
        nodes.extend(rest.iter().map(|&(id, p, at, kind)| node(id, Some(p), 1, at, kind)));
        let heads = heads_of(HarnessKind::ClaudeCode, nodes);
        assert_eq!(heads.heads.len(), 1, "{heads:?}");
        assert_eq!(heads.heads[0].source_id.as_ref(), tip);
        assert!(!heads.diverged);
    }

    /// A rewind on one host leaves two heads, newest first, parting at the row rewound to; one
    /// host's heads are no divergence.
    #[rstest]
    fn a_rewind_is_two_heads_of_one_host() {
        let mut nodes = chain(&["u1", "a1", "u2", "a2"], None, 1, 0);
        nodes.extend(chain(&["u2b", "a2b", "u3b"], Some("a1"), 1, 10));
        let heads = heads_of(HarnessKind::ClaudeCode, nodes);
        assert_eq!(tips(&heads), [tip("u3b", 3), tip("a2", 2)]);
        assert_eq!(bp(&heads).as_deref(), Some("a1"));
        assert!(!heads.diverged);
        assert_eq!(heads.latest().unwrap().source_id.as_ref(), "u3b");
    }

    /// Two hosts continuing the same row are diverged; one host carrying on another's line (a
    /// fast-forward) is not, even past a rewind of the first host.
    #[rstest]
    #[case::two_hosts(1, 2, true)]
    #[case::one_host_then_another_carries_on(1, 1, false)]
    fn hosts_starting_the_sides_of_a_fork_decide_divergence(
        #[case] left: u8,
        #[case] right: u8,
        #[case] diverged: bool,
    ) {
        let mut nodes = chain(&["u1", "a1"], None, 1, 0);
        nodes.extend(chain(&["u2", "a2"], Some("a1"), left, 10));
        nodes.extend(chain(&["v2", "b2"], Some("a1"), right, 20));
        // Host 2 carries the right side on.
        nodes.extend(chain(&["v3", "b3"], Some("b2"), 2, 30));
        let heads = heads_of(HarnessKind::ClaudeCode, nodes);
        assert_eq!(heads.diverged, diverged);
        assert_eq!(tips(&heads), [tip("b3", 4), tip("a2", 2)]);
        assert_eq!(heads.heads[0].host, Some(HostId(uuid::Uuid::from_u128(2))));
    }

    /// Titles and other content-addressed rows are no node; a Pi header is never a tip while
    /// anything else can be; a session of nothing but content-addressed rows is linear.
    #[rstest]
    fn rows_off_the_tree() {
        let mut nodes = chain(&["u1", "a1"], None, 1, 0);
        nodes.push(node("syn-00000000000000aa", None, 1, 5, Kind::Prompt));
        let mut header = node("hdr", None, 1, -1, Kind::Tool);
        header.header = true;
        nodes.push(header);
        let heads = heads_of(HarnessKind::Pi, nodes);
        assert_eq!(tips(&heads), [tip("a1", 2)]);

        let only = chain(&["syn-1", "syn-2"], None, 1, 0);
        let heads = heads_of(HarnessKind::Pi, only);
        assert_eq!(tips(&heads), [tip("syn-2", 2)]);
    }

    /// Codex rows (which have no parent pointers) string together by number: a host carrying on
    /// another's rollout continues its line, two hosts numbering the same lines part.
    #[rstest]
    fn codex_rows_string_together_by_number() {
        let row = |id: &str, seq: Option<i64>, on: u8, at: i64, kind: Kind| Node {
            seq,
            ..node(id, None, on, at, kind)
        };
        let mut nodes = vec![
            row("meta", None, 1, 0, Kind::Tool),
            row("p0", Some(1), 1, 1, Kind::Prompt),
            row("r0", Some(2), 1, 2, Kind::Reply),
        ];
        // Host 2 carries on from ordinal 2.
        nodes.extend([
            row("p1", Some(3), 2, 10, Kind::Prompt),
            row("r1", Some(4), 2, 11, Kind::Reply),
        ]);
        let heads = heads_of(HarnessKind::Codex, nodes.clone());
        assert_eq!(tips(&heads), [tip("r1", 5)]);
        assert!(!heads.diverged);

        // Host 1 carries on from ordinal 2 too, later.
        nodes.extend([
            row("q1", Some(3), 1, 20, Kind::Prompt),
            row("s1", Some(4), 1, 21, Kind::Reply),
            row("q2", Some(5), 1, 22, Kind::Prompt),
        ]);
        let heads = heads_of(HarnessKind::Codex, nodes.clone());
        assert!(heads.diverged);
        assert_eq!(bp(&heads).as_deref(), Some("r0"));
        assert_eq!(tips(&heads), [tip("q2", 3), tip("r1", 2)]);
        let analysis = Analysis::new(HarnessKind::Codex, nodes);
        let path: Vec<&str> = analysis
            .path_to("q2")
            .into_iter()
            .map(|i| analysis.nodes[i].source_id.as_str())
            .collect();
        assert_eq!(path, ["meta", "p0", "r0", "q1", "s1", "q2"]);
    }

    /// opencode is linear: one host carrying on from another continues it, a host coming back
    /// after another's rows is interleaving, parting at the end of the first host's run.
    #[rstest]
    #[case::handed_on(&[1, 1, 2, 2], false, None)]
    #[case::interleaved(&[1, 1, 2, 1, 2], true, Some("r1"))]
    fn opencode_parts_where_hosts_interleave(
        #[case] hosts: &[u8],
        #[case] diverged: bool,
        #[case] branch_point: Option<&str>,
    ) {
        let nodes: Vec<Node> = hosts
            .iter()
            .zip(0..)
            .map(|(&on, i)| node(&format!("r{i}"), None, on, i, Kind::Prompt))
            .collect();
        let heads = heads_of(HarnessKind::Opencode, nodes);
        assert_eq!(heads.diverged, diverged);
        assert_eq!(bp(&heads).as_deref(), branch_point);
        assert_eq!(
            heads.heads.len(),
            if diverged {
                2
            } else {
                1
            }
        );
    }

    const USER_PART: &str = "prt_fcf1e6fc6001aaaaaaaaaaaaaa";
    const USER_MESSAGE: &str = "msg_fcf1e6fc5001U1qt7HpZk2Rdwx";

    /// An opencode part's parent pointer names a message, which resolves to the message's first
    /// part: the first part minted after it. An assistant part is never what it resolves to.
    #[rstest]
    fn an_opencode_message_pointer_resolves_to_its_first_part() {
        let part = |id: &str, parent: Option<&str>, at, kind| node(id, parent, 1, at, kind);
        let nodes = vec![
            part(USER_PART, None, 0, Kind::Prompt),
            part("prt_fcf1e6fc7001bbbbbbbbbbbbbb", None, 1, Kind::Prompt),
            part("prt_fcf1e84ea001cccccccccccccc", Some(USER_MESSAGE), 2, Kind::Reply),
            part(
                "prt_fcf1e84ec001dddddddddddddd",
                Some("msg_fcf1e84e0001xxxxxxxxxxxxxx"),
                3,
                Kind::Reply,
            ),
        ];
        let analysis = Analysis::new(HarnessKind::Opencode, nodes);
        let resolved: Vec<Option<&str>> = analysis
            .resolved
            .iter()
            .map(|r| r.map(|i| analysis.nodes[i].source_id.as_str()))
            .collect();
        assert_eq!(resolved, [None, None, Some(USER_PART), None]);
    }

    /// A row whose parent was never captured continues the line rather than starting a branch.
    #[rstest]
    fn a_missing_parent_continues_the_line() {
        let mut nodes = chain(&["u1", "a1"], None, 1, 0);
        nodes.extend(chain(&["u2", "a2"], Some("not-captured"), 1, 10));
        let heads = heads_of(HarnessKind::ClaudeCode, nodes);
        assert_eq!(tips(&heads), [tip("a2", 4)]);
    }

    /// A parent cycle (a corrupt transcript) is cut rather than looping.
    #[rstest]
    fn a_parent_cycle_is_cut() {
        let nodes = vec![
            node("a", Some("c"), 1, 0, Kind::Prompt),
            node("b", Some("a"), 1, 1, Kind::Reply),
            node("c", Some("b"), 1, 2, Kind::Prompt),
        ];
        let heads = heads_of(HarnessKind::ClaudeCode, nodes);
        assert_eq!(tips(&heads), [tip("c", 3)]);
    }

    /// An arbitrary forest of rows: each row's parent is an earlier one (or none, or one not
    /// stored), on one of three hosts, and some are substantive. Every other row is numbered like
    /// the one before it, as two Codex hosts would.
    fn arb_rows() -> impl Strategy<Value = Vec<Node>> {
        prop::collection::vec((0..8_usize, 1..4_u8, any::<bool>(), 0..4_i64, any::<bool>()), 1..24)
            .prop_map(|specs| {
                specs
                    .into_iter()
                    .zip(0_i64..)
                    .enumerate()
                    .map(|(i, ((up, on, subst, jitter, dangling), n))| {
                        let parent = if dangling {
                            Some("missing".to_owned())
                        } else {
                            i.checked_sub(up + 1).map(|p| format!("n{p:02}"))
                        };
                        Node {
                            source_id: format!("n{i:02}"),
                            parent,
                            host: Some(host(on)),
                            at: n * 4 + jitter,
                            seq: Some(n / 2),
                            substantive: subst,
                            assistant: subst && n % 2 == 1,
                            header: false,
                        }
                    })
                    .collect()
            })
    }

    fn check_invariants(harness: HarnessKind, nodes: &[Node]) -> Result<(), TestCaseError> {
        let analysis = Analysis::new(harness, nodes.to_vec());
        let heads = analysis.session_heads();
        prop_assert!(!heads.heads.is_empty());
        for pair in heads.heads.windows(2) {
            prop_assert!(
                (pair[0].last_at, pair[0].source_id.as_ref())
                    >= (pair[1].last_at, pair[1].source_id.as_ref())
            );
        }
        for head in &heads.heads {
            let path = analysis.path_to(head.source_id.as_ref());
            let last = path.last().map(|&i| analysis.nodes[i].source_id.as_str());
            prop_assert_eq!(last, Some(head.source_id.as_ref()));
            // Every head's path runs through the branch point, and counts the rows past it.
            let past = match &heads.branch_point {
                None => path.len(),
                Some(bp) => {
                    let at = path.iter().position(|&i| analysis.nodes[i].source_id == bp.as_ref());
                    prop_assert!(at.is_some(), "the branch point is off {}'s path", head.source_id);
                    path.len() - at.unwrap_or_default() - 1
                }
            };
            prop_assert_eq!(head.rows, past as u64);
        }
        // Divergence is a fork whose sides different hosts started, which leaves a head on
        // each side (whoever carried them on since).
        if heads.diverged {
            prop_assert!(heads.heads.len() > 1, "{heads:?}");
        }
        if heads.heads.len() == 1 {
            prop_assert!(heads.branch_point.is_none());
        }
        Ok(())
    }

    fn arb_harness() -> impl Strategy<Value = HarnessKind> {
        prop_oneof![
            Just(HarnessKind::ClaudeCode),
            Just(HarnessKind::Codex),
            Just(HarnessKind::Opencode),
            Just(HarnessKind::Copilot),
        ]
    }

    /// A deterministic Fisher-Yates shuffle.
    fn shuffle<T>(items: &mut [T], mut seed: u64) {
        for i in (1..items.len()).rev() {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let j = usize::try_from((seed >> 33) % (i as u64 + 1)).unwrap();
            items.swap(i, j);
        }
    }

    proptest! {
        /// Heads depend only on the rows, never on the order they are handed over in.
        #[test]
        fn heads_do_not_depend_on_row_order(
            nodes in arb_rows(),
            harness in arb_harness(),
            seed in any::<u64>(),
        ) {
            let expected = heads_of(harness, nodes.clone());
            let mut shuffled = nodes;
            shuffle(&mut shuffled, seed);
            prop_assert_eq!(heads_of(harness, shuffled), expected);
        }

        #[test]
        fn heads_hold_their_invariants(nodes in arb_rows(), harness in arb_harness()) {
            check_invariants(harness, &nodes)?;
        }
    }

    // --- stored ---------------------------------------------------------------------------------

    fn handle(harness: HarnessKind) -> HarnessSession {
        HarnessSession {
            harness,
            session: NativeSessionId::from("s".to_owned()),
        }
    }

    /// `node` as a captured row of session `s`.
    fn message(harness: HarnessKind, n: &Node) -> Message {
        let (role, content) = match (n.substantive, n.assistant) {
            (true, false) => (Role::User, vec![Content::Text(format!("hi {}", n.source_id))]),
            (true, true) => (Role::Assistant, vec![Content::Text(format!("ok {}", n.source_id))]),
            (false, _) => (Role::Tool, vec![Content::Other(serde_json::json!(n.source_id))]),
        };
        Message::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .session(handle(harness))
            .source_id(SourceId::from(n.source_id.clone()))
            .parent_source_id(n.parent.clone().map(SourceId::from))
            .timestamp(AiSessionDatabase::time_from_millis(n.at).unwrap())
            .role(role)
            .content(content)
            .host(n.host.as_deref().map(|h| HostId(uuid::Uuid::parse_str(h).unwrap())))
            .seq(n.seq.map(|s| u64::try_from(s).unwrap()))
            .build()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        /// The heads stored as rows arrive, one at a time in any order or deferred to one
        /// refresh, are what working them out from all the rows at once gives.
        #[test]
        fn stored_heads_do_not_depend_on_arrival_order(
            nodes in arb_rows(),
            harness in arb_harness(),
            seed in any::<u64>(),
            deferred in any::<bool>(),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let expected = heads_of(harness, nodes.clone());
            let mut messages: Vec<Message> = nodes.iter().map(|n| message(harness, n)).collect();
            shuffle(&mut messages, seed);
            let (session, reread, rebuilt) = rt.block_on(async {
                let db = AiSessionDatabase::in_memory().await.unwrap();
                let deferral = deferred.then(|| db.defer_heads());
                for m in &messages {
                    db.append(m).await.unwrap();
                }
                drop(deferral);
                db.refresh_heads().await.unwrap();
                let session = db.get_session(&handle(harness)).await.unwrap().unwrap();
                let reread = db.heads(&handle(harness)).await.unwrap().unwrap();
                db.rebuild_heads().await.unwrap();
                let rebuilt = db.heads(&handle(harness)).await.unwrap().unwrap();
                (session, reread, rebuilt)
            });
            prop_assert_eq!(&session.heads, &expected.heads);
            prop_assert_eq!(&session.branch_point, &expected.branch_point);
            prop_assert_eq!(session.diverged, expected.diverged);
            prop_assert_eq!(&reread, &expected);
            prop_assert_eq!(&rebuilt, &expected);
        }
    }

    async fn stored(harness: HarnessKind, nodes: &[Node]) -> AiSessionDatabase {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for n in nodes {
            db.append(&message(harness, n)).await.unwrap();
        }
        db
    }

    fn ids(messages: &[Message]) -> Vec<String> {
        messages.iter().map(|m| m.source_id.to_string()).collect()
    }

    fn known(ids: &[&str]) -> HashSet<SourceId> {
        ids.iter().map(|id| SourceId::from((*id).to_owned())).collect()
    }

    /// A branch's path runs from the root to its head; what a transcript holding part of it
    /// lacks is what comes after the last row of it that it holds.
    #[rstest]
    #[case::the_start(&["u1", "a1"], &["v2", "b2", "v3"])]
    #[case::nothing(&[], &["u1", "a1", "v2", "b2", "v3"])]
    #[case::everything(&["u1", "a1", "v2", "b2", "v3"], &[])]
    #[case::another_branch(&["u1", "a1", "u2", "a2"], &["v2", "b2", "v3"])]
    #[tokio::test]
    async fn what_a_transcript_lacks_of_a_branch(#[case] holds: &[&str], #[case] missing: &[&str]) {
        let mut nodes = chain(&["u1", "a1", "u2", "a2"], None, 1, 0);
        nodes.extend(chain(&["v2", "b2", "v3"], Some("a1"), 2, 10));
        let db = stored(HarnessKind::ClaudeCode, &nodes).await;
        let session = handle(HarnessKind::ClaudeCode);

        let heads = db.heads(&session).await.unwrap().unwrap();
        assert!(heads.diverged);
        assert_eq!(bp(&heads).as_deref(), Some("a1"));
        let head = SourceId::from("v3".to_owned());
        assert_eq!(ids(&db.branch_path(&session, &head).await.unwrap()), [
            "u1", "a1", "v2", "b2", "v3"
        ]);
        let lacks = db.missing_after(&session, &head, &known(holds)).await.unwrap();
        assert_eq!(ids(&lacks), missing);
    }

    #[rstest]
    #[tokio::test]
    async fn a_row_off_the_tree_has_no_path() {
        let db = stored(HarnessKind::ClaudeCode, &chain(&["u1", "a1"], None, 1, 0)).await;
        let session = handle(HarnessKind::ClaudeCode);
        let nope = SourceId::from("nope".to_owned());
        assert!(db.branch_path(&session, &nope).await.unwrap().is_empty());
    }

    /// The session row carries its heads, and a row its resolved parent, while the pointer
    /// itself stays as captured.
    #[rstest]
    #[tokio::test]
    async fn the_session_carries_its_heads_and_rows_their_resolved_parent() {
        let nodes = vec![
            node(USER_PART, None, 1, 0, Kind::Prompt),
            node("prt_fcf1e84ea001cccccccccccccc", Some(USER_MESSAGE), 1, 2, Kind::Reply),
        ];
        let db = stored(HarnessKind::Opencode, &nodes).await;
        let session = handle(HarnessKind::Opencode);
        let row = db.get_session(&session).await.unwrap().unwrap();
        assert_eq!(row.heads.len(), 1);
        assert!(!row.diverged);
        let messages: Vec<Message> = db.messages(&session).try_collect().await.unwrap();
        assert_eq!(messages[1].parent_row.as_ref().map(AsRef::as_ref), Some(USER_PART));
        // opencode's writer groups parts into messages by it.
        assert_eq!(messages[1].parent_source_id.as_ref().map(AsRef::as_ref), Some(USER_MESSAGE));
    }

    /// Heads marked stale are worked out afresh when read, without storing them.
    #[rstest]
    #[tokio::test]
    async fn stale_heads_are_worked_out_when_read() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let session = handle(HarnessKind::ClaudeCode);
        let deferral = db.defer_heads();
        for n in chain(&["u1", "a1"], None, 1, 0) {
            db.append(&message(HarnessKind::ClaudeCode, &n)).await.unwrap();
        }
        drop(deferral);
        assert!(db.get_session(&session).await.unwrap().unwrap().heads.is_empty());
        assert_eq!(tips(&db.heads(&session).await.unwrap().unwrap()), [tip("a1", 2)]);
        assert_eq!(db.refresh_heads().await.unwrap(), 1);
        assert_eq!(db.get_session(&session).await.unwrap().unwrap().heads.len(), 1);
        assert_eq!(db.refresh_heads().await.unwrap(), 0);
    }

    /// A replayed record carrying a `seq` its row lacks (it arrived while this host ran an older
    /// build) fills it in, and moves the heads.
    #[rstest]
    #[tokio::test]
    async fn a_replayed_row_fills_in_a_missing_seq() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let mut row = message(HarnessKind::Codex, &node("p0", None, 1, 0, Kind::Prompt));
        row.seq = None;
        db.append(&row).await.unwrap();
        row.seq = Some(7);
        db.append(&row).await.unwrap();
        let messages: Vec<Message> =
            db.messages(&handle(HarnessKind::Codex)).try_collect().await.unwrap();
        assert_eq!(messages[0].seq, Some(7));
    }

    /// The real-data regression, run by hand against a copy of a real sidecar (never the live
    /// one: opening it migrates it): `ATUIN_HEADS_SIDECAR=/path/to/copy cargo test -p
    /// atuin-client real_sidecar -- --ignored --nocapture`.
    #[rstest]
    #[ignore = "needs a copy of a real sidecar"]
    #[tokio::test]
    async fn real_sidecar_heads() {
        let path = std::env::var("ATUIN_HEADS_SIDECAR").expect("ATUIN_HEADS_SIDECAR");
        let started = std::time::Instant::now();
        let db = AiSessionDatabase::open(&path).await.unwrap();
        println!("opened (migrated, heads computed) in {:?}", started.elapsed());
        let started = std::time::Instant::now();
        let rebuilt = db.rebuild_heads().await.unwrap();
        println!("rebuilt the heads of {rebuilt} sessions in {:?}", started.elapsed());

        let sessions =
            db.list_sessions(&crate::ai_session::SessionFilter::default()).await.unwrap();
        let mut counts = std::collections::BTreeMap::new();
        for s in &sessions {
            let key = (format!("{:?}", s.handle.harness), s.heads.len().min(5), s.diverged);
            *counts.entry(key).or_insert(0) += 1;
        }
        for ((harness, heads, diverged), n) in &counts {
            println!("{harness:>10} heads={heads} diverged={diverged:<5} sessions={n}");
        }
        for s in sessions.iter().filter(|s| s.diverged) {
            println!(
                "diverged: {:?} {} bp={:?}",
                s.handle.harness, s.handle.session, s.branch_point
            );
            for h in &s.heads {
                println!("  {} host={:?} rows={} at={}", h.source_id, h.host, h.rows, h.last_at);
            }
        }

        let regression = sessions
            .iter()
            .find(|s| s.handle.session.as_ref() == "7aaabc31-1631-4756-8810-d033deb08da5")
            .expect("the regression session");
        assert!(regression.diverged);
        let hosts: HashSet<String> =
            regression.heads.iter().filter_map(|h| h.host.map(|h| h.0.to_string())).collect();
        assert!(hosts.iter().any(|h| h.starts_with("01888135")), "{hosts:?}");
        assert!(hosts.iter().any(|h| h.starts_with("01a0e0e0")), "{hosts:?}");
        let multi_one_host = sessions
            .iter()
            .filter(|s| s.handle.harness == HarnessKind::ClaudeCode && s.heads.len() > 1)
            .filter(|s| !s.diverged)
            .count();
        println!("same-host multi-head Claude sessions: {multi_one_host}");
        assert_eq!(sessions.iter().filter(|s| s.diverged).count(), 1);
    }
}
