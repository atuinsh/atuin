//! Utilities for operating on files, locking them, etc.

use std::fs::{File, OpenOptions};
use std::os::fd::AsFd;
use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LockingError {
    #[error("another process is already locking this file: {0}")]
    AlreadyLocked(std::io::Error),
    #[error("unexpected IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum LockedFileOpenError<P: AsRef<Path>> {
    #[error("failed to open {}: {}", .0.as_ref().display(), .1)]
    Open(P, std::io::Error),
    #[error("failed to lock {}: {}", .0.as_ref().display(), .1)]
    Lock(P, LockingError),
}

/// Newtype wrapping [`File`] which locks its files with the `flock` C function.
///
/// **Do note that `flock` is not standardized by POSIX, however, is generally available.**
///
/// Note that the [`LockedExclusiveFile`] is unlocked on [`Drop`] or process exit (regardless of the
/// exit code -- even SIGKILL will release the lock).
///
/// Note that although this does not directly implement [`Drop`], dropping the underlying [`File`]
/// causes the kernel to close the `fd`, which, in turn, drops the lock too.
#[derive(Debug)]
pub struct LockedExclusiveFile {
    file: File,
}

impl LockedExclusiveFile {
    /// Take ownership of the given [`File`], locking it from other processes accesing it.
    pub fn lock(file: File) -> Result<Self, (File, LockingError)> {
        // Note we intentionally use `rustix` here since `File::try_lock` does not guarantee the use
        // of `flock`. Other locking implementations may not have the semantics of dropping the lock
        // on SIGKILL.
        use rustix::fs::{FlockOperation, flock};
        match flock(file.as_fd(), FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Self { file }),
            Err(err) => {
                let err = std::io::Error::from(err);
                let locking = match err.kind() {
                    std::io::ErrorKind::WouldBlock => LockingError::AlreadyLocked(err),
                    _ => LockingError::Io(err),
                };
                Err((file, locking))
            }
        }
    }

    /// Given the file path, open the path as a [`LockedExclusiveFile`].
    pub fn open<P: AsRef<Path>>(
        path: P,
        open_options: OpenOptions,
    ) -> Result<Self, LockedFileOpenError<P>> {
        // Linux doesn't have a way to do this in one syscall. macOS does, but consistency, in this
        // case, is easier to debug, than reducing a rarely called syscall.
        let file = match open_options.open(path.as_ref()) {
            Ok(file) => file,
            Err(err) => return Err(LockedFileOpenError::Open(path, err)),
        };
        Self::lock(file).map_err(|(_file, err)| LockedFileOpenError::Lock(path, err))
    }

    /// Unlock the file, enabling other processes on the system to lock it.
    pub fn unlock(self) -> Result<File, std::io::Error> {
        use rustix::fs::{FlockOperation, flock};
        flock(self.file.as_fd(), FlockOperation::Unlock)?;
        Ok(self.file)
    }

    /// Grab a handle to the file.
    pub const fn file(&self) -> &File {
        &self.file
    }

    /// Grab a mutable handle to the file.
    pub const fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }
}

impl TryFrom<File> for LockedExclusiveFile {
    type Error = (File, LockingError);

    fn try_from(value: File) -> Result<Self, Self::Error> {
        LockedExclusiveFile::lock(value).map_err(|(file, err)| (file, err))
    }
}
