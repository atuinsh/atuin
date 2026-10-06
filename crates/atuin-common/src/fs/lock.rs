//! File locking utilities.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::ops::ControlFlow;
use std::path::Path;
use std::time::Duration;

use crate::futures::Backoff;

/// How often [`LockOptions::wait`] waits between attempts to acquire the lock.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Which exclusivity mode to use when acquiring a file lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockMode {
    /// Acquire a shared lock.
    Shared,
    /// Acquire an exclusive lock.
    Exclusive,
}

/// Options for opening and locking a file.
///
/// After creating this struct, call [`Self::open`] or [`Self::wait`] depending on the semantics you
/// want.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockOptions {
    /// Whether to create the lock file and its parent directories if they don't exist.
    pub create: bool,

    /// Which kind of lock to take.
    pub mode: LockMode,
}

/// An error that occurred while opening or locking a file.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("could not create parent directory: {0}")]
    CreateDir(io::Error),
    #[error("could not open file: {0}")]
    Open(io::Error),
    #[error("could not acquire lock: {0}")]
    Lock(io::Error),
    #[error("lock is in use by another process")]
    WouldBlock,
    #[error("timed out while waiting for lock")]
    Timeout,
}

impl LockError {
    /// Whether this error was returned because the lock file didn't exist.
    ///
    /// This can only be true if [`LockOptions::create`] was false.
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Open(e) if e.kind() == io::ErrorKind::NotFound)
    }
}

impl LockOptions {
    /// Open and lock the file at `path`.
    ///
    /// This method blocks until the lock is available. The lock is released when the returned
    /// [`File`] is dropped.
    pub fn open(self, path: &Path) -> Result<File, LockError> {
        let file = self.open_file(path)?;
        let result = match self.mode {
            LockMode::Shared => file.lock_shared(),
            LockMode::Exclusive => file.lock(),
        };
        result.map_err(LockError::Lock)?;
        Ok(file)
    }

    /// Open and try to lock the file at `path`.
    ///
    /// If the lock is already in use, this immediately returns [`LockError::WouldBlock`].
    pub fn try_open(self, path: &Path) -> Result<File, LockError> {
        let file = self.open_file(path)?;
        let result = match self.mode {
            LockMode::Shared => file.try_lock_shared(),
            LockMode::Exclusive => file.try_lock(),
        };
        result.map_err(|e| match e {
            TryLockError::WouldBlock => LockError::WouldBlock,
            TryLockError::Error(e) => LockError::Lock(e),
        })?;
        Ok(file)
    }

    /// Try to lock the file at `path` in a given amount of time.
    ///
    /// This method will repeatedly try to lock `path`. If `timeout` elapses and the lock could not
    /// be acquired, [`LockError::Timeout`] is returned.
    ///
    /// On success, the lock is released when the returned [`File`] is dropped.
    pub async fn wait(self, path: &Path, timeout: Duration) -> Result<File, LockError> {
        let file = self.open_file(path)?;

        match Backoff::Constant(POLL_INTERVAL)
            .retry_sync(
                || {
                    let result = match self.mode {
                        LockMode::Shared => file.try_lock_shared(),
                        LockMode::Exclusive => file.try_lock(),
                    };
                    match result {
                        Ok(()) => ControlFlow::Break(Ok(())),
                        Err(TryLockError::WouldBlock) => ControlFlow::Continue(()),
                        Err(TryLockError::Error(e)) => ControlFlow::Break(Err(e)),
                    }
                },
                timeout,
            )
            .await
        {
            Ok(Ok(())) => Ok(file),
            Ok(Err(e)) => Err(LockError::Lock(e)),
            Err(()) => Err(LockError::Timeout),
        }
    }

    /// Internal helper that opens (and possibly creates) the lock file with the appropriate mode
    /// but doesn't attempt to acquire the lock.
    fn open_file(self, path: &Path) -> Result<File, LockError> {
        if self.create
            && let Some(parent) = path.parent()
        {
            fs::create_dir_all(parent).map_err(LockError::CreateDir)?;
        }

        // We only need write access if we're creating the file.
        OpenOptions::new()
            .read(true)
            .write(self.create)
            .create(self.create)
            .truncate(false)
            .open(path)
            .map_err(LockError::Open)
    }
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};

    use super::*;

    struct Lock {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
    }

    /// A lock path in a directory that doesn't exist yet.
    #[fixture]
    fn lock() -> Lock {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("test.lock");
        Lock { _dir: dir, path }
    }

    const fn opts(create: bool, mode: LockMode) -> LockOptions {
        LockOptions { create, mode }
    }

    #[rstest]
    fn test_open_creates_file_and_parents(
        lock: Lock,
        #[values(LockMode::Shared, LockMode::Exclusive)] mode: LockMode,
    ) {
        opts(true, mode).open(&lock.path).unwrap();
        assert!(lock.path.exists());
    }

    #[rstest]
    fn test_open_without_create(
        lock: Lock,
        #[values(LockMode::Shared, LockMode::Exclusive)] mode: LockMode,
    ) {
        let err = opts(false, mode).open(&lock.path).unwrap_err();
        assert!(err.is_not_found(), "got: {err}");
        assert!(!lock.path.parent().unwrap().exists(), "must not create parent directories");

        fs::create_dir_all(lock.path.parent().unwrap()).unwrap();
        File::create(&lock.path).unwrap();
        opts(false, mode).open(&lock.path).unwrap();
    }

    #[rstest]
    #[case::shared_shared(LockMode::Shared, LockMode::Shared, true)]
    #[case::shared_exclusive(LockMode::Shared, LockMode::Exclusive, false)]
    #[case::exclusive_shared(LockMode::Exclusive, LockMode::Shared, false)]
    #[case::exclusive_exclusive(LockMode::Exclusive, LockMode::Exclusive, false)]
    #[tokio::test]
    async fn test_wait_contention(
        lock: Lock,
        #[case] held: LockMode,
        #[case] wanted: LockMode,
        #[case] compatible: bool,
    ) {
        let _held = opts(true, held).open(&lock.path).unwrap();
        let result = opts(false, wanted).wait(&lock.path, Duration::from_millis(60)).await;
        if compatible {
            result.unwrap();
        } else {
            assert!(matches!(result, Err(LockError::Timeout)), "got: {result:?}");
        }
    }

    #[rstest]
    #[case::shared_shared(LockMode::Shared, LockMode::Shared, true)]
    #[case::shared_exclusive(LockMode::Shared, LockMode::Exclusive, false)]
    #[case::exclusive_shared(LockMode::Exclusive, LockMode::Shared, false)]
    #[case::exclusive_exclusive(LockMode::Exclusive, LockMode::Exclusive, false)]
    fn test_try_open_contention(
        lock: Lock,
        #[case] held: LockMode,
        #[case] wanted: LockMode,
        #[case] compatible: bool,
    ) {
        let held = opts(true, held).open(&lock.path).unwrap();
        let result = opts(false, wanted).try_open(&lock.path);
        if compatible {
            result.unwrap();
        } else {
            assert!(matches!(result, Err(LockError::WouldBlock)), "got: {result:?}");
        }

        drop(held);
        opts(false, wanted).try_open(&lock.path).unwrap();
    }

    #[rstest]
    #[tokio::test]
    async fn test_wait_after_release(lock: Lock) {
        let held = opts(true, LockMode::Exclusive).open(&lock.path).unwrap();
        let release = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(held);
        };
        let wait = opts(false, LockMode::Exclusive).wait(&lock.path, Duration::from_secs(5));
        let ((), result) = tokio::join!(release, wait);
        result.unwrap();
    }
}
