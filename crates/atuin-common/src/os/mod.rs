//! OS-specific utilities.

#[cfg(any(
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
))]
pub mod file;
pub mod process;

#[cfg(unix)]
pub mod unix;

#[cfg(windows)]
pub mod windows;
