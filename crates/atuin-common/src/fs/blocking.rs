//! Blocking twins of the [`crate::fs`] functions, for code outside a runtime: each waits for its
//! lease on the calling thread.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "the pool-backed wrappers are the only sanctioned std::fs callers"
)]

use std::fs::{Metadata, Permissions, ReadDir};
use std::io;
use std::path::{Path, PathBuf};

use super::pool::{FdPool, Leased};

/// Read the whole file at `path`.
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    FdPool::system().blocking_run(|| std::fs::read(path))
}

/// Read the whole file at `path` as UTF-8.
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    FdPool::system().blocking_run(|| std::fs::read_to_string(path))
}

/// Write `contents` to `path`, replacing the file if it exists.
pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    FdPool::system().blocking_run(|| std::fs::write(path, contents))
}

/// Iterate the entries of the directory at `path`, holding its descriptor's lease until dropped.
///
/// Entries collected out of it keep the directory open after the lease returns.
pub fn read_dir(path: impl AsRef<Path>) -> io::Result<Leased<ReadDir>> {
    FdPool::system().blocking_run_hold(|| std::fs::read_dir(path))
}

/// Create the directory at `path` and any missing ancestors.
pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    FdPool::system().blocking_run(|| std::fs::create_dir_all(path))
}

/// Create the directory at `path` with permission bits `mode`, whose parent must exist.
#[cfg(unix)]
pub fn create_secure_dir(path: impl AsRef<Path>, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    FdPool::system().blocking_run(|| std::fs::DirBuilder::new().mode(mode).create(path))
}

/// Rename `from` to `to`, replacing `to` if it exists.
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    FdPool::system().blocking_run(|| std::fs::rename(from, to))
}

/// Set the permissions of the file at `path`.
pub fn set_permissions(path: impl AsRef<Path>, permissions: Permissions) -> io::Result<()> {
    FdPool::system().blocking_run(|| std::fs::set_permissions(path, permissions))
}

/// The metadata of the file at `path`, following symbolic links.
pub fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    FdPool::system().blocking_run(|| std::fs::metadata(path))
}

/// The metadata of the file at `path`, without following a symbolic link.
pub fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    FdPool::system().blocking_run(|| std::fs::symlink_metadata(path))
}

/// The absolute form of `path`, with every symbolic link resolved.
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    FdPool::system().blocking_run(|| std::fs::canonicalize(path))
}

/// Whether `path` exists, following symbolic links; an error if that cannot be determined.
pub fn exists(path: impl AsRef<Path>) -> io::Result<bool> {
    FdPool::system().blocking_run(|| std::fs::exists(path))
}

/// An open file and the lease that pays for its descriptor.
pub type File = Leased<std::fs::File>;

impl File {
    /// Open the file at `path` read-only.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        FdPool::system().blocking_run_hold(|| std::fs::File::open(path))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{ErrorKind, Write};

    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;
    use crate::fs::OpenOptions;

    #[fixture]
    fn dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[rstest]
    fn a_file_round_trips(dir: TempDir) {
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        write(&a, "hello").unwrap();
        assert_eq!(read(&a).unwrap(), b"hello");
        assert_eq!(read_to_string(&a).unwrap(), "hello");
        assert_eq!(metadata(&a).unwrap().len(), 5);

        rename(&a, &b).unwrap();
        assert!(!exists(&a).unwrap());
        assert!(exists(&b).unwrap());
    }

    #[rstest]
    fn a_directory_round_trips(dir: TempDir) {
        let nested = dir.path().join("a/b");
        create_dir_all(&nested).unwrap();
        create_dir_all(nested.join("c")).unwrap();
        write(nested.join("f"), "").unwrap();

        // The system pool is process-wide, so this count relies on nextest's process per test.
        let held = FdPool::system().held();
        let mut dir_entries = read_dir(&nested).unwrap();
        assert_eq!(FdPool::system().held(), held + 1);
        let entries: Vec<_> = dir_entries.by_ref().collect::<io::Result<_>>().unwrap();
        drop(dir_entries);
        assert_eq!(FdPool::system().held(), held, "the lease outlived its ReadDir");

        let mut names: Vec<_> = entries.iter().map(std::fs::DirEntry::file_name).collect();
        names.sort();
        assert_eq!(names, ["c", "f"]);
    }

    #[rstest]
    fn an_open_file_holds_a_lease(dir: TempDir) {
        let path = dir.path().join("f");
        // The system pool is process-wide, so this count relies on nextest's process per test.
        let held = FdPool::system().held();

        let mut file = OpenOptions::new().write(true).create(true).blocking_open(&path).unwrap();
        assert_eq!(FdPool::system().held(), held + 1);
        file.write_all(b"hello").unwrap();
        drop(file);
        assert_eq!(FdPool::system().held(), held);

        let file = File::open(&path).unwrap();
        assert_eq!(FdPool::system().held(), held + 1);
        drop(file);
        assert_eq!(FdPool::system().held(), held);
    }

    #[rstest]
    fn open_options_append_and_create_new_match_std(dir: TempDir) {
        let path = dir.path().join("f");
        let mut create_new = OpenOptions::new();
        create_new.write(true).create_new(true);
        drop(create_new.blocking_open(&path).unwrap());
        assert_eq!(create_new.blocking_open(&path).unwrap_err().kind(), ErrorKind::AlreadyExists);

        let mut append = OpenOptions::new();
        append.append(true);
        for chunk in ["a", "b"] {
            append.blocking_open(&path).unwrap().write_all(chunk.as_bytes()).unwrap();
        }
        assert_eq!(read_to_string(&path).unwrap(), "ab");
    }
}
