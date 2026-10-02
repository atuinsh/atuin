//! OS-specific utilities.

pub mod fs;

#[cfg(feature = "os")]
pub mod disk;
#[cfg(feature = "os")]
pub mod process;

#[cfg(all(unix, feature = "os"))]
pub mod unix;

#[cfg(all(feature = "os", any(target_os = "macos", all(test, unix))))]
pub mod macos;

#[cfg(all(windows, feature = "os"))]
pub mod windows;
