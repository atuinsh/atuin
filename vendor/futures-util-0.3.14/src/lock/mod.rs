//! Futures-powered synchronization primitives.
//!
//! This module is only available when the `std` or `alloc` feature of this
//! library is activated, and it is activated by default.

cfg_target_has_atomic! {
    #[cfg(feature = "std")]
    mod mutex;
    #[cfg(feature = "std")]
    pub use self::mutex::{MappedMutexGuard, Mutex, MutexLockFuture, MutexGuard};

    #[cfg(any(feature = "bilock", feature = "sink", feature = "io"))]
    #[cfg_attr(docsrs, doc(cfg(feature = "bilock")))]
    #[cfg_attr(not(feature = "bilock"), allow(unreachable_pub))]
    mod bilock;
    #[cfg(feature = "bilock")]
    #[cfg_attr(docsrs, doc(cfg(feature = "bilock")))]
    pub use self::bilock::{BiLock, BiLockAcquire, BiLockGuard, ReuniteError};
    #[cfg(any(feature = "sink", feature = "io"))]
    #[cfg(not(feature = "bilock"))]
    pub(crate) use self::bilock::BiLock;
}
