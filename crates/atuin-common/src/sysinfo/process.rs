//! Utilities for interacting and working with processes.

use sysinfo::{Pid, Process, System};
use thiserror::Error;

/// The absolute maximum number of parents we will walk before giving up.
const MAX_DEPTH: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Error)]
pub enum PidAncestorWalkError {
    #[error("pid {0:?} is not present in the snapshot")]
    Unreachable(Pid),
    #[error("pid {claimed:?} claims to be the parent of {child:?} but started after it")]
    StaleParent { claimed: Pid, child: Pid },
    #[error("encountered cycle -- likely means that a PID number was reused by the system")]
    Cycle(Option<Pid>),
}

pub trait SystemExt {
    /// An iterator over the ancestors of `proc`, nearest parent first, not including `proc` itself.
    ///
    /// Ancestry is read from the [`System`] snapshot, so results reflect the last
    /// [`System::refresh_processes`] call rather than the live process table.
    ///
    /// # Termination
    ///
    /// Yields `Ok(pid)` for each ancestor. The walk ends after one of:
    ///   - a process with no recorded parent, where the iterator simply finishes, or
    ///   - a final `Err(`[`PidAncestorWalkError`]`)` when a parent is absent from the
    ///     snapshot, a pid repeats, or `MAX_DEPTH` hops are exceeded.
    ///
    /// At most one `Err` is ever yielded, and when present it is always the last item.
    #[must_use = "an ancestry walk does nothing unless iterated"]
    fn walk_parents(
        &self,
        proc: &Process,
    ) -> impl Iterator<Item = Result<Pid, PidAncestorWalkError>>;
}

impl SystemExt for System {
    fn walk_parents(
        &self,
        proc: &Process,
    ) -> impl Iterator<Item = Result<Pid, PidAncestorWalkError>> {
        // The pid the walk began at, retained to detect a wrap-around to the origin.
        let start = proc.pid();
        // The pid whose parent the next iteration produces, or `None` once the walk ends.
        let mut cursor = Some(start);
        let mut depth = 0usize;

        std::iter::from_fn(move || {
            // Taking the cursor fuses the iterator: every early return below leaves it
            // `None`, and only the success path re-arms it.
            let current = cursor.take()?;

            if depth >= MAX_DEPTH {
                return Some(Err(PidAncestorWalkError::Cycle(None)));
            }

            let Some(child) = self.process(current) else {
                return Some(Err(PidAncestorWalkError::Unreachable(current)));
            };

            // A process with no recorded parent ends the walk without an error.
            let parent_pid = child.parent()?;

            if parent_pid == current || parent_pid == start {
                return Some(Err(PidAncestorWalkError::Cycle(Some(parent_pid))));
            }

            let Some(parent) = self.process(parent_pid) else {
                return Some(Err(PidAncestorWalkError::Unreachable(parent_pid)));
            };

            if parent.start_time() > child.start_time() {
                return Some(Err(PidAncestorWalkError::StaleParent {
                    claimed: parent_pid,
                    child: current,
                }));
            }

            cursor = Some(parent_pid);
            depth += 1;
            Some(Ok(parent_pid))
        })
    }
}
