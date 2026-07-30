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
