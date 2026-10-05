//! A session's heads, and whether it diverged, worked out on demand from its rows.
//!
//! A session keeps one id across machines. Each machine's transcript is a line of it, and the
//! synced rows hold every line at once: a session continued on a second host from a copy that was
//! behind continues from an earlier row than the first host's last, and one rewound (or
//! interrupted and continued) on a single host holds a dead end. Sync resumes a copy as it is,
//! fast-forwards one that is behind on its own line, and otherwise offers to fork from a head; it
//! needs to know which heads there are, and the rows from the root to each.
//!
//! Every row of a session is a node of one tree (a forest, really), built per harness from what
//! its rows say:
//!
//! - **Claude Code and Pi** link each line to the one before it (`parent_source_id`). Rows keyed
//!   on their content (`syn-`: titles and other lines with no id) are no node; a session holding
//!   nothing else is linear.
//! - **Codex** lines name no parent, so capture links each row to the row it captured before it
//!   (see the daemon's `MessageEnricher`). Rows captured before it did have no parent: each
//!   follows on from the row before it, as a row whose parent is not stored does (below).
//! - **opencode** is linear. In time order, each row follows on from its host's row before it,
//!   and a host's first row from the row before it at all (it carries on what it restored). A
//!   host whose rows come back after another host's rows (any rows, tool rows too) have come
//!   between is interleaving: the session parts where that host left off, and the rows other
//!   hosts wrote since are on none of its paths. (A side holding only tool rows still makes no
//!   branch of its own; see below.)
//! - Anything else is linear, in time order.
//!
//! A row whose parent is not stored (a line capture keeps no row of, or one not synced yet)
//! follows on from the row before it from its own host, or from the row before it at all when its
//! host has none (a host's first row carries on what other hosts left). Codex rows captured
//! before capture linked them follow on the same way. Time order is by timestamp, then source id; nothing
//! depends on the order rows arrived in.
//!
//! Most forks are nothing: parallel tool calls, attachments and compaction branch the tree too.
//! A fork counts only when at least two of the subtrees below it hold a
//! [substantive](is_substantive) row, a user prompt or assistant text; the rest of it stays with
//! the branch it was on. Each branch that reaches no fork that counts is a head, whose tip is the
//! newest leaf below the last of its substantive rows (so a later tool result beside a reply never
//! leaves the reply off the head's path). The session is *diverged* when the substantive sides of a
//! fork that counts were started by different hosts, a side's host being that of its earliest
//! substantive row (a side can open with a tool row one host captured before another host's prompt
//! made it substantive); heads of one host alone (a rewind, an interrupt) are not a divergence, and
//! neither is one host carrying on a line another left.
//!
//! Nothing here is stored: [`AiSessionDatabase::analyse`] reads the session's rows each time.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use atuin_common::harnesstools::session::{Role, is_substantive};
use atuin_domain::record::HostId;
use futures::TryStreamExt;
use time::OffsetDateTime;

use super::{AiSessionDatabase, DbError};
use crate::ai_session::{HarnessKind, HarnessSession, Message, SourceId};

/// The prefix of a source id capture made up from a line's content (see the daemon's
/// `MessageEnricher`).
const SYNTHETIC: &str = "syn-";

/// The tip of one of a session's branches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Head {
    /// The newest row on the branch.
    pub source_id: SourceId,
    /// The host that captured it.
    pub host: Option<HostId>,
    /// When it was written.
    pub last_at: OffsetDateTime,
    /// The [substantive](is_substantive) rows from the session's root to here: prompts and
    /// assistant text.
    pub messages: u64,
}

/// A session's branches (see the module docs), worked out from its rows by
/// [`AiSessionDatabase::analyse`].
#[derive(Debug)]
pub struct Analysis {
    /// The session's rows, in time order.
    rows: Vec<Message>,
    /// Each row's parent in the branch tree; `None` for a root, and for a row that is no node.
    parent: Vec<Option<usize>>,
    in_tree: Vec<bool>,
    /// Newest first.
    heads: Vec<Head>,
    diverged: bool,
}

/// How a harness's rows string together (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Model {
    /// By parent pointers. `chained`: every row is a node, and one with no parent follows on
    /// from the row before it (Codex, whose capture links rows); otherwise it is a root, and
    /// content-addressed rows are no node.
    Tree {
        chained: bool,
    },
    Interleaving,
    Linear,
}

impl Model {
    const fn of(harness: HarnessKind) -> Self {
        match harness {
            HarnessKind::ClaudeCode | HarnessKind::Pi => Self::Tree { chained: false },
            HarnessKind::Codex => Self::Tree { chained: true },
            HarnessKind::Opencode => Self::Interleaving,
            HarnessKind::Copilot | HarnessKind::Unknown => Self::Linear,
        }
    }
}

impl Analysis {
    /// Work out the branches of a session of `harness` holding `rows`, in any order.
    fn new(harness: HarnessKind, mut rows: Vec<Message>) -> Self {
        let key = |m: &Message| (m.timestamp, m.source_id.as_ref().to_owned());
        rows.sort_by_cached_key(key);
        let substantive: Vec<bool> =
            rows.iter().map(|m| is_substantive(&m.role, &m.content)).collect();

        let mut model = Model::of(harness);
        let mut in_tree = vec![true; rows.len()];
        if model == (Model::Tree { chained: false }) {
            for (i, row) in rows.iter().enumerate() {
                in_tree[i] = !row.source_id.as_ref().starts_with(SYNTHETIC);
            }
            // Nothing but content-addressed rows (a Pi session from before ids): linear.
            if !in_tree.contains(&true) {
                model = Model::Linear;
                in_tree.fill(true);
            }
        }
        let mut parent = match model {
            Model::Tree { chained } => tree_parents(&rows, &in_tree, chained),
            Model::Interleaving => interleaved_parents(&rows),
            Model::Linear => (0..rows.len()).map(|i| i.checked_sub(1)).collect(),
        };
        break_cycles(&mut parent, &in_tree);

        let mut analysis = Self {
            rows,
            parent,
            in_tree,
            heads: Vec::new(),
            diverged: false,
        };
        analysis.find_heads(&substantive);
        analysis
    }

    fn find_heads(&mut self, substantive: &[bool]) {
        let n = self.rows.len();
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut roots = Vec::new();
        for i in (0..n).filter(|&i| self.in_tree[i]) {
            match self.parent[i] {
                Some(p) => children[p].push(i),
                None => roots.push(i),
            }
        }

        // Parents before children, so counts fill in going down and substance going up.
        let mut preorder = Vec::with_capacity(n);
        let mut stack: Vec<usize> = roots.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            preorder.push(i);
            stack.extend(children[i].iter().rev());
        }
        let mut messages = vec![0_u64; n];
        for &i in &preorder {
            let above = self.parent[i].map_or(0, |p| messages[p]);
            messages[i] = above + u64::from(substantive[i]);
        }
        // The earliest substantive row of each subtree, if any: the row that started it, as far
        // as divergence goes (a tool row can come first on a side another host made substantive).
        let earliest = |a: Option<usize>, b: Option<usize>| match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let mut first: Vec<Option<usize>> = vec![None; n];
        for &i in preorder.iter().rev() {
            if substantive[i] {
                first[i] = earliest(first[i], Some(i));
            }
            if let Some(p) = self.parent[i] {
                first[p] = earliest(first[p], first[i]);
            }
        }

        // Hand out branches going down: a fork that counts closes the branch reaching it and
        // opens one per substantive side; everything else stays on the branch it is on.
        let mut branch = vec![0_usize; n];
        let mut split = vec![false];
        let mut diverged = false;
        let rows = &self.rows;
        let mut fork = |kids: &[usize], on: usize, branch: &mut Vec<usize>| {
            for &k in kids {
                branch[k] = on;
            }
            let sides: Vec<usize> = kids.iter().copied().filter(|&k| first[k].is_some()).collect();
            if sides.len() < 2 {
                return;
            }
            split[on] = true;
            let hosts: HashSet<Option<HostId>> =
                sides.iter().filter_map(|&k| first[k]).map(|f| rows[f].host).collect();
            diverged |= hosts.len() > 1;
            for &k in &sides {
                branch[k] = split.len();
                split.push(false);
            }
        };
        fork(&roots, 0, &mut branch);
        for &i in &preorder {
            fork(&children[i], branch[i], &mut branch);
        }

        // Each branch no fork closed ends in a leaf below all its substantive rows (which, with no
        // fork that counts, lie on one line: the leaves holding the most messages), the newest of
        // those, a header (a Pi `session` line) only if nothing else.
        let header = |i: usize| rows[i].role == Role::Other("session".to_owned());
        let mut tips: HashMap<usize, usize> = HashMap::new();
        for &i in &preorder {
            if !children[i].is_empty() || split[branch[i]] {
                continue;
            }
            let rank = |i: usize| (messages[i], !header(i), i);
            let better = tips.get(&branch[i]).is_none_or(|&t| rank(i) > rank(t));
            if better {
                tips.insert(branch[i], i);
            }
        }
        let mut tips: Vec<usize> = tips.into_values().collect();
        tips.sort_by_key(|&i| Reverse(i));

        self.heads = tips
            .into_iter()
            .map(|tip| Head {
                source_id: rows[tip].source_id.clone(),
                host: rows[tip].host,
                last_at: rows[tip].timestamp,
                messages: messages[tip],
            })
            .collect();
        self.diverged = diverged;
    }

    /// The tips of the session's branches, newest first: one for a session on a single line,
    /// none for one with no rows.
    #[must_use]
    pub fn heads(&self) -> &[Head] {
        &self.heads
    }

    /// Whether hosts started different branches of the session: no head is then a copy of
    /// another one is behind, and continuing one means forking from it.
    #[must_use]
    pub const fn diverged(&self) -> bool {
        self.diverged
    }

    /// The rows from the root of the session down to `row` (a [head](Self::heads), or any row on
    /// a branch), in order, following the branch tree: what a transcript on that branch holds,
    /// for forking from it, or for fast-forwarding a copy holding the start of it.
    ///
    /// Only rows that are nodes of the tree are on it (see the module docs): for Claude Code and
    /// Pi, the rows with an id of their own, so not titles or other content-addressed rows. Empty
    /// when `row` is not stored, or is no node.
    #[must_use]
    pub fn path_to(&self, row: &SourceId) -> Vec<&Message> {
        let found =
            self.rows.iter().zip(&self.in_tree).position(|(m, t)| *t && m.source_id == *row);
        let mut path = Vec::new();
        let mut at = found;
        while let Some(i) = at {
            path.push(&self.rows[i]);
            at = self.parent[i];
        }
        path.reverse();
        path
    }
}

/// Parent pointers resolved to rows (see [`Model::Tree`]).
fn tree_parents(rows: &[Message], in_tree: &[bool], chained: bool) -> Vec<Option<usize>> {
    let by_id: HashMap<&SourceId, usize> = rows
        .iter()
        .enumerate()
        .filter(|&(i, _)| in_tree[i])
        .map(|(i, m)| (&m.source_id, i))
        .collect();
    let mut previous = None;
    let mut by_host: HashMap<Option<HostId>, usize> = HashMap::new();
    let mut parent = vec![None; rows.len()];
    for i in (0..rows.len()).filter(|&i| in_tree[i]) {
        let pointer = rows[i].parent_source_id.as_ref();
        parent[i] = match pointer.and_then(|p| by_id.get(p)) {
            Some(&p) if p != i => Some(p),
            // A parent that is not stored is a line capture kept no row of (or one not synced
            // yet), from the host's own transcript: the row follows on from its host's row before
            // it, not another host's that came between. A host's first row carries on from what
            // other hosts wrote before (it restored the session from them), so it follows the
            // row before it at all; a root would split it off as if it shared nothing.
            _ if chained || pointer.is_some() => by_host.get(&rows[i].host).copied().or(previous),
            _ => None,
        };
        previous = Some(i);
        by_host.insert(rows[i].host, i);
    }
    parent
}

/// opencode: in time order, linear unless hosts interleave (see the module docs). Tool rows count
/// like any other, so another host's tool row between a host's rows is on none of that host's
/// paths; a side holding only tool rows still parts no branch (see [`Analysis`]).
fn interleaved_parents(rows: &[Message]) -> Vec<Option<usize>> {
    let mut previous = None;
    let mut by_host: HashMap<Option<HostId>, usize> = HashMap::new();
    let mut parent = vec![None; rows.len()];
    for (i, row) in rows.iter().enumerate() {
        parent[i] = by_host.get(&row.host).copied().or(previous);
        previous = Some(i);
        by_host.insert(row.host, i);
    }
    parent
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

impl AiSessionDatabase {
    /// Work out the branches of `session` from the rows stored for it (see [`Analysis`]): its
    /// heads, whether it diverged, and the rows on each branch. Nothing is stored, so it reads a
    /// read-only database too; a session with no rows has no heads.
    pub async fn analyse(&self, session: &HarnessSession) -> Result<Analysis, DbError> {
        let rows: Vec<Message> = self.messages(session).try_collect().await?;
        Ok(Analysis::new(session.harness, rows))
    }
}

#[cfg(test)]
mod tests;
