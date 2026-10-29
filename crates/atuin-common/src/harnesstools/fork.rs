//! Forking a session: the same conversation as a new session of the same harness, under a fresh
//! id, linked to the original as its fork. The original is left as it is.
//!
//! A fork is written out from the captured rows by the harness's own writer, as a restore is
//! ([`Harness::rehydrate`](crate::harnesstools::Harness::rehydrate)): tool calls carry over as a
//! restore writes them (as calls where capture kept their input), not mapped onto another
//! harness's tools as a continuation's are ([`continuation`]). What differs is its id, and that it
//! names the session it was forked from ([`RehydrateSession::fork_of`]), which each writer puts
//! where its harness puts a fork's origin, out of the model's sight:
//!
//! - **Claude Code**: `forkedFrom` on every line, as `/branch` writes it.
//! - **Codex**: the rollout's `session_meta.forked_from_id`, the thread; for a segment of one (a
//!   thread reverted into a rollout of its own, `<thread>_<rollout>`), atuin's own
//!   `atuin_forked_from` names the segment. Such a rollout holds only what came after the revert,
//!   and names the rollout holding the rest in its `history_base`: with the original's rollout
//!   here, the fork names the same, so it resumes with the whole history.
//! - **pi**: the header's `parentSession`, the original's *file*, which must be on this machine.
//! - **opencode**: `parentID` names a subagent's parent, and `opencode import` writes no fork link
//!   (`fork_session_id` is a column of opencode 2.0's own session table, which import doesn't
//!   write). So the first message carries a [fork marker](continuation::fork_marker_text) in an
//!   `ignored` text part, which opencode keeps from the model and capture reads as a fork.
//!
//! Rows keep their ids where a transcript's ids are its own (Claude Code's line uuids, Codex's
//! items, pi's entries), as the harnesses' own forks keep them: capture keys rows per session,
//! so the fork's are rows of its own. opencode's message and part ids are keys of the whole
//! database, and `opencode import` skips one that exists already, so its writer gives a fork
//! fresh ones.

use time::OffsetDateTime;

use super::rehydrate::{ForkOf, RehydrateMessage, RehydrateSession};
use super::{AnyHarness, continuation};

/// `original`, recorded by `harness`, as a new session of it forked from `of`: its rows up to
/// `tip` (the row of that source id, it included), or all of them. `None` when no row is `tip`.
#[must_use]
pub fn fork(
    harness: AnyHarness,
    original: &RehydrateSession,
    of: ForkOf,
    tip: Option<&str>,
) -> Option<RehydrateSession> {
    let messages = match tip {
        Some(tip) => up_to(&original.messages, tip)?.to_vec(),
        None => original.messages.clone(),
    };
    Some(RehydrateSession {
        id: continuation::new_session_id(harness, OffsetDateTime::now_utc()),
        messages,
        fork_of: Some(of),
        ..original.clone()
    })
}

/// The rows a fork from `tip` keeps: `messages` up to the row of that source id, it included.
#[must_use]
pub fn up_to<'a>(messages: &'a [RehydrateMessage], tip: &str) -> Option<&'a [RehydrateMessage]> {
    let at = messages.iter().position(|m| m.source_id == tip)?;
    Some(&messages[..=at])
}

#[cfg(test)]
mod tests;
