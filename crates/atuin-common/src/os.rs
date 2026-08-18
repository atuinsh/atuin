//! OS-specific utilities.

use thiserror::Error;
use whoami;

#[cfg(unix)]
pub mod unix;

#[derive(Debug, Error)]
pub enum HostnameGetError {
    #[error("failed to get the hostname of this computer: {0}")]
    FailedToQuery(whoami::Error),
}

#[derive(Debug, Error)]
pub enum UsernameGetError {
    #[error("failed to get the username of the current user: {0}")]
    FailedToQuery(whoami::Error),
}

/// Equivalent to [`whoami::hostname`].
pub fn hostname() -> Result<String, HostnameGetError> {
    whoami::hostname().map_err(HostnameGetError::FailedToQuery)
}

/// Equivalent to [`whoami::username`].
pub fn username() -> Result<String, UsernameGetError> {
    whoami::username().map_err(UsernameGetError::FailedToQuery)
}
