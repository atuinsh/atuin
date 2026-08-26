//! A generic wrapper over a PID file.
//!
//! PID files, by convention, contain metadata about the process currently owning the file.
//!
//! In our case, this is encoded by the [`PidMeta`] structure.

use std::fs::{File, OpenOptions, Permissions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use thiserror::Error;

use crate::os::unix::file::LockedExclusiveFile;
use crate::os::unix::file::locked_exclusive_file::LockingError;

pub trait IsCodecError: std::error::Error + Send + Sync + Sized + 'static {}

/// Defines the contents stored within a PID file.
trait IsPidfileBody: Sized {
    type CodecError: IsCodecError;

    /// The PID which owns this pid file.
    fn owner(&self) -> u32;

    /// Convert the contents into bytes.
    ///
    /// Keep the result small -- this isn't intended to hold a huge amount of data.
    fn to_bytes(&self) -> Result<Vec<u8>, Self::CodecError>;

    /// Create a file from the given bytes.
    fn from_bytes(bytes: &[u8]) -> Result<Self, Self::CodecError>;
}

#[derive(Debug, Error)]
pub enum PidfileLockingError<CE: IsCodecError> {
    #[error("this file is owned (or was most recently owned) by another process with pid={0}")]
    OwnedByAnotherPid(u32),

    #[error("unexpected io error locking pid file: {0}")]
    LockingIo(#[from] std::io::Error),

    #[error("unexpected io error while stamping pid file {0}")]
    Stamping(std::io::Error),

    #[error("unexpected error decoding pid file: {0}")]
    DecodingError(CE),

    #[error("unexpected error encoding pid file: {0}")]
    EncodingError(CE),
}

/// Represents a PID file on-disk.
///
/// This function intentionally does not implement [`From<File>`] as that is considered a footgun --
/// [`File`] does not specify the mode, or permissions, so we here wish to be stricter.
#[derive(Debug)]
pub struct PidFile {
    file: File,
}

impl PidFile {
    /// Attempt to lock the given file to create a [`LockedPidFile`].
    ///
    /// A locked PID file has the following semantics:
    ///
    ///   - Other processes can open/read/write the file.
    ///   - Other processes **cannot** lock the file.
    ///   - It is stamped with metadata implementing [`IsPidfileBody`].
    ///
    /// This function requires you provide the metadata identifying the active process. This can be
    /// any structure you wish, and it will be written into the PID file.
    pub fn try_lock<B: IsPidfileBody>(
        self,
        meta: &B,
    ) -> Result<LockedPidFile, PidfileLockingError<B::CodecError>> {
        let encoded = meta.to_bytes().map_err(PidfileLockingError::EncodingError)?;

        let mut locked = match LockedExclusiveFile::try_from(self.file) {
            Ok(f) => f,
            Err((file, LockingError::AlreadyLocked(_))) => {
                // Awesome, well we know that some other process is holding the file hostage,
                // and there's not much we can do about it.
                //
                // Let's get more info and abort.
                let mut buf = Vec::new();
                (&file).read_to_end(&mut buf)?;

                let body = B::from_bytes(&buf).map_err(PidfileLockingError::DecodingError)?;

                return Err(PidfileLockingError::OwnedByAnotherPid(body.owner()));
            }
            Err((_file, LockingError::Io(err))) => {
                // Seems like there was an error attempting to lock this file, there's not
                // really much we can do, as this is completely unexpected. Let's just abort.
                return Err(PidfileLockingError::LockingIo(err));
            }
        };

        let file = locked.file_mut();
        file.set_len(0)?;
        file.write_all(&encoded).map_err(PidfileLockingError::Stamping)?;

        Ok(LockedPidFile { file: locked })
    }

    /// Open the [`PidFile`] from the given path.
    ///
    ///   - The file will be created if necessary, and so will its parent, if necessary.
    ///   - The file is created under `0o600` permissions.
    pub fn open_or_create<P: AsRef<Path>>(self, path: P) -> Result<Self, std::io::Error> {
        let path = path.as_ref();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file =
            OpenOptions::new().write(true).read(true).create(true).truncate(false).open(path)?;

        file.set_permissions(Permissions::from_mode(0o600))?;

        Ok(Self { file })
    }
}

/// Represents a file which is locked by the active process.
///
/// - Other processes can read/write to the file, however, they cannot lock the file.
/// - The file is unlocked on [`Drop`].
/// - The locked pid file is periodically touched in the background while it is locked.
#[derive(Debug)]
pub struct LockedPidFile {
    file: LockedExclusiveFile,
}

impl LockedPidFile {
    /// Unlock this file so other processes can lock it.
    pub fn unlock(self) -> Result<PidFile, std::io::Error> {
        let unlocked = self.file.unlock()?;

        Ok(PidFile { file: unlocked })
    }
}
