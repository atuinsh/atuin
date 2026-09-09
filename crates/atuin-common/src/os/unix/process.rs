//! Utilities for operating on processes.

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::time::Duration;

use rustix::io::Errno;
use rustix::process::{self, Pid, Signal};

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

/// Get a process's current working directory.
#[must_use]
pub fn cwd(pid: Pid) -> Option<PathBuf> {
    let pid = u32::try_from(pid.as_raw_pid()).ok()?;

    if cfg!(target_os = "linux") {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    } else {
        let pid = sysinfo::Pid::from_u32(pid);
        let mut system = sysinfo::System::new();
        if !system.refresh_process_specifics(
            pid,
            sysinfo::ProcessRefreshKind::new().with_cmd(sysinfo::UpdateKind::Always),
        ) {
            return None;
        }
        system.process(pid)?.cwd().map(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    use rstest::rstest;

    use super::*;

    /// Spawn a process that sits still in `dir`.
    fn sleeper(dir: &std::path::Path) -> (Child, Pid) {
        let child = Command::new("sleep").arg("30").current_dir(dir).spawn().unwrap();
        let pid = Pid::from_raw(child.id().cast_signed()).unwrap();
        (child, pid)
    }

    #[rstest]
    #[cfg_attr(target_os = "macos", ignore)] // TODO
    fn reads_the_working_directory_of_another_process() {
        let dir = tempfile::tempdir().unwrap();
        // The temporary directory may sit behind a symlink (/tmp on macOS), and a working
        // directory never does.
        let expected = dir.path().canonicalize().unwrap();
        let (mut child, pid) = sleeper(dir.path());

        // The child chdirs somewhere between fork and exec, so it may not be there yet.
        let deadline = Instant::now() + Duration::from_secs(10);
        while cwd(pid).as_deref() != Some(expected.as_path()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let found = cwd(pid);

        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(found.as_deref(), Some(expected.as_path()));
    }

    /// TODO: TEMPORARY diagnostic for the macOS failure, to be deleted once it is fixed.
    #[cfg(target_os = "macos")]
    #[test]
    fn cwd_layers() {
        use sysinfo::{ProcessRefreshKind, System, UpdateKind};

        let dir = tempfile::tempdir().unwrap();
        let expected = dir.path().canonicalize().unwrap();
        let (mut child, pid) = sleeper(dir.path());
        std::thread::sleep(Duration::from_millis(500));
        let sysinfo_pid = sysinfo::Pid::from_u32(child.id());
        let me = Pid::from_raw(std::process::id().cast_signed()).unwrap();

        println!("expected                  {}", expected.display());
        println!("cwd(child)                {:?}", cwd(pid));
        println!("cwd(self)                 {:?}", cwd(me));

        let mut narrow = System::new();
        let refreshed = narrow.refresh_process_specifics(
            sysinfo_pid,
            ProcessRefreshKind::new().with_cwd(UpdateKind::Always),
        );
        println!("cwd-only refresh          {refreshed}");
        println!("cwd-only found            {}", narrow.process(sysinfo_pid).is_some());
        println!(
            "cwd-only name             {:?}",
            narrow.process(sysinfo_pid).map(sysinfo::Process::name)
        );
        println!(
            "cwd-only exe              {:?}",
            narrow.process(sysinfo_pid).and_then(sysinfo::Process::exe)
        );
        println!(
            "cwd-only cwd              {:?}",
            narrow.process(sysinfo_pid).and_then(sysinfo::Process::cwd)
        );

        let mut wide = System::new();
        let refreshed = wide.refresh_process_specifics(
            sysinfo_pid,
            ProcessRefreshKind::everything().with_cwd(UpdateKind::Always),
        );
        println!("wide refresh              {refreshed}");
        println!("wide found                {}", wide.process(sysinfo_pid).is_some());
        println!(
            "wide cwd                  {:?}",
            wide.process(sysinfo_pid).and_then(sysinfo::Process::cwd)
        );

        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[rstest]
    #[cfg_attr(target_os = "macos", ignore)] // TODO
    fn a_process_that_has_gone_has_no_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        let (mut child, pid) = sleeper(dir.path());
        child.kill().unwrap();
        child.wait().unwrap();

        assert_eq!(cwd(pid), None);
    }
}
