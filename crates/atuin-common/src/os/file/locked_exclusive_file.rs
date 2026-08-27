use std::fs::{File, OpenOptions, TryLockError};
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

#[derive(Debug)]
pub struct LockedExclusiveFile {
    file: File,
}

impl LockedExclusiveFile {
    pub fn lock(file: File) -> Result<Self, (File, LockingError)> {
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(TryLockError::WouldBlock) => Err((
                file,
                LockingError::AlreadyLocked(std::io::Error::from(std::io::ErrorKind::WouldBlock)),
            )),
            Err(TryLockError::Error(err)) => Err((file, LockingError::Io(err))),
        }
    }

    pub fn open<P: AsRef<Path>>(
        path: P,
        open_options: OpenOptions,
    ) -> Result<Self, LockedFileOpenError<P>> {
        let file = match open_options.open(path.as_ref()) {
            Ok(file) => file,
            Err(err) => return Err(LockedFileOpenError::Open(path, err)),
        };
        Self::lock(file).map_err(|(_file, err)| LockedFileOpenError::Lock(path, err))
    }

    pub fn unlock(self) -> Result<File, std::io::Error> {
        self.file.unlock()?;
        Ok(self.file)
    }

    pub const fn file(&self) -> &File {
        &self.file
    }

    pub const fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }
}

impl TryFrom<File> for LockedExclusiveFile {
    type Error = (File, LockingError);

    fn try_from(value: File) -> Result<Self, Self::Error> {
        LockedExclusiveFile::lock(value)
    }
}
