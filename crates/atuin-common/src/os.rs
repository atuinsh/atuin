//! OS-specific utilities.

use std::ffi::OsString;

use thiserror::Error;
use whoami;

#[cfg(unix)]
pub mod unix;

#[cfg(windows)]
pub mod windows;

#[derive(Debug, Error)]
pub enum HostnameGetError {
    #[error("failed to get the hostname of this computer: {0}")]
    FailedToQuery(whoami::Error),
}

#[derive(Debug, Clone, Copy, Error)]
pub enum HostnameStringConversionError {
    #[error("given string contains an invalid character: {0}")]
    InvalidCharacters(char),

    #[error("the given string is too long. the maximum allowed length is: {0}")]
    TooLong(usize),
}

/// A hostname string which abides by RFC 1123.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DnsHostname(pub OsString);

impl TryFrom<&str> for DnsHostname {
    type Error = HostnameStringConversionError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        // The constraints here reference <https://docs.rs/whoami/latest/whoami/fn.hostname.html>
        let l = value;

        if l.starts_with('-') {
            return Err(HostnameStringConversionError::InvalidCharacters('-'));
        }

        if l.ends_with('-') {
            return Err(HostnameStringConversionError::InvalidCharacters('-'));
        }

        for byte in l.bytes() {
            if !(byte.is_ascii_alphanumeric() || byte == b'-') {
                return Err(HostnameStringConversionError::InvalidCharacters(char::from(byte)));
            }
        }

        Ok(DnsHostname(value.into()))
    }
}

#[cfg(unix)]
pub type Hostname = unix::PosixHostname;

#[cfg(windows)]
pub type Hostname = windows::WindowsHostname;
