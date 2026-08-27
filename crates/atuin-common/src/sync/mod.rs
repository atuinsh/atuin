//! Synchronization primitives.

mod eager_future_cell;
mod periodic_task;

#[cfg(all(
    feature = "os",
    any(
        windows,
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "fuchsia",
        target_os = "hurd",
        target_os = "cygwin",
        target_os = "illumos",
        target_os = "aix",
        target_vendor = "apple",
    )
))]
mod proc_mutex;

pub use eager_future_cell::{EagerFuture, EagerFutureCell, MutEagerFutureCell, ResultCell};
pub use periodic_task::PeriodicTask;
#[cfg(all(
    feature = "os",
    any(
        windows,
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "fuchsia",
        target_os = "hurd",
        target_os = "cygwin",
        target_os = "illumos",
        target_os = "aix",
        target_vendor = "apple",
    )
))]
pub use proc_mutex::{
    AsyncProcMutex, AsyncProcMutexGuard, ProcMutex, ProcMutexGuard, ProcMutexPool, TryLockError,
};
