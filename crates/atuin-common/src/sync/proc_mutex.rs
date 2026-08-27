//! A [`ProcMutex`] is a mutex variant that guards a resource across all processes.
//!
//! In logic, it is equivalent to a [`std::sync::Mutex`], except that when the lock is acquired, it
//! is acquired against all other processes.
//!
//! This is facilitated by the [`ProcMutexPool`], a type which is responsible for issuing
//! [`ProcMutex<T>`] objects.
//!
//! When you invoke [`ProcMutex::try_lock`], the locking will only succeed if and only if no other
//! process has a lock on the same mutex.
//!
//! Mutices are identified with a unique name. In effect, two mutices are considered equivalent if
//! and only if they stem from the same [`ProcMutexPool`] (which itself is identified by a path) and
//! if their names are equivalent.
//!
//! Note that [`ProcMutex`] is many orders of magnitude slower than a [`std::sync::Mutex`] and
//! should be used sparingly -- only when you need a mutex that spans multiple processes.
//!
//! This type is implemented by a locked file. On Linux, `pthread_create` can be given arguments to
//! create cross-process pthread locks, which could be implemented in the future, as necessary, if
//! performance improvements are necessary.
use std::fs::{File, OpenOptions};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::os::file::{LockedExclusiveFile, LockingError};

const POLL_INTERVAL: Duration = Duration::from_millis(20);

fn open_lock_file(path: &Path) -> Result<File, std::io::Error> {
    OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)
}

/// The error returned by [`ProcMutex::try_lock`] and [`AsyncProcMutex::try_lock`].
#[derive(Debug, Error)]
pub enum TryLockError {
    #[error("the mutex is already held by another process")]
    WouldBlock,
    #[error("unexpected io error while locking the mutex: {0}")]
    Io(#[from] std::io::Error),
}

/// A poll represents a system location where [`ProcMutex`] objects can be issued from.
#[derive(Debug, Clone)]
pub struct ProcMutexPool {
    dir: PathBuf,
}

impl ProcMutexPool {
    pub fn new<P: AsRef<Path>>(dir: P) -> Result<Self, std::io::Error> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Create a new synchronous mutex that is shared across all processes. It is uniquely
    /// identified by `name`.
    ///
    /// It is legal to call this multiple times, with different `T` types with the same name, but do
    /// be wary of deadlocking. Mutices that are created sequentially with the same name will
    /// deadlock upon `try_lock` calls.
    pub fn new_sync_mutex<T>(&self, name: &str, value: T) -> Result<ProcMutex<T>, std::io::Error> {
        let path = self.dir.join(name);
        drop(open_lock_file(&path)?);
        Ok(ProcMutex {
            path,
            inner: parking_lot::Mutex::new(value),
        })
    }

    /// See [`Self::new_sync_mutex`]. Identical behavior, except returns async-safe mutices.
    pub fn new_async_mutex<T>(
        &self,
        name: &str,
        value: T,
    ) -> Result<AsyncProcMutex<T>, std::io::Error> {
        let path = self.dir.join(name);
        drop(open_lock_file(&path)?);
        Ok(AsyncProcMutex {
            path,
            inner: tokio::sync::Mutex::new(value),
        })
    }
}

/// A [`ProcMutex`] is a mutex variant that guards a resource across all processes.
///
/// In logic, it is equivalent to a [`std::sync::Mutex`], except that when the lock is acquired, it
/// is acquired against all other processes.
#[derive(Debug)]
pub struct ProcMutex<T> {
    path: PathBuf,
    inner: parking_lot::Mutex<T>,
}

impl<T> ProcMutex<T> {
    /// Try to lock the mutex, returning a new [`ProcMutexGuard`] which allows for interior
    /// mutability of the type.
    pub fn try_lock(&self) -> Result<ProcMutexGuard<'_, T>, TryLockError> {
        let Some(inner) = self.inner.try_lock() else {
            return Err(TryLockError::WouldBlock);
        };

        match LockedExclusiveFile::lock(open_lock_file(&self.path)?) {
            Ok(locked) => Ok(ProcMutexGuard {
                _locked: locked,
                inner,
            }),
            Err((_file, LockingError::AlreadyLocked(_))) => Err(TryLockError::WouldBlock),
            Err((_file, LockingError::Io(err))) => Err(TryLockError::Io(err)),
        }
    }

    /// Try to lock the mutex with a given timeout.
    pub fn lock_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<ProcMutexGuard<'_, T>>, std::io::Error> {
        let start = Instant::now();
        loop {
            match self.try_lock() {
                Ok(guard) => return Ok(Some(guard)),
                Err(TryLockError::Io(err)) => return Err(err),
                Err(TryLockError::WouldBlock) => {}
            }

            if start.elapsed() >= timeout {
                return Ok(None);
            }

            std::thread::sleep(POLL_INTERVAL);
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }

    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }
}

/// Guard wrapping a type returnned by [`ProcMutex::try_lock`].
pub struct ProcMutexGuard<'a, T> {
    _locked: LockedExclusiveFile,
    inner: parking_lot::MutexGuard<'a, T>,
}

impl<T> Deref for ProcMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> DerefMut for ProcMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

/// Identical to [`ProcMutex`] except operations behave in an async-safe fashion.
#[derive(Debug)]
pub struct AsyncProcMutex<T> {
    path: PathBuf,
    inner: tokio::sync::Mutex<T>,
}

impl<T> AsyncProcMutex<T> {
    pub fn try_lock(&self) -> Result<AsyncProcMutexGuard<'_, T>, TryLockError> {
        let Ok(inner) = self.inner.try_lock() else {
            return Err(TryLockError::WouldBlock);
        };

        match LockedExclusiveFile::lock(open_lock_file(&self.path)?) {
            Ok(locked) => Ok(AsyncProcMutexGuard {
                _locked: locked,
                inner,
            }),
            Err((_file, LockingError::AlreadyLocked(_))) => Err(TryLockError::WouldBlock),
            Err((_file, LockingError::Io(err))) => Err(TryLockError::Io(err)),
        }
    }

    pub async fn lock_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<AsyncProcMutexGuard<'_, T>>, std::io::Error> {
        let start = Instant::now();
        loop {
            match self.try_lock() {
                Ok(guard) => return Ok(Some(guard)),
                Err(TryLockError::Io(err)) => return Err(err),
                Err(TryLockError::WouldBlock) => {}
            }

            if start.elapsed() >= timeout {
                return Ok(None);
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }

    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }
}

pub struct AsyncProcMutexGuard<'a, T> {
    _locked: LockedExclusiveFile,
    inner: tokio::sync::MutexGuard<'a, T>,
}

impl<T> Deref for AsyncProcMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> DerefMut for AsyncProcMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}
