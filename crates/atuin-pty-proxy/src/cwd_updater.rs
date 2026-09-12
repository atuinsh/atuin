//! Updates this PTY proxy's CWD to match the CWD of its child.
//!
//! This is necessary to enable programs like tmux to detect the CWD of the child.

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::time::Duration;

use atuin_common::os::unix::process;
use portable_pty::MasterPty;
use rustix::process::Pid;

/// The minimum amount of time between successive CWD updates.
///
/// Updating the CWD requires a syscall, so this is used to reduce the performance impact.
const MIN_UPDATE_DELAY: Duration = Duration::from_millis(100);

/// The maximum amount of time between successive CWD updates.
///
/// The CWD will be updated at least this often, even if the updater receives no explicit signal to
/// perform an update.
const MAX_UPDATE_DELAY: Duration = Duration::from_secs(1);

/// Get an [`OwnedFd`] from a [`MasterPty`].
fn pty_parent_fd(parent: &dyn MasterPty) -> Option<OwnedFd> {
    let raw_fd = parent.as_raw_fd()?;
    // SAFETY: `parent` owns the underlying FD, and it cannot be dropped while this function runs
    // because we hold a reference to it.
    unsafe { BorrowedFd::borrow_raw(raw_fd) }.try_clone_to_owned().ok()
}

/// Get the CWD of the PTY proxy child.
///
/// This tries to obtain the CWD of the terminal's foreground process group, which is what programs
/// like tmux do. If that fails, we fall back to querying the CWD of the top-level PTY proxy child.
fn pty_cwd(parent: Option<BorrowedFd<'_>>, child: Option<Pid>) -> Option<PathBuf> {
    let parent = parent.and_then(tcgetpgrp);
    [parent, child].into_iter().flatten().find_map(process::cwd)
}

/// Get the foreground process group of a terminal.
///
/// We avoid using [`rustix::termios::tcgetpgrp`] because rustix has an upstream soundness bug that
/// causes undefined behavior on macOS. The underlying `tcgetpgrp` libc call returns 0 when called
/// on a PTY that no session has claimed, but on macOS, this gets passed directly to
/// `NonZero::new_unchecked`, which is UB.
///
/// TODO(taylordotfish): Remove this once <https://github.com/bytecodealliance/rustix/issues/1678>
/// is fixed.
fn tcgetpgrp(terminal: BorrowedFd<'_>) -> Option<Pid> {
    // SAFETY: The FD we pass is guaranteed to be valid because it came from a `BorrowedFd` that
    // exists at least for the life of the call.
    let pid = unsafe { libc::tcgetpgrp(terminal.as_raw_fd()) };
    if pid <= 0 {
        return None;
    }
    Pid::from_raw(pid)
}

/// Updates this process's CWD to match the PTY proxy child's CWD.
pub struct CwdUpdater {
    /// Used to tell the update thread to update the CWD now.
    tx: SyncSender<()>,
}

impl CwdUpdater {
    /// Start updating this process's CWD to match the PTY proxy child's.
    ///
    /// `master` is the PTY proxy's master PTY. `shell` is the PID of the PTY's child process, if
    /// available.
    ///
    /// Returns a [`CwdUpdater`] that can be used to signal the update thread to update the CWD now.
    pub fn new(master: &dyn MasterPty, child: Option<Pid>) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        let parent = pty_parent_fd(master);

        std::thread::spawn(move || {
            let mut cwd = None;
            let mut set_cwd = |new_cwd: PathBuf| {
                if cwd.as_deref().is_some_and(|p| p == new_cwd) {
                    return;
                }
                if std::env::set_current_dir(&new_cwd).is_ok() {
                    cwd = Some(new_cwd);
                }
            };

            let timeout_after_min_delay = MAX_UPDATE_DELAY - MIN_UPDATE_DELAY;
            let mut timeout = MAX_UPDATE_DELAY;
            loop {
                match rx.recv_timeout(timeout) {
                    Ok(()) => {}
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }

                loop {
                    if let Some(new_cwd) = pty_cwd(parent.as_ref().map(AsFd::as_fd), child) {
                        set_cwd(new_cwd);
                    }
                    std::thread::sleep(MIN_UPDATE_DELAY);
                    if rx.try_iter().count() == 0 {
                        timeout = timeout_after_min_delay;
                        break;
                    }
                }
            }
        });

        Self { tx }
    }

    /// Tell the updater to update this process's CWD to match that of the PTY proxy child.
    pub fn update(&self) {
        // A full channel isn't an error; it means the updater has already been signaled to run.
        let _ = self.tx.try_send(());
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::Path;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use portable_pty::{CommandBuilder, PtyPair, PtySize, native_pty_system};
    use rstest::rstest;

    use super::*;

    /// Wait for `read` to return `expected`, then report what it last returned.
    fn settles_on<T: PartialEq>(expected: &T, mut read: impl FnMut() -> T) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let found = read();
            if found == *expected || Instant::now() >= deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Open a pty that nothing has claimed as its controlling terminal.
    fn open_pty() -> PtyPair {
        native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to open pty")
    }

    /// Spawn `/bin/sh` on `pair`, in `cwd`, waiting for input.
    fn spawn_shell(pair: &PtyPair, cwd: &Path) -> Box<dyn portable_pty::Child + Send + Sync> {
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.cwd(cwd);
        pair.slave.spawn_command(cmd).expect("failed to spawn shell")
    }

    /// The pid of a spawned process, as [`pty_cwd`] takes it.
    fn pid_of(process_id: Option<u32>) -> Option<Pid> {
        process_id.and_then(|pid| Pid::from_raw(pid.cast_signed()))
    }

    /// A shell on a pty of its own, as the updater sees it.
    ///
    /// The descriptors kept here hold the pty open for as long as the fixture lives.
    struct Fixture {
        /// The pty the shell runs on, as the updater keeps it.
        parent: Option<OwnedFd>,
        /// The shell, as the updater knows it.
        child: Option<Pid>,
        /// The terminal, for typing at the shell.
        ///
        /// This is a descriptor of our own rather than portable-pty's writer, whose `Drop`
        /// writes a newline and an EOF into the terminal -- a blocking write that a test has
        /// no reason to make, and that would tell the shell to exit.
        input: File,
        shell: Box<dyn portable_pty::Child + Send + Sync>,
    }

    impl Fixture {
        /// Start a shell on a pty of its own, in `cwd`.
        fn new(cwd: &Path) -> Self {
            let pair = open_pty();
            let shell = spawn_shell(&pair, cwd);
            drop(pair.slave);

            // Nothing here reads the terminal, and a shell whose output has nowhere to go
            // stalls once the pty fills -- which takes far less output on some platforms than
            // on others. Throw it away as it arrives.
            let mut output = pair.master.try_clone_reader().expect("clone pty reader");
            std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                while output.read(&mut buf).is_ok_and(|read| read > 0) {}
            });

            Self {
                parent: pty_parent_fd(pair.master.as_ref()),
                child: pid_of(shell.process_id()),
                input: File::from(pty_parent_fd(pair.master.as_ref()).expect("pty descriptor")),
                shell,
            }
        }

        /// The directory the updater would move the proxy to.
        fn cwd(&self) -> Option<PathBuf> {
            pty_cwd(self.parent.as_ref().map(AsFd::as_fd), self.child)
        }

        /// Type a line at the shell.
        fn send_line(&mut self, line: &str) {
            writeln!(self.input, "{line}").expect("write to pty");
            self.input.flush().expect("flush pty");
        }

        /// Send an interrupt, as pressing ctrl-c would.
        fn interrupt(&mut self) {
            self.input.write_all(&[0x03]).expect("write to pty");
            self.input.flush().expect("flush pty");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.shell.kill();
            let _ = self.shell.wait();
        }
    }

    #[rstest]
    fn the_cwd_is_the_shells_while_it_is_in_the_foreground() {
        let dir = tempfile::tempdir().unwrap();
        // A working directory is never a symlink, but a temporary directory may be.
        let expected = dir.path().canonicalize().unwrap();
        let fixture = Fixture::new(dir.path());

        let found = settles_on(&Some(expected.clone()), || fixture.cwd());

        assert_eq!(found, Some(expected));
    }

    #[rstest]
    fn the_cwd_follows_the_shell_as_it_moves() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().canonicalize().unwrap().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let mut fixture = Fixture::new(dir.path());

        fixture.send_line(&format!("cd '{}'", elsewhere.display()));

        let found = settles_on(&Some(elsewhere.clone()), || fixture.cwd());

        assert_eq!(found, Some(elsewhere));
    }

    #[rstest]
    fn the_cwd_is_the_foreground_jobs_rather_than_the_shells() {
        // A multiplexer reports the directory of whatever is in the foreground, which is not the
        // shell while a job is running.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let elsewhere = home.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let mut fixture = Fixture::new(&home);

        // A foreground job of its own, in a directory the shell itself never enters.
        fixture.send_line(&format!("(cd '{}' && exec sleep 30)", elsewhere.display()));
        let found = settles_on(&Some(elsewhere.clone()), || fixture.cwd());

        // Interrupt the job rather than leave it running for half a minute.
        fixture.interrupt();
        assert_eq!(
            fixture.child.and_then(process::cwd),
            Some(home),
            "the shell itself never moved, so this can only have come from the foreground job"
        );
        assert_eq!(found, Some(elsewhere));
    }

    #[rstest]
    fn a_foreground_process_we_cannot_read_falls_back_to_the_child() {
        // `sudo` puts a process we have no business reading in the foreground, and a process
        // group outlives the leader that names it. Either way the child is the better answer.
        let dir = tempfile::tempdir().unwrap();
        let expected = dir.path().canonicalize().unwrap();
        // The shell starts somewhere other than the directory we expect back, so a reading
        // taken from it rather than from the fallback would be visible.
        let mut fixture = Fixture::new(Path::new("/"));
        let mut child = Command::new("sleep").arg("30").current_dir(dir.path()).spawn().unwrap();
        // Leave the terminal naming a foreground process group that no longer exists.
        fixture.shell.kill().unwrap();
        fixture.shell.wait().unwrap();

        let found = settles_on(&Some(expected.clone()), || {
            pty_cwd(fixture.parent.as_ref().map(AsFd::as_fd), pid_of(Some(child.id())))
        });

        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(found, Some(expected));
    }

    #[rstest]
    fn a_descriptor_that_is_not_a_terminal_has_no_foreground_group() {
        // `tcgetpgrp` answers -1 here. That is not a pid, and unlike the 0 a pty answers with,
        // `Pid::from_raw` asserts on it rather than returning `None`.
        let not_a_terminal = File::open("/dev/null").expect("open /dev/null");

        assert_eq!(tcgetpgrp(not_a_terminal.as_fd()), None);
    }

    #[rstest]
    fn nothing_readable_has_no_cwd() {
        // Nothing has claimed this pty, so it names no foreground process group, and there is no
        // child to fall back to.
        let pair = open_pty();
        let parent = pty_parent_fd(pair.master.as_ref());

        assert_eq!(pty_cwd(parent.as_ref().map(AsFd::as_fd), None), None);
        assert_eq!(pty_cwd(None, None), None);
    }

    #[rstest]
    fn the_copied_descriptor_outlives_the_pty_it_came_from() {
        // The updater keeps a copy of the descriptor precisely so that reading the terminal
        // doesn't mean holding on to the pty itself.
        let dir = tempfile::tempdir().unwrap();
        let expected = dir.path().canonicalize().unwrap();
        let pair = open_pty();
        let mut shell = spawn_shell(&pair, dir.path());
        drop(pair.slave);
        let parent = pty_parent_fd(pair.master.as_ref());
        drop(pair.master);

        let found =
            settles_on(&Some(expected.clone()), || pty_cwd(parent.as_ref().map(AsFd::as_fd), None));

        shell.kill().unwrap();
        shell.wait().unwrap();
        assert_eq!(found, Some(expected));
    }
}
