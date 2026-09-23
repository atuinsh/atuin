//! Filesystem access, each call under a descriptor lease from [`FdPool::system`], mirroring
//! [`std::fs`].
//!
//! The functions here are async and run on tokio's blocking pool; [`blocking`] has their twins for
//! code outside a runtime.

pub mod blocking;
pub mod pool;

use std::fs::{DirEntry, Metadata, Permissions};
use std::io;
use std::path::{Path, PathBuf};

use self::pool::{FdPool, Leased};

/// Read the whole file at `path`.
pub async fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::read(path)).await
}

/// Read the whole file at `path` as UTF-8.
pub async fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::read_to_string(path)).await
}

/// Write `contents` to `path`, replacing the file if it exists.
pub async fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let (path, contents) = (path.as_ref().to_owned(), contents.as_ref().to_owned());
    FdPool::system().blocking(move || std::fs::write(path, contents)).await
}

/// List the entries of the directory at `path`.
///
/// The lease lives with the entries, since each keeps the directory's descriptor open until the
/// last of them drops.
pub async fn read_dir(path: impl AsRef<Path>) -> io::Result<Leased<Vec<DirEntry>>> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking_hold(move || std::fs::read_dir(path)?.collect()).await
}

/// The target of the symbolic link at `path`.
pub async fn read_link(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::read_link(path)).await
}

/// Remove the file at `path`.
pub async fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::remove_file(path)).await
}

/// Remove the directory at `path` and everything under it.
pub async fn remove_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::remove_dir_all(path)).await
}

/// Create the directory at `path`, whose parent must exist.
pub async fn create_dir(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::create_dir(path)).await
}

/// Create the directory at `path` and any missing ancestors.
pub async fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::create_dir_all(path)).await
}

/// Create the directory at `path` with permission bits `mode`, whose parent must exist.
#[cfg(unix)]
pub async fn create_secure_dir(path: impl AsRef<Path>, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::DirBuilder::new().mode(mode).create(path)).await
}

/// Rename `from` to `to`, replacing `to` if it exists.
pub async fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let (from, to) = (from.as_ref().to_owned(), to.as_ref().to_owned());
    FdPool::system().blocking(move || std::fs::rename(from, to)).await
}

/// Create `link` as a hard link to `original`.
pub async fn hard_link(original: impl AsRef<Path>, link: impl AsRef<Path>) -> io::Result<()> {
    let (original, link) = (original.as_ref().to_owned(), link.as_ref().to_owned());
    FdPool::system().blocking(move || std::fs::hard_link(original, link)).await
}

/// Set the permissions of the file at `path`.
pub async fn set_permissions(path: impl AsRef<Path>, permissions: Permissions) -> io::Result<()> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::set_permissions(path, permissions)).await
}

/// The metadata of the file at `path`, following symbolic links.
pub async fn metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::metadata(path)).await
}

/// The metadata of the file at `path`, without following a symbolic link.
pub async fn symlink_metadata(path: impl AsRef<Path>) -> io::Result<Metadata> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::symlink_metadata(path)).await
}

/// The absolute form of `path`, with every symbolic link resolved.
pub async fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::canonicalize(path)).await
}

/// Whether `path` exists, following symbolic links; an error if that cannot be determined.
pub async fn exists(path: impl AsRef<Path>) -> io::Result<bool> {
    let path = path.as_ref().to_owned();
    FdPool::system().blocking(move || std::fs::exists(path)).await
}

/// An open file and the lease that pays for its descriptor.
///
/// Dropping it with a tokio operation still in flight or unflushed can return the lease a moment
/// before tokio closes the file inside.
pub type File = Leased<tokio::fs::File>;

impl File {
    /// Open the file at `path` read-only.
    pub async fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_owned();
        let file = FdPool::system().blocking_hold(move || std::fs::File::open(path)).await?;
        Ok(Leased::map(file, tokio::fs::File::from_std))
    }

    /// Open the file at `path` write-only, creating or truncating it.
    pub async fn create(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_owned();
        let file = FdPool::system().blocking_hold(move || std::fs::File::create(path)).await?;
        Ok(Leased::map(file, tokio::fs::File::from_std))
    }
}

/// Options for opening a file under a lease, mirroring [`std::fs::OpenOptions`].
#[derive(Debug, Clone)]
pub struct OpenOptions(std::fs::OpenOptions);

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenOptions {
    /// Options with every flag off.
    #[must_use]
    pub fn new() -> Self {
        Self(std::fs::OpenOptions::new())
    }

    /// See [`std::fs::OpenOptions::read`].
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.0.read(read);
        self
    }

    /// See [`std::fs::OpenOptions::write`].
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.0.write(write);
        self
    }

    /// See [`std::fs::OpenOptions::append`].
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.0.append(append);
        self
    }

    /// See [`std::fs::OpenOptions::create`].
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.0.create(create);
        self
    }

    /// See [`std::fs::OpenOptions::create_new`].
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.0.create_new(create_new);
        self
    }

    /// See [`std::fs::OpenOptions::truncate`].
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.0.truncate(truncate);
        self
    }

    /// Permission bits `mode` for a file these options create.
    #[cfg(unix)]
    pub fn mode(&mut self, mode: u32) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;

        self.0.mode(mode);
        self
    }

    /// Open the file at `path` with these options.
    pub async fn open(&self, path: impl AsRef<Path>) -> io::Result<File> {
        let (options, path) = (self.0.clone(), path.as_ref().to_owned());
        let file = FdPool::system().blocking_hold(move || options.open(path)).await?;
        Ok(Leased::map(file, tokio::fs::File::from_std))
    }

    /// Equivalent to [`Self::open`], except it waits and opens on the calling thread.
    pub fn blocking_open(&self, path: impl AsRef<Path>) -> io::Result<blocking::File> {
        FdPool::system().blocking_run_hold(|| self.0.open(path))
    }
}

/// A path that is removed when this type is dropped.
pub struct RemoveOnDropPath<P: AsRef<Path> = PathBuf>(
    /// The path to the file.
    pub P,
);

impl<P: AsRef<Path>> Drop for RemoveOnDropPath<P> {
    fn drop(&mut self) {
        // Drop cannot wait for a lease, and skipping the removal would leak the file. Removing opens
        // no descriptor, so going ahead unleased on a full pool costs nothing.
        let _lease = FdPool::system().try_acquire();
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

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use rstest::{fixture, rstest};
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;

    use super::*;

    #[fixture]
    fn dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[rstest]
    #[tokio::test]
    async fn a_file_round_trips(dir: TempDir) {
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        write(&a, "hello").await.unwrap();
        assert_eq!(read(&a).await.unwrap(), b"hello");
        assert_eq!(read_to_string(&a).await.unwrap(), "hello");
        assert_eq!(metadata(&a).await.unwrap().len(), 5);

        rename(&a, &b).await.unwrap();
        assert!(!exists(&a).await.unwrap());
        assert!(exists(&b).await.unwrap());

        remove_file(&b).await.unwrap();
        assert!(!exists(&b).await.unwrap());
    }

    #[rstest]
    #[tokio::test]
    async fn a_directory_round_trips(dir: TempDir) {
        let nested = dir.path().join("a/b");
        create_dir_all(&nested).await.unwrap();
        create_dir(nested.join("c")).await.unwrap();
        write(nested.join("f"), "").await.unwrap();

        // The system pool is process-wide, so this count relies on nextest's process per test.
        let held = FdPool::system().held();
        let entries = read_dir(&nested).await.unwrap();
        assert_eq!(FdPool::system().held(), held + 1);
        let mut names: Vec<_> = entries.iter().map(DirEntry::file_name).collect();
        names.sort();
        assert_eq!(names, ["c", "f"]);
        drop(entries);
        assert_eq!(FdPool::system().held(), held);

        remove_dir_all(dir.path().join("a")).await.unwrap();
        assert!(!exists(&nested).await.unwrap());
    }

    #[rstest]
    #[tokio::test]
    async fn an_open_file_holds_a_lease(dir: TempDir) {
        let path = dir.path().join("f");
        // The system pool is process-wide, so this count relies on nextest's process per test.
        let held = FdPool::system().held();

        let mut file = File::create(&path).await.unwrap();
        assert_eq!(FdPool::system().held(), held + 1);
        file.write_all(b"hello").await.unwrap();
        file.flush().await.unwrap();
        drop(file);
        assert_eq!(FdPool::system().held(), held);

        let file = File::open(&path).await.unwrap();
        assert_eq!(FdPool::system().held(), held + 1);
        drop(file);
        assert_eq!(FdPool::system().held(), held);
    }

    #[rstest]
    #[tokio::test]
    async fn open_options_append_and_create_new_match_std(dir: TempDir) {
        let path = dir.path().join("f");
        let mut create_new = OpenOptions::new();
        create_new.write(true).create_new(true);
        drop(create_new.open(&path).await.unwrap());
        assert_eq!(create_new.open(&path).await.unwrap_err().kind(), ErrorKind::AlreadyExists);

        let mut append = OpenOptions::new();
        append.append(true);
        for chunk in ["a", "b"] {
            let mut file = append.open(&path).await.unwrap();
            file.write_all(chunk.as_bytes()).await.unwrap();
            file.flush().await.unwrap();
        }
        assert_eq!(read_to_string(&path).await.unwrap(), "ab");
    }

    #[rstest]
    fn remove_on_drop_path_removes_its_file(dir: TempDir) {
        let path = dir.path().join("f");
        std::fs::write(&path, "").unwrap();
        drop(RemoveOnDropPath(&path));
        assert!(!path.exists());
    }
}
