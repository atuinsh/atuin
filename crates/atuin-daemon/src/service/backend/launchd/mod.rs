//! Atuin's launchd agent, which only macOS has. Elsewhere, the same API answers that there is no
//! agent, so callers need no `cfg` of their own.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

#[cfg(target_os = "macos")]
pub use macos::Launchd;
#[cfg(not(target_os = "macos"))]
pub use unsupported::Launchd;
