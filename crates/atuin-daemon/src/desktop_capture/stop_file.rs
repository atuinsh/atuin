//! The file that keeps `atuin desktop-capture stop` in force across daemon restarts.

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::PathBuf;

use atuin_common::path::PathBufExt as _;
use tracing::warn;

/// The name of the stop file in the desktop capture directory.
const NAME: &str = "stopped";

/// What the user asked for with `atuin desktop-capture start` and `stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requested {
    Running,
    Stopped,
}

#[derive(Debug, thiserror::Error)]
pub enum StopFileError {
    #[error("could not create the desktop capture stop file {path}: {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not remove the desktop capture stop file {path}: {source}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// A file whose existence means that the user stopped desktop capture.
///
/// The daemon reads the stop file when the daemon starts, and keeps the file up to date while the
/// daemon is running. The `atuin` CLI writes the file only when the CLI cannot reach the daemon, so
/// that the stop holds once a daemon starts.
#[derive(Debug, Clone)]
pub struct StopFile {
    path: PathBuf,
}

impl StopFile {
    /// The stop file in `dir`, the desktop capture directory.
    #[must_use]
    pub fn in_dir(dir: PathBuf) -> Self {
        Self {
            path: dir.joined(NAME),
        }
    }

    /// Whether `name` is the file name of a stop file.
    #[must_use]
    pub fn is_named(name: &OsStr) -> bool {
        name == NAME
    }

    /// What the stop file says. A stop file that cannot be checked counts as a stop.
    pub async fn requested(&self) -> Requested {
        match tokio::fs::try_exists(&self.path).await {
            Ok(true) => Requested::Stopped,
            Ok(false) => Requested::Running,
            Err(err) => {
                warn!(?err, path = ?self.path, "could not check the desktop capture stop file");
                Requested::Stopped
            }
        }
    }

    /// Makes the stop file say `requested`.
    pub async fn set(&self, requested: Requested) -> Result<(), StopFileError> {
        match requested {
            Requested::Running => self.remove().await,
            Requested::Stopped => self.create().await,
        }
    }

    /// Creates the stop file and its directory.
    pub async fn create(&self) -> Result<(), StopFileError> {
        let create = async {
            if let Some(dir) = self.path.parent() {
                tokio::fs::create_dir_all(dir).await?;
            }
            tokio::fs::write(&self.path, b"").await
        };
        create.await.map_err(|source| StopFileError::Create {
            path: self.path.clone(),
            source,
        })
    }

    /// Removes the stop file. Removing a missing stop file succeeds.
    pub async fn remove(&self) -> Result<(), StopFileError> {
        match tokio::fs::remove_file(&self.path).await {
            Err(err) if err.kind() != ErrorKind::NotFound => Err(StopFileError::Remove {
                path: self.path.clone(),
                source: err,
            }),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[tokio::test]
    async fn create_and_remove() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = StopFile::in_dir(dir.path().join("desktop-capture"));
        assert_eq!(file.requested().await, Requested::Running);

        file.create().await.expect("create");
        assert_eq!(file.requested().await, Requested::Stopped);

        file.remove().await.expect("remove");
        assert_eq!(file.requested().await, Requested::Running);
        file.remove().await.expect("remove a missing file");
    }
}
