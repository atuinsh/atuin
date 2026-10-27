//! systemd, which only Linux has. Elsewhere, the same API answers that systemd isn't there, so
//! callers need no `cfg` of their own.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(not(target_os = "linux"))]
mod unsupported;

#[cfg(target_os = "linux")]
pub(super) use linux::Systemd;
#[cfg(not(target_os = "linux"))]
pub(super) use unsupported::Systemd;
