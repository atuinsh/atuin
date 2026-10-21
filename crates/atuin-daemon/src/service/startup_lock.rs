use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use atuin_common::fs::lock::{LockError, LockMode, LockOptions};

/// The lock a client holds while it replaces or starts the service, issued by
/// [`Pidfile::startup_lock`](crate::service::pidfile::Pidfile::startup_lock).
#[derive(Debug)]
pub(super) struct StartupLock {
    path: PathBuf,
    timeout: Duration,
}

#[derive(Debug, thiserror::Error)]
#[error("failed to acquire daemon startup lock at {}: {lock}", .path.display())]
pub struct StartupLockError {
    path: PathBuf,
    lock: LockError,
}

impl StartupLock {
    pub(super) const fn new(path: PathBuf, timeout: Duration) -> Self {
        Self { path, timeout }
    }

    /// Wait for the lock, which is held until the returned file is dropped.
    pub(super) async fn acquire(&self) -> Result<File, StartupLockError> {
        LockOptions {
            create: true,
            mode: LockMode::Exclusive,
        }
        .wait(&self.path, self.timeout)
        .await
        .map_err(|lock| StartupLockError {
            path: self.path.clone(),
            lock,
        })
    }
}
