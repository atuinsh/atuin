//! OS-specific utilities.

#[cfg(feature = "os")]
pub mod disk;
pub mod fd;
#[cfg(feature = "os")]
pub mod process;

#[cfg(all(feature = "os", unix))]
pub mod unix;

#[cfg(all(feature = "os", windows))]
pub mod windows;
