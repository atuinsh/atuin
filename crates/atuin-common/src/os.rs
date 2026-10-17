//! OS-specific utilities.

pub mod disk;
pub mod fs;
pub mod process;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(unix)]
pub mod unix;

#[cfg(windows)]
pub mod windows;
