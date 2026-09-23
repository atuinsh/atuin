//! OS-specific utilities.

pub mod disk;
pub mod fd;
pub mod process;

#[cfg(unix)]
pub mod unix;

#[cfg(windows)]
pub mod windows;
