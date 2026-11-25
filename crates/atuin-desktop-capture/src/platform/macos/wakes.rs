//! When the backend asked each application to build its tree.

use std::collections::HashMap;
use std::time::Instant;

use atuin_common::os::unix::process::is_u32_alive;
use parking_lot::Mutex;

use super::element::Pid;
use super::quirks;

/// When each running application was asked to build its tree.
#[derive(Default)]
pub struct WakeLog(Mutex<HashMap<Pid, Wakes>>);

impl WakeLog {
    /// Notes that `app` was just asked to build its tree, unless it was asked before.
    ///
    /// An application builds its tree on the first request, so a later request must not start
    /// the wait again.
    pub fn note_first(&self, app: Pid) {
        self.0.lock().entry(app).or_insert_with(|| Wakes::first(Instant::now()));
    }

    /// Whether `app` is due to be asked once more, because [`quirks::WAKE_AGAIN`] has passed
    /// since the first request. Notes the second request when it is due.
    pub fn again_is_due(&self, app: Pid) -> bool {
        self.0.lock().get_mut(&app).is_some_and(|wakes| wakes.again(Instant::now()))
    }

    /// Whether `app` may still be building its tree, because it was last asked to build it
    /// less than [`quirks::WAKE_TIME`] ago.
    pub fn may_be_building(&self, app: Pid) -> bool {
        self.0.lock().get(&app).is_some_and(|wakes| wakes.may_be_building(Instant::now()))
    }

    /// Forgets the applications whose processes exited.
    pub fn forget_exited(&self) {
        let now = Instant::now();
        // An entry past the wait only tells that the application built its tree, which matters
        // only while its process runs.
        self.0.lock().retain(|&pid, wakes| wakes.may_be_building(now) || process_runs(pid));
    }

    /// Forgets `app`, whose process exited. A new process can reuse its id.
    pub fn forget(&self, app: Pid) {
        self.0.lock().remove(&app);
    }
}

/// When an application was asked to build its tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Wakes {
    first: Instant,
    /// When the application was asked again, if it was.
    again: Option<Instant>,
}

impl Wakes {
    const fn first(now: Instant) -> Self {
        Self {
            first: now,
            again: None,
        }
    }

    /// Notes the second request, and returns whether it is due now: once
    /// [`quirks::WAKE_AGAIN`] has passed since the first request, and only once.
    fn again(&mut self, now: Instant) -> bool {
        let due = self.again.is_none() && now.duration_since(self.first) >= quirks::WAKE_AGAIN;
        if due {
            self.again = Some(now);
        }
        due
    }

    /// Whether the last request came less than [`quirks::WAKE_TIME`] before `now`.
    fn may_be_building(&self, now: Instant) -> bool {
        let last = self.again.unwrap_or(self.first);
        now.duration_since(last) < quirks::WAKE_TIME
    }
}

/// Whether the process `pid` is still running.
fn process_runs(pid: Pid) -> bool {
    u32::try_from(pid.raw()).is_ok_and(is_u32_alive)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use rstest::rstest;

    use super::*;

    /// The second request comes once, after [`quirks::WAKE_AGAIN`], and starts the wait for
    /// the tree again.
    #[rstest]
    fn an_application_is_woken_again_once() {
        let start = Instant::now();
        let mut wakes = Wakes::first(start);
        let soon = start + quirks::WAKE_AGAIN / 2;
        let due = start + quirks::WAKE_AGAIN;

        assert!(!wakes.again(soon));
        assert!(!wakes.may_be_building(due));
        assert!(wakes.again(due));
        assert!(wakes.may_be_building(due + quirks::WAKE_TIME / 2));
        assert!(!wakes.again(due + quirks::WAKE_AGAIN));
        assert!(!wakes.may_be_building(due + quirks::WAKE_TIME));
    }

    #[rstest]
    fn this_process_runs() {
        let pid = i32::try_from(std::process::id()).expect("a process id fits in an i32");
        assert!(process_runs(Pid::new(pid)));
    }

    #[rstest]
    fn a_reaped_process_does_not_run() {
        let mut child = Command::new("true").spawn().expect("true starts");
        let pid = i32::try_from(child.id()).expect("a process id fits in an i32");
        child.wait().expect("true exits");
        assert!(!process_runs(Pid::new(pid)));
    }
}
