//! The backends, one per platform. Everything outside this module is platform-neutral.
//!
//! Each backend implements [`Backend`](crate::Backend), and [`Desktop`] names the one for
//! the platform that the crate is compiled for.

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub mod unsupported;

#[cfg(target_os = "linux")]
pub use self::linux::AtSpi as Desktop;
#[cfg(target_os = "macos")]
pub use self::macos::MacAx as Desktop;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub use self::unsupported::Unsupported as Desktop;
