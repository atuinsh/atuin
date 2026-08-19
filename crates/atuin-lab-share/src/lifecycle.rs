//! Daemon lifecycle plumbing for the backgrounded `atuin lab share --active`
//! session: the pidfile whose **exclusive lock is the aliveness signal**, the
//! URL file `--url` reprints, and the pre-runtime daemonize step.
//!
//! Two kinds of process coordinate through these files and nothing else:
//!
//! * short-lived foreground CLIs (the spawning parent, `--stop`, `--url`)
//!   only ever *probe* the lock ([`probe_lock`]) and read the files;
//! * the daemonized child *holds* the lock for the session's lifetime
//!   ([`PidfileGuard`]) and is the only writer of the URL file.
//!
//! The lock — not the file's existence — is the truth: a pidfile left behind
//! by a killed session has a free lock and is reclaimed silently, matching
//! the atuin daemon's pidfile semantics (`command/client/daemon.rs`, the
//! in-tree precedent this module follows; its helpers are compiled out of
//! `client`-only builds, which is why lab share carries its own). One
//! consequence, documented at [`PidfileGuard::acquire`]: at most one active
//! share per user.
//!
//! # Manual smoke test
//!
//! Daemonizing forks and detaches, so it can never run under `cargo test`
//! (the in-tree precedent, `atuin daemon start --daemonize`, is under the
//! same constraint). The full flow is smoke-tested by hand:
//!
//! ```text
//! $ eval "$(atuin pty-proxy init zsh)"   # a shell owned by a pty-proxy
//! $ atuin lab share --active             # prints the URL, prompt returns
//! $ atuin lab share --url                # reprints the URL
//! $ atuin lab share --stop               # "sharing stopped."
//! ```

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs4::fs_std::FileExt as _;

use crate::{Error, Result};

/// The environment variable through which the spawning parent hands its
/// child the spawn id recorded in the URL file's owner line — see
/// [`write_url_file`]. The parent only accepts a URL file carrying the id it
/// minted, so a stale file, or a URL written by a concurrently racing share,
/// can never be reported as this launch's success.
pub const SPAWN_ID_ENV: &str = "ATUIN_LAB_SHARE_SPAWN_ID";

/// Where the daemonized child's pidfile lives: the data dir, following the
/// daemon's convention (`daemon.pidfile_path` defaults there too).
pub fn pidfile_path() -> PathBuf {
    atuin_common::utils::data_dir().join("lab-share.pid")
}

/// Where the join URL is persisted for `--url`. The URL is secret-bearing —
/// its fragment carries the E2EE key — and session-scoped, so it belongs in
/// `$XDG_RUNTIME_DIR` (a per-user 0700 tmpfs) where the platform provides
/// one. Where it does not — macOS never sets the variable — the file goes
/// into a `private` subdirectory of the data dir instead of the data dir
/// itself: [`write_url_file`] creates that directory 0700, so the
/// private-directory half of the requirement holds on every platform even
/// though the fallback is persistent disk (the file itself is 0600 from
/// birth and removed on teardown, `--stop`, and the next launch's
/// lock-guarded cleanup).
pub fn url_file_path() -> PathBuf {
    url_file_path_from(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
}

/// [`url_file_path`] with the runtime dir injected, for tests.
fn url_file_path_from(runtime_dir: Option<PathBuf>) -> PathBuf {
    match runtime_dir {
        Some(dir) => dir.join("lab-share.url"),
        None => atuin_common::utils::data_dir().join("private").join("lab-share.url"),
    }
}

/// What a momentary look at the pidfile lock found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    /// Nobody holds the lock: no share is running (any pidfile is stale).
    Free,
    /// A live process holds the lock: a share is running.
    Held,
}

/// The daemonized child's claim on being *the* active share: an exclusive
/// lock on the pidfile, held for the session's lifetime and released on drop
/// — or by the OS when the process dies, which is exactly what makes the
/// lock a truthful aliveness signal where the file's existence is not.
#[derive(Debug)]
pub struct PidfileGuard {
    file: File,
}

impl PidfileGuard {
    /// How often and how long [`Self::acquire`] retries a busy lock: long
    /// enough to absorb a foreground CLI's momentary [`probe_lock`] (which
    /// holds the lock for microseconds), short enough that colliding with a
    /// genuinely running share still fails promptly.
    const ATTEMPTS: u32 = 5;
    /// Delay between those attempts.
    const RETRY: Duration = Duration::from_millis(100);

    /// Take the pidfile lock, truncate the file, and record our pid on the
    /// first line (`--stop` reads it to aim its SIGTERM).
    ///
    /// A stale pidfile — the file exists but its lock is free because the
    /// previous session died without cleanup — is reclaimed silently. A held
    /// lock means a share is genuinely running: one active share per user.
    ///
    /// # Errors
    ///
    /// [`Error::ShareAlreadyRunning`] when the lock stays busy through the
    /// retries; [`Error::Io`] when the file cannot be created or written.
    pub async fn acquire(path: &Path) -> Result<Self> {
        Self::acquire_with(path, Self::ATTEMPTS, Self::RETRY).await
    }

    /// [`Self::acquire`] with the retry schedule injected, for tests.
    ///
    /// # Errors
    ///
    /// See [`Self::acquire`].
    pub async fn acquire_with(path: &Path, attempts: u32, retry: Duration) -> Result<Self> {
        let file = open_pidfile(path)?;
        let mut remaining = attempts;
        while !file.try_lock_exclusive()? {
            if remaining == 0 {
                return Err(Error::ShareAlreadyRunning);
            }
            remaining -= 1;
            tokio::time::sleep(retry).await;
        }
        // Truncate only AFTER the lock is won: truncating first would wipe a
        // running share's pid out from under `--stop`.
        file.set_len(0)?;
        writeln!(&file, "{}", std::process::id())?;
        Ok(Self { file })
    }
}

impl Drop for PidfileGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Open the pidfile (creating it 0600 if needed) without truncating —
/// truncation is [`PidfileGuard`]'s job, after the lock is won.
fn open_pidfile(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(path)
}

/// Probe the pidfile lock without disturbing a holder.
///
/// Momentarily takes (then immediately releases) the exclusive lock when it
/// is free — which is why [`PidfileGuard::acquire`] retries instead of
/// treating one busy attempt as fatal: a concurrent probe must never bounce
/// a starting share.
///
/// # Errors
///
/// Only real I/O errors: a missing pidfile is [`LockState::Free`], not an
/// error.
pub fn probe_lock(path: &Path) -> std::io::Result<LockState> {
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(LockState::Free),
        Err(err) => return Err(err),
    };
    if file.try_lock_exclusive()? {
        file.unlock()?;
        Ok(LockState::Free)
    } else {
        Ok(LockState::Held)
    }
}

/// The pid recorded on the pidfile's first line, or `None` when the file is
/// missing or garbled. Only meaningful while [`probe_lock`] says
/// [`LockState::Held`].
pub fn read_pidfile_pid(path: &Path) -> Option<u32> {
    let contents = fs::read_to_string(path).ok()?;
    contents.lines().next()?.trim().parse().ok()
}

/// Wait (polling) until the pidfile lock is free — the share process exited
/// — or `timeout` elapses. Returns whether the lock was released. `--stop`
/// uses this after its SIGTERM; probe errors count as "still held" so a
/// transient failure cannot fake a clean stop.
pub async fn wait_for_lock_release(path: &Path, timeout: Duration) -> bool {
    const POLL: Duration = Duration::from_millis(100);
    let start = Instant::now();
    loop {
        if matches!(probe_lock(path), Ok(LockState::Free)) {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Persist (or re-persist) the join URL: written 0600 from creation to a tmp
/// sibling, then renamed over `path`, so a concurrent `--url` or the spawning
/// parent's startup poll reads either the old complete URL or the new one,
/// never a torn write. Rewriting after a reconnect mints a fresh session is
/// the same call. Any missing parent directories are created 0700 — the URL
/// is secret-bearing, and on platforms without `$XDG_RUNTIME_DIR` the file
/// lives in a `private` subdirectory this call mints (see [`url_file_path`]).
///
/// File format: the URL on the first line; when `owner` is given (the spawn
/// id from [`SPAWN_ID_ENV`]), it follows on the second. [`read_url_file`]
/// reads only the URL; [`read_url_file_owner`] reads only the owner, letting
/// the spawning parent verify the file was written by the child it spawned.
///
/// # Errors
///
/// I/O errors creating, writing, or renaming. Callers treat this as
/// best-effort: a failed write must not kill a healthy session.
pub fn write_url_file(path: &Path, url: &str, owner: Option<&str>) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::other("url file path has no parent directory"));
    };
    fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    {
        let mut file =
            OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        file.write_all(url.as_bytes())?;
        file.write_all(b"\n")?;
        if let Some(owner) = owner {
            file.write_all(owner.as_bytes())?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// The persisted join URL — the file's first line, trimmed — or `None` when
/// the file is missing or empty (the share is still connecting, or none is
/// running).
pub fn read_url_file(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    let url = contents.lines().next().unwrap_or_default().trim();
    if url.is_empty() {
        None
    } else {
        Some(url.to_string())
    }
}

/// The owner (spawn id) recorded on the URL file's second line, or `None`
/// when the file is missing or carries no owner. See [`write_url_file`].
pub fn read_url_file_owner(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    let owner = contents.lines().nth(1).unwrap_or_default().trim();
    if owner.is_empty() {
        None
    } else {
        Some(owner.to_string())
    }
}

/// Remove the URL file, best-effort: session teardown, `--stop`, and the
/// spawning parent's stale-file cleanup all call this, and a missing file is
/// success.
pub fn remove_url_file(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Daemonize the current process (fork, setsid, stdio to `/dev/null` via the
/// `daemonize` crate), the daemon's `daemonize_current_process` precedent
/// verbatim.
///
/// # Invariant — fork before the runtime
///
/// This must run **before the tokio runtime is built**: `fork()` inside a
/// live runtime corrupts its internal state. The only caller is the CLI's
/// pre-runtime dispatch for the hidden `--internal-daemon` flag; never call
/// it from async code.
///
/// # Errors
///
/// [`Error::Daemonize`] when the fork/detach fails; [`Error::Io`] when the
/// current working directory cannot be read.
pub fn daemonize_current_process() -> Result<()> {
    let cwd = std::env::current_dir()?;
    daemonize::Daemonize::new()
        .working_directory(cwd)
        .start()
        .map_err(|err| Error::Daemonize(err.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    /// A retry delay short enough that the deliberately-busy tests stay fast.
    const FAST: Duration = Duration::from_millis(1);

    /// The free -> held -> free arc: acquiring locks and records our pid,
    /// dropping releases.
    #[tokio::test]
    async fn pidfile_free_then_held_then_free() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lab-share.pid");

        assert_eq!(probe_lock(&path).unwrap(), LockState::Free, "no file yet");

        let guard = PidfileGuard::acquire_with(&path, 0, FAST).await.unwrap();
        assert_eq!(probe_lock(&path).unwrap(), LockState::Held);
        assert_eq!(read_pidfile_pid(&path), Some(std::process::id()));

        drop(guard);
        assert_eq!(probe_lock(&path).unwrap(), LockState::Free);
    }

    /// Busy lock => `ShareAlreadyRunning`, with the pinned user-visible copy.
    #[tokio::test]
    async fn pidfile_busy_is_share_already_running() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lab-share.pid");

        let _held = PidfileGuard::acquire_with(&path, 0, FAST).await.unwrap();
        let err = PidfileGuard::acquire_with(&path, 2, FAST)
            .await
            .expect_err("second acquire must fail while the lock is held");
        assert!(matches!(err, Error::ShareAlreadyRunning));
        assert_eq!(err.to_string(), "an active share is already running");
    }

    /// A stale pidfile (file exists, lock free) is reclaimed: the old pid is
    /// replaced by ours, no error, no leftover second line.
    #[tokio::test]
    async fn pidfile_stale_is_reclaimed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lab-share.pid");
        fs::write(&path, "424242\nleftover junk\n").unwrap();

        assert_eq!(probe_lock(&path).unwrap(), LockState::Free, "stale = free");
        let _guard = PidfileGuard::acquire_with(&path, 0, FAST).await.unwrap();
        assert_eq!(read_pidfile_pid(&path), Some(std::process::id()));
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 1);
    }

    #[tokio::test]
    async fn wait_for_lock_release_observes_the_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lab-share.pid");

        let guard = PidfileGuard::acquire_with(&path, 0, FAST).await.unwrap();
        assert!(
            !wait_for_lock_release(&path, Duration::from_millis(50)).await,
            "held lock must time out"
        );
        drop(guard);
        assert!(wait_for_lock_release(&path, Duration::from_secs(5)).await);
    }

    /// Write, rewrite (a reconnect's fresh link), read, and cleanup — and the
    /// file is 0600 because the URL's fragment carries the session key.
    #[test]
    fn url_file_write_rewrite_read_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lab-share.url");

        assert_eq!(read_url_file(&path), None, "missing file reads None");

        write_url_file(&path, "https://hub.example/s/one#key1", None).unwrap();
        assert_eq!(read_url_file(&path).as_deref(), Some("https://hub.example/s/one#key1"));
        assert_eq!(read_url_file_owner(&path), None, "no owner recorded");
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "URL file must be private");

        write_url_file(&path, "https://hub.example/s/two#key2", None).unwrap();
        assert_eq!(
            read_url_file(&path).as_deref(),
            Some("https://hub.example/s/two#key2"),
            "rewrite replaces the URL"
        );

        remove_url_file(&path);
        assert_eq!(read_url_file(&path), None);
        remove_url_file(&path); // second removal is still success
    }

    /// The owner (spawn id) rides the second line: `--url` still reads the
    /// bare URL, and the spawning parent reads the owner to verify the file
    /// was written by the child IT spawned.
    #[test]
    fn url_file_owner_round_trips_without_polluting_the_url() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lab-share.url");

        write_url_file(&path, "https://hub.example/s/one#key1", Some("spawn-42")).unwrap();
        assert_eq!(
            read_url_file(&path).as_deref(),
            Some("https://hub.example/s/one#key1"),
            "the owner line must never leak into the URL"
        );
        assert_eq!(read_url_file_owner(&path).as_deref(), Some("spawn-42"));
    }

    /// The secret-bearing file's parent directory is minted 0700 — the
    /// private-directory half of the requirement on platforms where the
    /// fallback lands outside `$XDG_RUNTIME_DIR`.
    #[test]
    fn url_file_parent_directory_is_created_private() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("private").join("lab-share.url");

        write_url_file(&path, "https://hub.example/s/one#key1", None).unwrap();
        let mode = fs::metadata(tmp.path().join("private")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "URL directory must be private");
    }

    /// Without `$XDG_RUNTIME_DIR` (macOS always; some Linux setups) the URL
    /// file must land in the 0700 `private` subdirectory of the data dir,
    /// never the shared data dir itself.
    #[test]
    fn url_file_path_falls_back_to_a_private_subdirectory() {
        let with_runtime = url_file_path_from(Some(PathBuf::from("/run/user/1000")));
        assert_eq!(with_runtime, PathBuf::from("/run/user/1000/lab-share.url"));

        let fallback = url_file_path_from(None);
        assert_eq!(fallback.file_name().unwrap(), "lab-share.url");
        assert_eq!(
            fallback.parent().unwrap().file_name().unwrap(),
            "private",
            "the data-dir fallback must use the private subdirectory"
        );
    }

    #[test]
    fn url_file_empty_reads_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lab-share.url");
        fs::write(&path, "\n").unwrap();
        assert_eq!(read_url_file(&path), None);
        assert_eq!(read_url_file_owner(&path), None);
    }

    #[test]
    fn pidfile_pid_parses_first_line_only() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lab-share.pid");

        assert_eq!(read_pidfile_pid(&path), None, "missing file");
        fs::write(&path, "not a pid\n").unwrap();
        assert_eq!(read_pidfile_pid(&path), None, "garbled file");
        fs::write(&path, "12345\nanything\n").unwrap();
        assert_eq!(read_pidfile_pid(&path), Some(12345));
    }

    /// The file names are part of the CLI contract (`--stop`/`--url` and the
    /// session must agree across versions); the directories come from
    /// `atuin_common` and are env-dependent, so pin only the names.
    #[test]
    fn coordination_file_names_are_stable() {
        assert_eq!(pidfile_path().file_name().unwrap(), "lab-share.pid");
        assert_eq!(url_file_path().file_name().unwrap(), "lab-share.url");
    }
}
