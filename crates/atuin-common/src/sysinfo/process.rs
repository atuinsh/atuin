//! Utilities for interacting and working with processes.

use sysinfo::{Pid, Process, System};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Error)]
pub enum PidAncestorStopError {
    #[error("pid {0:?} is not present in the snapshot")]
    Unreachable(Pid),
    #[error("pid {claimed:?} claims to be the parent of {child:?} but started after it")]
    StaleParent { claimed: Pid, child: Pid },
    #[error("encountered cycle -- likely means that a PID number was reused by the system")]
    Cycle(Option<Pid>),
}

#[must_use = "an ancestry walk does nothing unless iterated"]
pub struct PidAncestors<'a> {
    system: &'a System,
    /// The pid the walk began at, retained to detect a wrap-around to the origin.
    start: Pid,
    /// The pid whose parent will be produced by the next call to `next`.
    cursor: Pid,
    depth: usize,
    /// Set exactly once, when iteration ends. `Some` implies exhausted.
    stop: Option<Result<(), PidAncestorStopError>>,
}

impl PidAncestors<'_> {
    /// The absolute maximum number of parents we will iterate until giving up.
    const MAX_DEPTH: usize = 128;

    /// Why the walk ended, or `None` if it has not ended yet.
    ///
    /// Returns `Ok(())` if the walk ended normally, and [`PidAncestorStopError`] otherwise.
    #[must_use]
    pub fn result(&self) -> Option<Result<(), PidAncestorStopError>> {
        self.stop
    }
}

impl Iterator for PidAncestors<'_> {
    type Item = Pid;

    fn next(&mut self) -> Option<Self::Item> {
        if self.stop.is_some() {
            return None;
        }

        if self.depth >= Self::MAX_DEPTH {
            self.stop = Some(Err(PidAncestorStopError::Cycle(None)));
            return None;
        }

        let system = self.system;

        let Some(child) = system.process(self.cursor) else {
            self.stop = Some(Err(PidAncestorStopError::Unreachable(self.cursor)));
            return None;
        };

        let Some(parent_pid) = child.parent() else {
            self.stop = Some(Ok(()));
            return None;
        };

        if parent_pid == self.cursor || parent_pid == self.start {
            self.stop = Some(Err(PidAncestorStopError::Cycle(Some(parent_pid))));
            return None;
        }

        let Some(parent) = system.process(parent_pid) else {
            self.stop = Some(Err(PidAncestorStopError::Unreachable(parent_pid)));
            return None;
        };

        if parent.start_time() > child.start_time() {
            self.stop = Some(Err(PidAncestorStopError::StaleParent {
                claimed: parent_pid,
                child: self.cursor,
            }));
            return None;
        }

        self.cursor = parent_pid;
        self.depth += 1;
        Some(parent_pid)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(Self::MAX_DEPTH.saturating_sub(self.depth)))
    }
}

impl std::iter::FusedIterator for PidAncestors<'_> {}

pub trait SystemExt {
    /// An iterator over the ancestors of `proc`, nearest parent first, not including `proc` itself.
    ///
    /// Ancestry is read from the [`System`] snapshot, so results reflect the last
    /// [`System::refresh_processes`] call rather than the live process table.
    ///
    /// # Termination
    ///
    /// Stops at the first of:
    ///   - a process with no recorded parent,
    ///   - a parent absent from the snapshot,
    ///   - a repeat of the starting pid,
    ///   - or `MAX_DEPTH` hops.
    fn walk_parents(&self, proc: &Process) -> PidAncestors<'_>;
}

impl SystemExt for System {
    fn walk_parents(&self, proc: &Process) -> PidAncestors<'_> {
        PidAncestors {
            system: self,
            start: proc.pid(),
            cursor: proc.pid(),
            depth: 0,
            stop: None,
        }
    }
}
