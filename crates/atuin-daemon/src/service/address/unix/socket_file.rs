use std::io::ErrorKind;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use atuin_client::settings::Daemon as DaemonSettings;
use tokio::task::JoinHandle;

use super::DaemonAddress;

/// The socket file a daemon listens on, kept fresh against temporary file cleaners and, if the
/// daemon bound it itself, removed when dropped.
pub struct SocketFile {
    keepalive: JoinHandle<()>,
    /// The file to remove when the daemon exits, unless it outlives the daemon as launchd's does.
    remove_on_drop: Option<PathBuf>,
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
        socket.remove_on_drop = Some(address.0.clone());
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
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        self.keepalive.abort();
        let Some(path) = &self.remove_on_drop else {
            return;
        };
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
    use rstest::rstest;

    use super::*;

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
}
