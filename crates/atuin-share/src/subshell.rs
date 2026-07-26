// The subshell is a complete, self-contained unit exercised by its own test,
// but its only in-crate consumer is the `session` module, which lands in a
// later task of this plan. Until then these are technically dead code and
// `cargo clippy -- -D warnings` (what CI runs) would reject the crate.
#![allow(dead_code)]

//! The child shell: a process attached to a PTY sized to the negotiated child
//! dimensions (the host's terminal minus the row reserved for the warning bar).

use std::io::{Read, Write};

use eyre::eyre;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::Size;

/// The shared subshell: a child process attached to a PTY that the session
/// loop reads from, writes to, and resizes.
///
/// Dropping a `Subshell` drops the PTY master, which sends the child `SIGHUP` —
/// relied upon during teardown.
pub struct Subshell {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

// `portable-pty` 0.9's `SlavePty::spawn_command` returns
// `Box<dyn Child + Send + Sync>`, so this bound matches exactly and no
// `Mutex` wrapper is needed around the child itself.

impl Subshell {
    /// Open a PTY sized to `size` and spawn the user's default shell in it.
    ///
    /// # Errors
    ///
    /// Returns an error if the PTY cannot be opened or the shell cannot be
    /// spawned.
    pub fn spawn(size: Size) -> eyre::Result<Self> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| eyre!("openpty: {e:#}"))?;

        let cmd = CommandBuilder::new_default_prog();
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| eyre!("spawn: {e:#}"))?;
        drop(pair.slave);

        Ok(Self {
            master: pair.master,
            child,
        })
    }

    /// A fresh reader over the PTY master. Every call clones the underlying
    /// descriptor, so this may be called more than once.
    ///
    /// # Panics
    ///
    /// Panics if the descriptor cannot be cloned, which would mean the process
    /// is out of file descriptors.
    #[must_use]
    pub fn reader(&self) -> Box<dyn Read + Send> {
        self.master.try_clone_reader().expect("clone pty reader")
    }

    /// The PTY master writer. **May only be called once** — the session takes
    /// it at startup and shares it behind an `Arc<Mutex<..>>`, because two
    /// paths write to the child: the host's own stdin, and inbound viewer
    /// `input`.
    ///
    /// # Panics
    ///
    /// Panics if the writer has already been taken.
    #[must_use]
    pub fn writer(&self) -> Box<dyn Write + Send> {
        self.master.take_writer().expect("take pty writer")
    }

    /// Resize the child PTY. Best-effort: a failed `TIOCSWINSZ` (e.g. the child
    /// already exited) is not worth tearing the session down for.
    pub fn resize(&self, size: Size) {
        let _ = self.master.resize(PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        });
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
        let sh = Subshell::spawn(Size { cols: 40, rows: 10 }).unwrap();
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
}
