//! A generic wrapper over a PID file.
//!
//! PID files, by convention, contain metadata about the process currently owning the file.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::os::file::{LockedExclusiveFile, LockingError};

const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(20);

pub trait IsCodecError: std::error::Error + Send + Sync + Sized + 'static {}

impl<T: std::error::Error + Send + Sync + 'static> IsCodecError for T {}

/// Defines the contents stored within a PID file.
pub trait IsPidfileBody: Sized {
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

    #[error("failed to read the pidfile: {0}")]
    Peeking(#[from] PidFilePeekError<CE>),

    #[error("unexpected error encoding pid file: {0}")]
    EncodingError(CE),
}

#[derive(Debug, Error)]
pub enum PidFilePeekError<CE: IsCodecError> {
    #[error("error decoding: {0}")]
    DecodingError(CE),

    #[error("error reading: {0}")]
    Io(#[from] std::io::Error),
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
    ) -> Result<PidFileLock<()>, PidfileLockingError<B::CodecError>> {
        let encoded = meta.to_bytes().map_err(PidfileLockingError::EncodingError)?;

        let mut locked = match LockedExclusiveFile::try_from(self.file) {
            Ok(f) => f,
            Err((file, LockingError::AlreadyLocked(_))) => {
                // Awesome, well we know that some other process is holding the file hostage,
                // and there's not much we can do about it.
                //
                // Let's get more info and abort.
                let body = Self::peekf::<B>(&file)?;

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

        Ok(PidFileLock::new(locked, ()))
    }

    pub async fn lock_timeout<B: IsPidfileBody>(
        self,
        meta: &B,
        timeout: Duration,
    ) -> Result<Option<PidFileLock<()>>, PidfileLockingError<B::CodecError>> {
        let encoded = meta.to_bytes().map_err(PidfileLockingError::EncodingError)?;

        let mut file = self.file;
        let start = Instant::now();
        loop {
            match LockedExclusiveFile::lock(file) {
                Ok(mut locked) => {
                    let f = locked.file_mut();
                    f.set_len(0)?;
                    f.write_all(&encoded).map_err(PidfileLockingError::Stamping)?;
                    return Ok(Some(PidFileLock::new(locked, ())));
                }
                Err((f, LockingError::AlreadyLocked(_))) => {
                    if start.elapsed() >= timeout {
                        return Ok(None);
                    }
                    file = f;
                    tokio::time::sleep(LOCK_POLL_INTERVAL).await;
                }
                Err((_file, LockingError::Io(err))) => {
                    return Err(PidfileLockingError::LockingIo(err));
                }
            }
        }
    }

    /// Open the [`PidFile`] from the given path.
    ///
    ///   - The file will be created if necessary, and so will its parent, if necessary.
    ///   - The file is created under `0o600` permissions.
    pub fn open_or_create<P: AsRef<Path>>(path: P) -> Result<Self, std::io::Error> {
        let path = path.as_ref();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file =
            OpenOptions::new().write(true).read(true).create(true).truncate(false).open(path)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }

        Ok(Self { file })
    }

    fn peekf<B: IsPidfileBody>(mut file: &File) -> Result<B, PidFilePeekError<B::CodecError>> {
        file.rewind()?;

        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;

        B::from_bytes(&buf).map_err(PidFilePeekError::DecodingError)
    }

    /// Read the value of the pidfile without acquiring a lock on it.
    pub fn peek<B: IsPidfileBody>(&self) -> Result<B, PidFilePeekError<B::CodecError>> {
        Self::peekf(&self.file)
    }
}

#[derive(Debug)]
struct Lease {
    #[allow(dead_code)]
    file: LockedExclusiveFile,
}

pub struct PidFileLock<T> {
    lease: Arc<Lease>,
    data: T,
}

impl<T> PidFileLock<T> {
    fn new(file: LockedExclusiveFile, data: T) -> Self {
        Self {
            lease: Arc::new(Lease { file }),
            data,
        }
    }

    /// Replace the payload, transforming it while keeping the same held lock.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> PidFileLock<U> {
        PidFileLock {
            lease: self.lease,
            data: f(self.data),
        }
    }

    /// Fallibly replace the payload, keeping the same held lock on success.
    pub fn try_map<U, E>(self, f: impl FnOnce(T) -> Result<U, E>) -> Result<PidFileLock<U>, E> {
        Ok(PidFileLock {
            lease: self.lease,
            data: f(self.data)?,
        })
    }

    /// Share the held lock, attaching a new payload to the clone.
    pub fn with_payload<U>(&self, data: U) -> PidFileLock<U> {
        PidFileLock {
            lease: Arc::clone(&self.lease),
            data,
        }
    }

    pub fn into_data(self) -> T {
        self.data
    }
}

impl<T: Clone> Clone for PidFileLock<T> {
    fn clone(&self) -> Self {
        Self {
            lease: Arc::clone(&self.lease),
            data: self.data.clone(),
        }
    }
}

impl<T> std::ops::Deref for PidFileLock<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.data
    }
}

impl<T> std::ops::DerefMut for PidFileLock<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for PidFileLock<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PidFileLock").field("lease", &self.lease).field("data", &self.data).finish()
    }
}
