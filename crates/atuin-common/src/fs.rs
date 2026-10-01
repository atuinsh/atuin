pub mod lock;
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
/// goes to a temporary file beside it first, which is then linked into place, so a reader (or a
/// watcher) only ever sees the whole file, and a file that appears at `path` meanwhile is left
/// alone. Fails with [`std::io::ErrorKind::AlreadyExists`] when `path` exists.
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
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.tmp", crate::utils::uuid_v7().as_simple()));
    let tmp = RemoveOnDropPath(dir.join(tmp_name));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&*tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    // A hard link fails if `path` exists, where a rename would replace it.
    match std::fs::hard_link(&*tmp, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Err(e),
        // A filesystem without hard links: rename, after checking again.
        Err(_) if !path.exists() => std::fs::rename(&*tmp, path),
        Err(_) => Err(ErrorKind::AlreadyExists.into()),
    }
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
}
