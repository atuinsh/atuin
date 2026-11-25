//! Notices a replacement of the helper's file, which can fix a helper that failed.

use std::path::Path;
use std::time::{Duration, SystemTime};

/// How often a watch looks at the file.
///
/// Each look is one `stat`, so a watch costs little however long it runs.
pub const POLL: Duration = Duration::from_secs(2);

/// The helper's file as a run of the helper found it. The calls take the file's path, which the
/// settings hold.
///
/// A reinstall replaces a missing helper, a helper of another build, or a helper whose signature
/// broke, so a change of the file is a good reason to start a helper again at once. On macOS,
/// the daemon signs a copy of this file, and a change of this file gives a new copy.
#[derive(Debug)]
pub struct HelperFile {
    seen: Option<Stamp>,
}

/// What changes when a file is replaced: its length and its modification time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    /// The stamp of the file at `path`. `None` when the file can't be read. For example, the
    /// file may be missing.
    fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

impl HelperFile {
    /// The file at `path` as it is now.
    pub fn of(path: &Path) -> Self {
        Self {
            seen: Stamp::of(path),
        }
    }

    /// Whether the file at `path` changed since [`of`](Self::of): it appeared, went, or was
    /// replaced.
    pub fn changed(&self, path: &Path) -> bool {
        Stamp::of(path) != self.seen
    }

    /// Resolves once the file at `path` has changed. The call checks the file every [`POLL`].
    pub async fn changes(&self, path: &Path) {
        loop {
            tokio::time::sleep(POLL).await;
            if self.changed(path) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    #[fixture]
    fn dir() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// What happens to the file after a run found it.
    #[derive(Debug, Clone, Copy)]
    enum Then {
        Nothing,
        Replaced,
        Removed,
        Installed,
    }

    #[rstest]
    #[case::unchanged(Some("helper"), Then::Nothing, false)]
    #[case::still_missing(None, Then::Nothing, false)]
    #[case::replaced(Some("helper"), Then::Replaced, true)]
    #[case::removed(Some("helper"), Then::Removed, true)]
    #[case::installed(None, Then::Installed, true)]
    fn notices_a_change_of_the_file(
        dir: TempDir,
        #[case] content: Option<&str>,
        #[case] then: Then,
        #[case] changed: bool,
    ) {
        let path = dir.path().join("atuin-desktop-capture");
        if let Some(content) = content {
            std::fs::write(&path, content).expect("write");
        }
        let file = HelperFile::of(&path);

        match then {
            Then::Nothing => {}
            Then::Replaced => std::fs::write(&path, "another helper").expect("write"),
            Then::Removed => std::fs::remove_file(&path).expect("remove"),
            Then::Installed => std::fs::write(&path, "helper").expect("write"),
        }

        assert_eq!(file.changed(&path), changed);
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn changes_resolves_once_the_file_changes(dir: TempDir) {
        let path = dir.path().join("atuin-desktop-capture");
        let file = HelperFile::of(&path);
        let unchanged = tokio::time::timeout(POLL * 5, file.changes(&path)).await;
        assert!(unchanged.is_err());

        std::fs::write(&path, "helper").expect("write");

        tokio::time::timeout(POLL * 2, file.changes(&path)).await.expect("the change");
    }
}
