use std::io::ErrorKind;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use atuin_client::settings::Daemon as DaemonSettings;
use tokio::task::JoinHandle;

/// The socket file a daemon bound itself, kept fresh against temporary file cleaners and removed
/// when dropped.
pub(super) struct SocketFile {
    path: PathBuf,
    keepalive: JoinHandle<()>,
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
        let touched = path.clone();
        let keepalive = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Self::KEEPALIVE_INTERVAL);
            loop {
                interval.tick().await;
                if let Err(e) = atuin_common::os::unix::touch_file(&touched) {
                    tracing::warn!("failed to refresh {}: {e}", touched.display());
                }
            }
        });
        Self { path, keepalive }
    }

    /// Remove the daemon's socket from every path it may be at, subject to `should_remove`.
    pub(super) fn remove_all(
        daemon: &DaemonSettings,
        should_remove: impl Fn(&Path) -> bool,
    ) -> Result<(), RemoveSocketError> {
        let mut error = None;
        for socket_path in crate::client::potential_socket_paths(daemon) {
            if !socket_path.exists() || !should_remove(&socket_path) {
                continue;
            }

            if let Err(e) = std::fs::remove_file(&socket_path)
                && e.kind() != ErrorKind::NotFound
            {
                // Log the error because we only return the first error when multiple occur.
                tracing::error!("failed to remove daemon socket {}: {e}", socket_path.display());
                error.get_or_insert_with(|| RemoveSocketError {
                    path: socket_path.into_owned(),
                    source: e,
                });
            }
        }

        error.map_or(Ok(()), Err)
    }

    /// Remove any socket left behind by a daemon that is no longer listening.
    pub(super) fn remove_stale(daemon: &DaemonSettings) -> Result<(), RemoveSocketError> {
        Self::remove_all(daemon, |socket_path| {
            matches!(atuin_common::os::unix::socket_in_use(socket_path), Ok(false))
                // Ensure file is a socket; `socket_in_use` can return `Ok(false)` for regular files.
                && std::fs::symlink_metadata(socket_path)
                    .is_ok_and(|meta| meta.file_type().is_socket())
        })
    }
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        self.keepalive.abort();
        tracing::info!("removing socket {}", self.path.display());
        if let Err(e) = std::fs::remove_file(&self.path)
            && e.kind() != ErrorKind::NotFound
        {
            tracing::warn!("failed to remove socket {}: {e}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::settings::Settings;
    use atuin_common::path::AbsPathBuf;
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
        let mut daemon = Settings::default().daemon;
        daemon.socket_path = AbsPathBuf::new(path.clone()).ok();
        SocketFile::remove_stale(&daemon).unwrap();
        assert_eq!(path.exists(), kept);
    }
}
