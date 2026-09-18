//! OS-specific utilities.

pub mod fs;

#[cfg(feature = "os")]
pub mod disk;
#[cfg(feature = "os")]
pub mod process;

#[cfg(all(unix, feature = "os"))]
pub mod unix;

#[cfg(all(windows, feature = "os"))]
pub mod windows;
