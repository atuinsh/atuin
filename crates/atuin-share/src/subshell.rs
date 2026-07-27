//! The child shell: a process attached to a PTY sized to the negotiated child
//! dimensions (the host's terminal minus the row reserved for the warning bar).

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use eyre::eyre;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::Size;

/// The shared subshell: a child process attached to a PTY that the session
/// loop reads from, writes to, and resizes.
///
/// Dropping a `Subshell` drops the PTY master, which sends the child `SIGHUP` —
/// relied upon during teardown.
///
/// `portable-pty` 0.9 declares `pub trait MasterPty: Downcast + Send` — it is
/// `Send` but **not** `Sync`, and offers no general `clone`. The master is
/// therefore held in an `Arc<Mutex<..>>`, which *is* `Send + Sync` (because
/// `Mutex<T>: Sync` when `T: Send`) and can be shared with the SIGWINCH and
/// inbound-event threads.
pub struct Subshell {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

// `portable-pty` 0.9's `SlavePty::spawn_command` returns
// `Box<dyn Child + Send + Sync>`, so this bound matches exactly and no
// `Mutex` wrapper is needed around the child itself.

/// Resizes the child PTY. Cloneable and thread-safe, so the SIGWINCH handler and
/// the inbound-event thread can both resize without owning the `Subshell`.
pub type ResizeHandle = Arc<dyn Fn(Size) + Send + Sync>;

impl Subshell {
    /// Open a PTY sized to `size` and spawn the user's default shell in it.
    ///
    /// # Errors
    ///
    /// Returns an error if the PTY cannot be opened or the shell cannot be
    /// spawned.
    ///
    /// `env` entries are set in the child's environment. `portable-pty` seeds
    /// the builder from the parent environment, so these are layered on top —
    /// `PATH`, `HOME`, etc. are preserved. Used to expose `ATUIN_SHARE_URL` (the
    /// join link) inside the shared shell so it can be retrieved after the
    /// printed link has scrolled away.
    pub fn spawn(size: Size, env: &[(&str, &str)]) -> eyre::Result<Self> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| eyre!("openpty: {e:#}"))?;

        let mut cmd = CommandBuilder::new_default_prog();
        for (key, value) in env {
            cmd.env(key, value);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| eyre!("spawn: {e:#}"))?;
        drop(pair.slave);

        Ok(Self {
            master: Arc::new(Mutex::new(pair.master)),
            child,
        })
    }

    /// A fresh reader over the PTY master. Every call clones the underlying
    /// descriptor, so this may be called more than once.
    ///
    /// # Panics
    ///
    /// Panics if the descriptor cannot be cloned, which would mean the process
    /// is out of file descriptors, or if the master lock is poisoned.
    #[must_use]
    pub fn reader(&self) -> Box<dyn Read + Send> {
        self.master
            .lock()
            .expect("master lock")
            .try_clone_reader()
            .expect("clone pty reader")
    }

    /// The PTY master writer. **May only be called once** — the session takes
    /// it at startup and shares it behind an `Arc<Mutex<..>>`, because two
    /// paths write to the child: the host's own stdin, and inbound viewer
    /// `input`.
    ///
    /// # Panics
    ///
    /// Panics if the writer has already been taken, or if the master lock is
    /// poisoned.
    #[must_use]
    pub fn writer(&self) -> Box<dyn Write + Send> {
        self.master
            .lock()
            .expect("master lock")
            .take_writer()
            .expect("take pty writer")
    }

    /// Resize the child PTY. Best-effort: a failed `TIOCSWINSZ` (e.g. the child
    /// already exited) is not worth tearing the session down for.
    ///
    /// Part of the crate's declared `Subshell` surface. The session's threads
    /// do not own the `Subshell`, so they resize through [`Self::resize_handle`]
    /// instead — leaving this owned-receiver convenience with no in-crate caller.
    #[allow(
        dead_code,
        reason = "declared Subshell surface; threads use resize_handle() instead"
    )]
    pub fn resize(&self, size: Size) {
        (self.resize_handle())(size);
    }

    /// A cloneable resize closure, so threads can resize without owning the PTY.
    #[must_use]
    pub fn resize_handle(&self) -> ResizeHandle {
        let master = Arc::clone(&self.master);
        Arc::new(move |size: Size| {
            if let Ok(m) = master.lock() {
                let _ = m.resize(PtySize {
                    rows: size.rows,
                    cols: size.cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
        })
    }

    /// Non-blocking exit check, so the run loop can also watch the kill switch.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `waitpid` fails.
    pub fn try_wait(&mut self) -> eyre::Result<Option<i32>> {
        match self.child.try_wait() {
            Ok(Some(status)) => Ok(Some(i32::try_from(status.exit_code()).unwrap_or(1))),
            Ok(None) => Ok(None),
            Err(e) => Err(eyre!("try_wait: {e:#}")),
        }
    }

    /// Terminate the child (kill switch / teardown). Best-effort: the child may
    /// already have exited, in which case the error is not interesting.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// Block until the child exits and return its exit code.
    ///
    /// # Errors
    ///
    /// Returns an error if waiting on the child fails.
    pub fn wait(&mut self) -> eyre::Result<i32> {
        let status = self.child.wait().map_err(|e| eyre!("wait: {e:#}"))?;
        Ok(i32::try_from(status.exit_code()).unwrap_or(1))
    }
}

#[cfg(all(test, unix))]
// `std::env::set_var` is `unsafe` in edition 2024, and the crate denies
// `unsafe_code`; the test below is single-threaded so the call is sound.
#[allow(unsafe_code)]
mod tests {
    use std::io::Read;

    use super::*;
    use crate::Size;

    #[test]
    fn spawns_shell_and_reads_output() {
        // Force a known program via $SHELL so the test is deterministic.
        // SAFETY: single-threaded test; std::env::set_var is fine here.
        unsafe { std::env::set_var("SHELL", "/bin/sh") };
        let sh = Subshell::spawn(Size { cols: 40, rows: 10 }, &[]).unwrap();
        {
            use std::io::Write;
            sh.writer().write_all(b"printf ATUINOK\n").unwrap();
        }
        let mut reader = sh.reader();
        let mut buf = [0u8; 4096];
        let mut seen = String::new();
        for _ in 0..50 {
            if let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.push_str(&String::from_utf8_lossy(&buf[..n]));
                if seen.contains("ATUINOK") {
                    break;
                }
            }
        }
        assert!(
            seen.contains("ATUINOK"),
            "did not observe command output: {seen:?}"
        );
    }

    #[test]
    fn spawn_sets_env_and_preserves_inherited_env() {
        // `new_default_prog` spawns the login shell from the passwd database
        // (not $SHELL), so we can't force a specific shell here. Instead we run
        // the probe then `exit` and read to EOF, which is deterministic for any
        // POSIX shell regardless of prompt/line-editing noise. The expanded
        // `U=...` output is emitted by `printf` on its own line, so it is not
        // corrupted by input-echo line wrapping.
        //
        // SAFETY: single-threaded test; std::env::set_var is fine here.
        // A var set in the PARENT the child must still inherit.
        unsafe { std::env::set_var("ATUIN_SHARE_TEST_INHERIT", "yes") };

        let sh = Subshell::spawn(
            Size { cols: 80, rows: 24 },
            &[("ATUIN_SHARE_URL", "http://h/lab/share/tok")],
        )
        .unwrap();
        {
            use std::io::Write;
            sh.writer()
                .write_all(
                    b"printf '\\nU=%s I=%s\\n' \"$ATUIN_SHARE_URL\" \"$ATUIN_SHARE_TEST_INHERIT\"; exit\n",
                )
                .unwrap();
        }
        let mut reader = sh.reader();
        let mut buf = [0u8; 4096];
        let mut seen = String::new();
        // Bounded so a misbehaving shell fails the test instead of hanging;
        // normally the loop ends at EOF/EIO once the child has exited.
        for _ in 0..1000 {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    seen.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if seen.contains("U=http://h/lab/share/tok") && seen.contains("I=yes") {
                        break;
                    }
                }
                // EIO is the normal way a PTY master reports the child exiting.
                Err(_) => break,
            }
        }
        assert!(
            seen.contains("U=http://h/lab/share/tok"),
            "ATUIN_SHARE_URL was not set in the child: {seen:?}"
        );
        assert!(
            seen.contains("I=yes"),
            "child did not inherit the parent environment: {seen:?}"
        );
    }
}
