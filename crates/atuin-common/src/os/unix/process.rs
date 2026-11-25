//! Utilities for operating on processes.

use std::ffi::OsString;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::time::Duration;

use rustix::io::Errno;
use rustix::process::{self, Pid, Signal};

mod close_fds;

/// Extension trait for [`std::process::Command`].
pub trait CommandExt {
    /// Make the child process not inherit any file descriptors except the standard streams, like
    /// Python's `close_fds`.
    fn close_fds(&mut self) -> &mut Self;
}

impl CommandExt for std::process::Command {
    fn close_fds(&mut self) -> &mut Self {
        close_fds::close_fds(self)
    }
}

/// Kills a process group with `SIGKILL` when dropped, unless [`Self::disarm`] is called first.
#[must_use = "the group is killed as soon as the guard is dropped"]
#[derive(Debug)]
pub struct KillGroupOnDrop(Option<Pid>);

impl KillGroupOnDrop {
    pub const fn new(group: Pid) -> Self {
        Self(Some(group))
    }

    /// Consume the guard without killing the process group.
    pub fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        if let Some(group) = self.0 {
            let _ = process::kill_process_group(group, Signal::KILL);
        }
    }
}

/// Try to convert a [`u32`] PID into a typed [`Pid`].
///
/// Standard library functions like [`std::process::id`] use [`u32`], but functions in this module
/// expect [`Pid`].
pub fn pid_from_u32(pid: u32) -> Option<Pid> {
    i32::try_from(pid).ok().and_then(rustix::process::Pid::from_raw)
}

/// Gracefully terminate the process via `SIGTERM`.
///
/// If the process does not gracefully terminate, it is forcefully killed via `SIGKILL`.
pub async fn force_terminate(pid: Pid, timeout: Duration) -> Result<(), std::io::Error> {
    let identity = process_start_time(pid);

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

/// Check whether the process with the [`u32`] PID `pid` is alive.
///
/// A PID that doesn't fit in a [`Pid`] is never alive.
#[must_use]
pub fn is_u32_alive(pid: u32) -> bool {
    pid_from_u32(pid).is_some_and(is_alive)
}

/// Get when the process was started in seconds.
pub fn process_start_time(pid: Pid) -> Option<u64> {
    let pid = sysinfo::Pid::from_u32(pid.as_raw_nonzero().get().unsigned_abs());
    let mut system = sysinfo::System::new();
    if system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true) > 0 {
        system.process(pid).map(sysinfo::Process::start_time)
    } else {
        None
    }
}

/// Get a process's current working directory.
#[must_use]
pub fn cwd(pid: Pid) -> Option<PathBuf> {
    let pid = u32::try_from(pid.as_raw_pid()).ok()?;

    if cfg!(target_os = "linux") {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    } else {
        cwd_generic(pid)
    }
}

/// Get the file name of the executable a process is running.
///
/// This is the name the process was exec'd as, so a process that rewrites its `argv[0]` keeps it.
#[must_use]
pub fn name(pid: Pid) -> Option<OsString> {
    let pid = sysinfo::Pid::from_u32(u32::try_from(pid.as_raw_pid()).ok()?);
    let mut system = sysinfo::System::new();
    if system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    ) == 0
    {
        return None;
    }
    Some(system.process(pid)?.name().to_owned())
}

/// Get the path of the executable that a process runs.
///
/// On Linux, the path of an executable that was deleted or replaced after the process started ends
/// with ` (deleted)`.
#[must_use]
pub fn exe(pid: Pid) -> Option<PathBuf> {
    let pid = u32::try_from(pid.as_raw_pid()).ok()?;

    if cfg!(target_os = "linux") {
        std::fs::read_link(format!("/proc/{pid}/exe")).ok()
    } else {
        exe_generic(pid)
    }
}

/// Implementation of [`exe`] that works on all Unix-like systems.
///
/// Linux has a specific, more efficient implementation.
fn exe_generic(pid: u32) -> Option<PathBuf> {
    let pid = sysinfo::Pid::from_u32(pid);
    let mut system = sysinfo::System::new();
    if system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always),
    ) == 0
    {
        return None;
    }
    system.process(pid)?.exe().map(Into::into)
}

/// Implementation of [`cwd`] that works on all Unix-like systems.
///
/// Linux has a specific, more efficient implementation.
fn cwd_generic(pid: u32) -> Option<PathBuf> {
    let pid = sysinfo::Pid::from_u32(pid);
    let mut system = sysinfo::System::new();
    if system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing().with_cwd(sysinfo::UpdateKind::Always),
    ) == 0
    {
        return None;
    }
    system.process(pid)?.cwd().map(Into::into)
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    use rstest::{fixture, rstest};

    use super::*;
    use crate::path::AbsPathBuf;

    /// Spawn a process that sits still in `dir`.
    fn sleeper(dir: &std::path::Path) -> (Child, Pid) {
        let child = Command::new("sleep").arg("30").current_dir(dir).spawn().unwrap();
        let pid = Pid::from_raw(child.id().cast_signed()).unwrap();
        (child, pid)
    }

    /// Wait for `read` to return `expected`, then report what it last returned.
    ///
    /// `spawn` can return before the child is fully set up. The child chdirs between fork and
    /// exec, and the kernel renames the child late in exec, after the parent has resumed.
    fn settles_on<T: PartialEq + ?Sized, O: std::ops::Deref<Target = T>>(
        expected: &T,
        mut read: impl FnMut() -> Option<O>,
    ) -> Option<O> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let found = read();
            if found.as_deref() == Some(expected) || Instant::now() >= deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[rstest]
    fn reads_the_working_directory_of_another_process() {
        let dir = tempfile::tempdir().unwrap();
        // The temporary directory may sit behind a symlink (/tmp on macOS), and a working
        // directory never does.
        let expected = AbsPathBuf::canonicalize(dir.path()).unwrap();
        let (mut child, pid) = sleeper(dir.path());

        let found = settles_on(expected.as_path(), || cwd(pid));

        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(found.as_deref(), Some(expected.as_path()));
    }

    #[rstest]
    fn the_generic_lookup_reads_the_working_directory_too() {
        // Linux takes the /proc route, so without this the portable route would go untested
        // until it reached a platform that has no choice but to use it.
        let dir = tempfile::tempdir().unwrap();
        let expected = AbsPathBuf::canonicalize(dir.path()).unwrap();
        let (mut child, _) = sleeper(dir.path());
        let pid = child.id();

        let found = settles_on(expected.as_path(), || cwd_generic(pid));

        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(found.as_deref(), Some(expected.as_path()));
    }

    /// A `sleep` running under another name, killed when dropped.
    struct RenamedSleep(Child);

    impl Drop for RenamedSleep {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[fixture]
    fn renamed_sleep() -> RenamedSleep {
        use std::os::unix::process::CommandExt;

        RenamedSleep(Command::new("sleep").arg0("renamed").arg("30").spawn().unwrap())
    }

    #[rstest]
    fn names_a_process_by_its_executable_rather_than_its_argv0(renamed_sleep: RenamedSleep) {
        let pid = Pid::from_raw(renamed_sleep.0.id().cast_signed()).unwrap();
        let expected = std::ffi::OsStr::new("sleep");
        assert_eq!(settles_on(expected, || name(pid)).as_deref(), Some(expected));
    }

    /// Both routes find the executable of this process, which no other process can replace while
    /// the test runs.
    #[rstest]
    fn reads_the_executable_of_a_process() {
        let expected = AbsPathBuf::canonicalize(std::env::current_exe().unwrap()).unwrap();
        let pid = std::process::id();

        let found = exe(pid_from_u32(pid).unwrap()).unwrap();
        assert_eq!(AbsPathBuf::canonicalize(found).unwrap(), expected);
        let generic = exe_generic(pid).unwrap();
        assert_eq!(AbsPathBuf::canonicalize(generic).unwrap(), expected);
    }

    #[rstest]
    fn a_process_that_has_gone_has_no_executable() {
        let dir = tempfile::tempdir().unwrap();
        let (mut child, pid) = sleeper(dir.path());
        child.kill().unwrap();
        child.wait().unwrap();

        assert_eq!(exe(pid), None);
    }

    #[rstest]
    fn a_process_that_has_gone_has_no_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        let (mut child, pid) = sleeper(dir.path());
        child.kill().unwrap();
        child.wait().unwrap();

        assert_eq!(cwd(pid), None);
    }

    #[rstest]
    #[case::zero(0, None)]
    #[case::one(1, Some(1))]
    #[case::max(i32::MAX.cast_unsigned(), Some(i32::MAX))]
    #[case::just_over_max(i32::MAX.cast_unsigned() + 1, None)]
    #[case::minus_one_if_wrapped(u32::MAX, None)]
    fn test_pid_from_u32(#[case] pid: u32, #[case] expected: Option<i32>) {
        assert_eq!(pid_from_u32(pid).map(Pid::as_raw_pid), expected);
    }
}
