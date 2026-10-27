use std::io::ErrorKind;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use atuin_client::settings::Daemon as DaemonSettings;
use atuin_common::os::fs::FdIdentity;
use tokio::task::JoinHandle;

use super::DaemonAddress;

/// The socket file a daemon listens on, kept fresh against temporary file cleaners and, if the
/// daemon bound it itself, removed when dropped, unless another socket has taken its path since.
pub struct SocketFile {
    keepalive: JoinHandle<()>,
    /// The file to remove when the daemon exits, unless it outlives the daemon as launchd's does.
    remove_on_drop: Option<Bound>,
}

/// A socket file a daemon bound.
struct Bound {
    path: PathBuf,
    /// Tells the file apart from a socket put at its path since, as launchd's agent's is at login.
    identity: FdIdentity,
}

/// An error that occurred while trying to remove a socket.
#[derive(Debug, thiserror::Error)]
#[error("failed to remove daemon socket {}: {source}", .path.display())]
pub struct RemoveSocketError {
    path: PathBuf,
    source: std::io::Error,
}

impl SocketFile {
    /// How often to update the socket's modification time so it doesn't get automatically deleted
    /// by temporary file cleaners.
    const KEEPALIVE_INTERVAL: Duration = Duration::from_mins(1);

    /// The socket the daemon bound at `address`, which it owns until it exits, kept fresh from the
    /// current Tokio runtime.
    #[expect(clippy::unnecessary_wraps, reason = "a daemon listening on TCP has no socket file")]
    pub(crate) fn of(address: &DaemonAddress) -> Option<Self> {
        let mut socket = Self::kept_fresh(address.0.clone());
        socket.remove_on_drop = Self::identity(&address.0).map(|identity| Bound {
            path: address.0.clone(),
            identity,
        });
        Some(socket)
    }

    /// Keep fresh the socket at `path`, which goes on listening once the daemon exits, as
    /// launchd's does.
    pub(crate) fn kept_fresh(path: PathBuf) -> Self {
        Self {
            keepalive: Self::keep_fresh(path),
            remove_on_drop: None,
        }
    }

    fn keep_fresh(path: PathBuf) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Self::KEEPALIVE_INTERVAL);
            loop {
                interval.tick().await;
                if let Err(e) = atuin_common::os::unix::touch_file(&path) {
                    tracing::warn!("failed to refresh {}: {e}", path.display());
                }
            }
        })
    }

    /// Remove the socket of the daemon `daemon` describes from every path it may be at.
    pub(crate) fn remove_all(daemon: &DaemonSettings) -> Result<(), RemoveSocketError> {
        Self::first_error(DaemonAddress::candidates(daemon).map(|path| Self::remove(&path)))
    }

    /// Remove the sockets that a daemon `daemon` describes left behind when it stopped listening.
    pub(super) fn remove_stale(daemon: &DaemonSettings) -> Result<(), RemoveSocketError> {
        Self::first_error(
            DaemonAddress::candidates(daemon).map(|path| Self::remove_if_stale(&path)),
        )
    }

    /// Remove the socket at `path` if nothing listens on it any more.
    pub(crate) fn remove_if_stale(path: &Path) -> Result<(), RemoveSocketError> {
        if !Self::is_stale(path) {
            return Ok(());
        }
        Self::remove(path)
    }

    fn remove(path: &Path) -> Result<(), RemoveSocketError> {
        match std::fs::remove_file(path) {
            Err(source) if source.kind() != ErrorKind::NotFound => Err(RemoveSocketError {
                path: path.to_owned(),
                source,
            }),
            _ => Ok(()),
        }
    }

    /// The first of `removals` that failed, after logging each, since only one is returned.
    fn first_error(
        removals: impl Iterator<Item = Result<(), RemoveSocketError>>,
    ) -> Result<(), RemoveSocketError> {
        let mut first = None;
        for err in removals.filter_map(Result::err) {
            tracing::error!("{err}");
            first.get_or_insert(err);
        }
        first.map_or(Ok(()), Err)
    }

    /// Whether the file at `path` is a socket that nothing listens on.
    fn is_stale(path: &Path) -> bool {
        matches!(atuin_common::os::unix::socket_in_use(path), Ok(false))
            // Ensure file is a socket; `socket_in_use` can return `Ok(false)` for regular files.
            && std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_socket())
    }

    /// The identity of the file at `path`, if there is one.
    fn identity(path: &Path) -> Option<FdIdentity> {
        std::fs::symlink_metadata(path).ok().as_ref().map(FdIdentity::from_metadata)
    }
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        self.keepalive.abort();
        let Some(Bound { path, identity }) = &self.remove_on_drop else {
            return;
        };
        // launchd puts its agent's socket at the path at login, even over a daemon listening there,
        // and that one is the agent's to keep. Doing so between this check and the removal, it
        // would still lose it, but launchd only does as the user logs in.
        if Self::identity(path) != Some(*identity) {
            tracing::info!("leaving {}, which is no longer this daemon's socket", path.display());
            return;
        }
        tracing::info!("removing socket {}", path.display());
        if let Err(e) = std::fs::remove_file(path)
            && e.kind() != ErrorKind::NotFound
        {
            tracing::warn!("failed to remove socket {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use rstest::{fixture, rstest};

    use super::*;

    struct Socket {
        _dir: tempfile::TempDir,
        address: DaemonAddress,
    }

    #[fixture]
    fn socket() -> Socket {
        let dir = tempfile::tempdir().unwrap();
        let address = DaemonAddress::from(dir.path().join("atuin.sock"));
        Socket { _dir: dir, address }
    }

    /// A socket nothing listens on is removed; one a daemon listens on, and a file that is no
    /// socket (a `socket_path` set wrong), are not.
    #[rstest]
    #[case::stale(false, true, false)]
    #[case::listening(true, true, true)]
    #[case::not_a_socket(false, false, true)]
    fn only_a_stale_socket_is_removed(
        #[case] listening: bool,
        #[case] socket: bool,
        #[case] kept: bool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("atuin.sock");
        let listener = socket.then(|| std::os::unix::net::UnixListener::bind(&path).unwrap());
        if !socket {
            std::fs::write(&path, "not a socket").unwrap();
        }
        if !listening {
            drop(listener);
        }
        SocketFile::remove_if_stale(&path).unwrap();
        assert_eq!(path.exists(), kept);
    }

    /// launchd puts its agent's socket at the path of a daemon that still runs, and the daemon
    /// mustn't take launchd's socket with it as it exits.
    #[rstest]
    #[case::its_own(false)]
    #[case::replaced_by_launchd(true)]
    #[tokio::test]
    async fn a_daemon_removes_only_its_own_socket(socket: Socket, #[case] replaced: bool) {
        let path = socket.address.as_ref();
        let _bound = socket.address.bind().unwrap();
        let file = SocketFile::of(&socket.address).unwrap();
        let launchds = replaced.then(|| {
            std::fs::remove_file(path).unwrap();
            UnixListener::bind(path).unwrap()
        });

        drop(file);

        assert_eq!(path.exists(), launchds.is_some());
    }
}
