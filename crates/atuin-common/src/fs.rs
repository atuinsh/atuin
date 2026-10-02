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
