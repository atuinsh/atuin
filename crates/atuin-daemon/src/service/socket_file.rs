use std::io::ErrorKind;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::task::JoinHandle;

/// The socket file a daemon listens on, kept fresh against temporary file cleaners and, if the
/// daemon bound it itself, removed when dropped.
pub(super) struct SocketFile {
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

    /// Own the socket bound at `path`, keeping it fresh from the current Tokio runtime.
    pub(super) fn new(path: PathBuf) -> Self {
        let mut socket = Self::kept_fresh(path.clone());
        socket.remove_on_drop = Some(path);
        socket
    }

    /// Keep fresh the socket at `path`, which goes on listening once the daemon exits, as
    /// launchd's does.
    pub(super) fn kept_fresh(path: PathBuf) -> Self {
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

    /// Remove the daemon's socket from each of `paths` it is at, subject to `should_remove`.
    pub(super) fn remove_all(
        paths: impl IntoIterator<Item = impl AsRef<Path>>,
        should_remove: impl Fn(&Path) -> bool,
    ) -> Result<(), RemoveSocketError> {
        let mut error = None;
        for socket_path in paths {
            let socket_path = socket_path.as_ref();
            if !socket_path.exists() || !should_remove(socket_path) {
                continue;
            }

            if let Err(e) = std::fs::remove_file(socket_path)
                && e.kind() != ErrorKind::NotFound
            {
                // Log the error because we only return the first error when multiple occur.
                tracing::error!("failed to remove daemon socket {}: {e}", socket_path.display());
                error.get_or_insert_with(|| RemoveSocketError {
                    path: socket_path.to_owned(),
                    source: e,
                });
            }
        }

        error.map_or(Ok(()), Err)
    }

    /// Remove the sockets at `paths` that a daemon no longer listening left behind.
    pub(super) fn remove_stale(
        paths: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> Result<(), RemoveSocketError> {
        Self::remove_all(paths, Self::is_stale)
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
        SocketFile::remove_stale([&path]).unwrap();
        assert_eq!(path.exists(), kept);
    }
}
