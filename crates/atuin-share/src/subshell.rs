//! The child shell: a process attached to a PTY sized to the negotiated child
//! dimensions (the host's terminal minus the row reserved for the warning bar).

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::{Error, Size};

/// The shared subshell: a child process attached to a PTY.
///
/// Lives only between [`Subshell::spawn`] and [`Subshell::into_parts`]:
/// `run_share` spawns it, then the session splits it into the pieces its task
/// topology needs.
pub(crate) struct Subshell {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    child: Box<dyn Child + Send + Sync>,
}

// `portable-pty` 0.9's `SlavePty::spawn_command` returns
// `Box<dyn Child + Send + Sync>`, so this bound matches exactly and no
// `Mutex` wrapper is needed around the child itself.

/// Resizes the child PTY without owning the [`Subshell`].
///
/// `portable-pty` 0.9 declares `pub trait MasterPty: Downcast + Send` — it is
/// `Send` but **not** `Sync`, and offers no general `clone`. The master is
/// therefore held in an `Arc<Mutex<..>>`, which *is* `Send + Sync` (because
/// `Mutex<T>: Sync` when `T: Send`), so the session future holding the resizer
/// stays freely movable across the runtime's worker threads.
///
/// After [`Subshell::into_parts`] the resizer holds the boxed master itself,
/// but it is **not** the only master fd: the reader and writer split off there
/// are dups of it (`portable-pty` 0.9 implements `try_clone_reader` /
/// `take_writer` as fd clones), so dropping the resizer alone does not close
/// the master side. Nothing relies on it doing so — the reader's EOF comes
/// from the *slave* side, once the child and every descendant that inherited
/// the tty have exited.
#[derive(Clone)]
pub(crate) struct PtyResizer(Arc<Mutex<Box<dyn MasterPty + Send>>>);

impl PtyResizer {
    /// Resize the child PTY. Best-effort: a failed `TIOCSWINSZ` (e.g. the
    /// child already exited) is not worth tearing the session down for.
    pub(crate) fn resize(&self, size: Size) {
        if let Ok(master) = self.0.lock() {
            let _ = master.resize(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }
}

/// The session-facing pieces of a spawned subshell, split apart so each can go
/// where the session's topology needs it: the reader to the pty-reader thread,
/// the child to the blocking-pool wait, and the writer, resizer and killer to
/// the central task.
pub(crate) struct SubshellParts {
    /// A blocking reader over the PTY master (child output).
    pub(crate) reader: Box<dyn Read + Send>,
    /// The sole PTY writer (child input).
    pub(crate) writer: Box<dyn Write + Send>,
    /// Child PTY resizer — holds the boxed master after the split (though the
    /// reader and writer are fd dups of it); see [`PtyResizer`].
    pub(crate) resizer: PtyResizer,
    /// Terminates the child without owning it, so the session can kill while
    /// `child.wait()` runs on the blocking pool.
    pub(crate) killer: Box<dyn ChildKiller + Send + Sync>,
    /// The child itself, kept only to be `wait()`ed on for its exit code.
    pub(crate) child: Box<dyn Child + Send + Sync>,
}

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
    pub(crate) fn spawn(size: Size, env: &[(&str, &str)]) -> crate::Result<Self> {
        let pty = native_pty_system();
        // `portable-pty` errors are `anyhow::Error`; stringify with `{:#}` (the
        // cause chain) so the typed variants display exactly what `eyre` did.
        let pair = pty
            .openpty(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::OpenPty(format!("{e:#}")))?;

        let mut cmd = CommandBuilder::new_default_prog();
        for (key, value) in env {
            cmd.env(key, value);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Error::SpawnShell(format!("{e:#}")))?;
        drop(pair.slave);

        Ok(Self {
            master: Arc::new(Mutex::new(pair.master)),
            child,
        })
    }

    /// Split the subshell into the pieces the session's topology needs — see
    /// [`SubshellParts`]. The boxed PTY master survives inside the resizer;
    /// the reader and writer carry their own dups of the master fd.
    ///
    /// # Panics
    ///
    /// Panics if the reader cannot be cloned (the process is out of file
    /// descriptors) or the writer was already taken — impossible on a freshly
    /// spawned subshell, which is the only caller.
    #[must_use]
    pub(crate) fn into_parts(self) -> SubshellParts {
        let (reader, writer) = {
            let master = self.master.lock().expect("master lock");
            (
                master.try_clone_reader().expect("clone pty reader"),
                master.take_writer().expect("take pty writer"),
            )
        };
        SubshellParts {
            reader,
            writer,
            resizer: PtyResizer(self.master),
            killer: self.child.clone_killer(),
            child: self.child,
        }
    }
}
