//! Utilities for operating on processes.

use std::ops::ControlFlow;
use std::time::Duration;

use rustix::io::Errno;
use rustix::process::{self, Pid, Signal};

/// Gracefully terminate the process via `SIGTERM`.
///
/// If the process does not gracefully terminate, it is forcefully killed via `SIGKILL`.
///
/// `expected_start_time` is the start_time the caller expects the process at `pid` to have (for
/// example, the value persisted in the daemon pidfile). It is used to detect pid reuse: if the live
/// process's start_time differs from `expected_start_time`, the pid has been recycled by an
/// unrelated process and it must NOT be signalled. When `expected_start_time` is `None` (an
/// old-format pidfile that never persisted the identity), we fall back to best-effort liveness.
pub async fn force_terminate(
    pid: Pid,
    expected_start_time: Option<u64>,
    timeout: Duration,
) -> Result<(), std::io::Error> {
    // Guard against pid reuse BEFORE sending any signal. If the caller told us which process to
    // expect and the process currently occupying `pid` has a different start_time, the pid has been
    // recycled by an unrelated process. Signalling it would kill an innocent bystander, so we bail
    // out cleanly — there is nothing of ours left to terminate.
    if let Some(expected) = expected_start_time
        && process_start_time(pid) != Some(expected)
    {
        return Ok(());
    }

    let identity = expected_start_time.or_else(|| process_start_time(pid));

    let is_original_alive = || match identity {
        Some(start_time) => process_start_time(pid) == Some(start_time),
        None => is_alive(pid),
    };

    match process::kill_process(pid, Signal::TERM) {
        Ok(()) => {}
        Err(Errno::SRCH) => return Ok(()),
        Err(errno) => return Err(errno.into()),
    }

    let exited = crate::os::process::EXIT_BACKOFF
        .retry_sync(
            || {
                if is_original_alive() {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(())
                }
            },
            timeout,
        )
        .await;

    if exited.is_ok() {
        return Ok(());
    }

    if !is_original_alive() {
        return Ok(());
    }

    match process::kill_process(pid, Signal::KILL) {
        Ok(()) => {}
        Err(Errno::SRCH) => return Ok(()),
        Err(errno) => return Err(errno.into()),
    }

    Ok(())
}

/// Check whether the given pid is alive.
#[must_use]
pub fn is_alive(pid: Pid) -> bool {
    !matches!(process::test_kill_process(pid), Err(Errno::SRCH))
}

/// Get when the process was started in seconds.
pub fn process_start_time(pid: Pid) -> Option<u64> {
    let pid = sysinfo::Pid::from_u32(pid.as_raw_nonzero().get().unsigned_abs());
    let mut system = sysinfo::System::new();
    if system.refresh_process(pid) {
        system.process(pid).map(sysinfo::Process::start_time)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command};

    use super::*;

    /// A victim process we own, killed on drop so a failing test never leaks a `sleep`.
    struct Victim(Child);

    impl Victim {
        fn spawn() -> Self {
            // A long-lived child we own; it stands in for an innocent process that has been
            // assigned a pid previously held by the daemon.
            let child = Command::new("sleep").arg("60").spawn().expect("failed to spawn victim");
            Self(child)
        }

        fn pid(&self) -> Pid {
            Pid::from_raw(i32::try_from(self.0.id()).unwrap()).unwrap()
        }

        /// Poll until the victim has exited, reaping the zombie, for up to `timeout`.
        ///
        /// Returns `true` if it exited within the window. `try_wait` reaps the child, so this also
        /// avoids leaving a zombie that would still answer `process_start_time`.
        fn exited_within(&mut self, timeout: Duration) -> bool {
            let deadline = std::time::Instant::now() + timeout;
            loop {
                match self.0.try_wait() {
                    Ok(Some(_)) => return true,
                    Ok(None) => {}
                    Err(_) => return true,
                }
                if std::time::Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        /// Assert the victim stays alive for the whole `grace` window (was never signalled).
        fn survives(&mut self, grace: Duration) -> bool {
            !self.exited_within(grace)
        }
    }

    impl Drop for Victim {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// The core of D2: force_terminate must NOT signal a process whose identity does not match the
    /// expected start_time. An innocent process that inherited a recycled pid must survive.
    #[tokio::test]
    async fn wrong_identity_does_not_terminate_victim() {
        let mut victim = Victim::spawn();
        let pid = victim.pid();

        let actual = process_start_time(pid).expect("victim should have a start_time");
        // A start_time the victim definitely does not have (it started ~now, not far in the future).
        let wrong = actual.wrapping_add(1_000_000);

        force_terminate(pid, Some(wrong), Duration::from_secs(2)).await.unwrap();

        assert!(
            victim.survives(Duration::from_millis(500)),
            "victim was signalled despite a mismatched expected start_time (pid reuse guard failed)"
        );
    }

    /// Positive case: when the expected identity matches, the process is terminated as before.
    #[tokio::test]
    async fn matching_identity_terminates_victim() {
        let mut victim = Victim::spawn();
        let pid = victim.pid();

        let actual = process_start_time(pid).expect("victim should have a start_time");

        force_terminate(pid, Some(actual), Duration::from_secs(2)).await.unwrap();

        assert!(
            victim.exited_within(Duration::from_secs(2)),
            "victim with a matching identity should have been terminated"
        );
    }

    /// Old-format pidfile (no persisted identity): fall back to best-effort liveness and terminate.
    #[tokio::test]
    async fn unknown_identity_terminates_victim() {
        let mut victim = Victim::spawn();
        let pid = victim.pid();

        force_terminate(pid, None, Duration::from_secs(2)).await.unwrap();

        assert!(
            victim.exited_within(Duration::from_secs(2)),
            "victim should be terminated when no identity is persisted"
        );
    }
}
