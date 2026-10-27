pub mod lock;
#[cfg(feature = "fs-watcher")]
pub mod tree_watcher;

use std::path::{Path, PathBuf};

/// A path that is removed when this type is dropped.
pub struct RemoveOnDropPath<P: AsRef<Path> = PathBuf>(
    /// The path to the file.
    pub P,
);

impl<P: AsRef<Path>> Drop for RemoveOnDropPath<P> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// Not using `derive_more` as it causes Clippy to emit an incorrect "this trait bound is already
// specified in the where clause" warning.
impl<P: AsRef<Path>> AsRef<Path> for RemoveOnDropPath<P> {
    fn as_ref(&self) -> &Path {
        self
    }
}

// Not using `derive_more` for this as it would require adding a `P: Deref<Target = Path>` bound to
// the struct.
impl<P: AsRef<Path>> std::ops::Deref for RemoveOnDropPath<P> {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

/// Write `contents` to a new file at `path`, atomically and never over an existing file: the data
/// goes to a temporary file beside it first, which is then moved into place only if nothing is
/// there (`renameat2(RENAME_NOREPLACE)` or a hard link on unix, `MoveFileExW` without
/// `MOVEFILE_REPLACE_EXISTING` on Windows; see [`tempfile::NamedTempFile::persist_noclobber`]),
/// so a reader (or a watcher) only ever sees the whole file, and a file that appears at `path`
/// meanwhile is left alone. Fails with [`std::io::ErrorKind::AlreadyExists`] when `path` exists.
///
/// Where the filesystem can do neither, `path` is created with `O_EXCL` and written in place:
/// still never over another file, though a reader may then see it before it is whole.
///
/// The temporary file's name starts with `.` and ends in `.tmp`, so a watcher filtering on the
/// final file's extension never picks it up.
pub fn write_new(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind, Write as _};

    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(Error::new(ErrorKind::InvalidInput, "not a file path"));
    };
    if path.exists() {
        return Err(ErrorKind::AlreadyExists.into());
    }
    let mut prefix = std::ffi::OsString::from(".");
    prefix.push(name);
    prefix.push(".");
    let mut tmp = tempfile::Builder::new().prefix(&prefix).suffix(".tmp").tempfile_in(dir)?;
    tmp.write_all(contents)?;
    tmp.as_file().sync_all()?;
    persist_new(tmp, path, contents)
}

/// Why [`replace`] failed, and whether the file was replaced all the same.
#[derive(Debug, thiserror::Error)]
pub enum ReplaceError {
    /// Nothing was replaced: the file at the path is as it was (and no temporary file is left).
    /// [`std::io::ErrorKind::Interrupted`] when the file changed since it was read.
    #[error(transparent)]
    NotReplaced(std::io::Error),
    /// The new file was moved into place, but its directory could not be synced after: the file
    /// holds the new content, though a crash before the directory reaches the disk could still
    /// bring back the old one.
    #[error("the file was replaced, but syncing its directory failed: {0}")]
    Unsynced(std::io::Error),
}

impl ReplaceError {
    /// Whether the file holds the new content: the replace happened.
    #[must_use]
    pub const fn replaced(&self) -> bool {
        matches!(self, Self::Unsynced(_))
    }
}

/// Replace the file at `path` with `contents`, atomically: the data goes to a temporary file in the
/// same directory (named as [`write_new`] names it, so no watcher takes it for the file), is
/// synced, and is then renamed over `path` (`rename(2)`; `MoveFileExW` with
/// `MOVEFILE_REPLACE_EXISTING` on Windows), so a reader sees the old file or the new one, never
/// half of either. The directory is synced after, on unix, so the rename survives a crash.
///
/// `still` is handed what is at `path` just before the rename, read after the temporary file is
/// written: the replace goes ahead only when it says the file is still the one the caller read,
/// else fails with [`std::io::ErrorKind::Interrupted`] and leaves `path` alone. It narrows, but
/// can't close, the window for a writer appending meanwhile; callers check that no writer has the
/// file open first.
///
/// The error says whether the file was replaced ([`ReplaceError::replaced`]): only a failure to
/// sync the directory comes after the rename, and leaves the new file in place.
///
/// The file keeps its path, not its inode: a reader that holds it open goes on reading the old
/// content, and one following it by path finds it replaced (`atuin_common::io`'s readers read
/// it again from its start).
pub fn replace(
    path: &Path,
    contents: &[u8],
    still: impl FnOnce(&[u8]) -> bool,
) -> Result<(), ReplaceError> {
    replace_syncing(path, contents, still, sync_dir)
}

/// Sync directory `dir`, so a rename in it survives a crash (on unix; nothing elsewhere).
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// [`replace`], syncing the directory with `sync`.
pub(crate) fn replace_syncing(
    path: &Path,
    contents: &[u8],
    still: impl FnOnce(&[u8]) -> bool,
    sync: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), ReplaceError> {
    use std::io::{Error, ErrorKind, Write as _};

    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(ReplaceError::NotReplaced(Error::new(
            ErrorKind::InvalidInput,
            "not a file path",
        )));
    };
    let written = (|| {
        let mut prefix = std::ffi::OsString::from(".");
        prefix.push(name);
        prefix.push(".");
        let mut tmp = tempfile::Builder::new().prefix(&prefix).suffix(".tmp").tempfile_in(dir)?;
        tmp.write_all(contents)?;
        // The file's permissions, not the temporary file's private ones.
        let permissions = std::fs::metadata(path)?.permissions();
        tmp.as_file().set_permissions(permissions)?;
        tmp.as_file().sync_all()?;
        if !still(&std::fs::read(path)?) {
            return Err(Error::new(ErrorKind::Interrupted, "the file changed since it was read"));
        }
        // A failed rename leaves the file as it was, and removes the temporary file.
        tmp.persist(path).map_err(|e| e.error)?;
        Ok(())
    })();
    written.map_err(ReplaceError::NotReplaced)?;
    sync(dir).map_err(ReplaceError::Unsynced)
}

/// Move `tmp`, holding `contents`, to `path` if nothing is there; see [`write_new`].
fn persist_new(tmp: tempfile::NamedTempFile, path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::ErrorKind;

    match tmp.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(e) if e.error.kind() == ErrorKind::AlreadyExists || path.exists() => {
            Err(ErrorKind::AlreadyExists.into())
        }
        // Neither a no-replace rename nor a hard link here (the temporary file is removed as
        // `e` drops).
        Err(_) => copy_new(path, contents),
    }
}

/// Create `path`, which must not exist, and write `contents` to it; a file left half-written is
/// removed again.
fn copy_new(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    let written = file.write_all(contents).and_then(|()| file.sync_all());
    if written.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    written
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// A replace moves a new file into place (a new inode, so readers following the path see it
    /// replaced), leaving no temporary file; one whose file changed since it was read leaves it.
    #[rstest]
    #[case::unchanged(true)]
    #[case::changed(false)]
    fn replace_moves_a_whole_new_file_into_place_only_if_unchanged(#[case] unchanged: bool) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        std::fs::write(&path, b"one\ntwo\n").unwrap();
        #[cfg(unix)]
        let before = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&path).unwrap());

        let replaced = replace(&path, b"three\n", |current| {
            assert_eq!(current, b"one\ntwo\n");
            unchanged
        });
        if unchanged {
            replaced.unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"three\n");
            #[cfg(unix)]
            assert_ne!(
                std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&path).unwrap()),
                before
            );
        } else {
            let Err(ReplaceError::NotReplaced(e)) = replaced else {
                panic!("not replaced: {replaced:?}");
            };
            assert_eq!(e.kind(), std::io::ErrorKind::Interrupted);
            assert_eq!(std::fs::read(&path).unwrap(), b"one\ntwo\n");
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no temporary file left");
        // Nothing to replace.
        let gone = dir.path().join("gone.jsonl");
        assert!(matches!(replace(&gone, b"x", |_| true), Err(ReplaceError::NotReplaced(_))));
        assert!(!gone.exists());
    }

    /// A directory that can't be synced once the new file is in place fails the replace as one
    /// that happened: the file holds the new content, which the caller must not take for the old.
    #[rstest]
    fn a_replace_whose_directory_fails_to_sync_says_it_replaced_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        std::fs::write(&path, b"one\n").unwrap();
        let failed =
            replace_syncing(&path, b"two\n", |_| true, |_| Err(std::io::Error::other("injected")))
                .unwrap_err();
        assert!(matches!(failed, ReplaceError::Unsynced(_)), "{failed:?}");
        assert!(failed.replaced());
        assert_eq!(std::fs::read(&path).unwrap(), b"two\n");
        // Failing before the rename, it did not.
        let failed = replace_syncing(&path, b"three\n", |_| false, |_| Ok(())).unwrap_err();
        assert!(!failed.replaced());
        assert_eq!(std::fs::read(&path).unwrap(), b"two\n");
    }

    #[rstest]
    fn write_new_writes_a_new_file_and_never_replaces_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        write_new(&path, b"one").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one");

        let err = write_new(&path, b"two").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        // No temporary file is left behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// A file that appears at the path after the check, while the temporary file is written, is
    /// never replaced, however the temporary file is moved into place.
    #[rstest]
    fn a_file_that_appears_meanwhile_is_never_replaced() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        let mut tmp = tempfile::Builder::new().prefix(".a.").tempfile_in(dir.path()).unwrap();
        tmp.write_all(b"mine").unwrap();
        std::fs::write(&path, b"theirs").unwrap();

        let err = persist_new(tmp, &path, b"mine").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"theirs");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

        let err = copy_new(&path, b"mine").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"theirs");

        let other = dir.path().join("b.jsonl");
        copy_new(&other, b"mine").unwrap();
        assert_eq!(std::fs::read(&other).unwrap(), b"mine");
    }
}
