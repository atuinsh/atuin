//! Cli invocation errors.
use std::error::Error;

#[cfg(feature = "sync")]
use atuin_client::record::sync::SyncError;

#[cfg(feature = "sync")]
use crate::i18n::fl;

/// A command that failed, as `main` returns it.
// `main` prints it with `Debug`. A wrong key's message says how to fix it, and where it was raised
// would only bury that.
#[derive(thiserror::Error, derive_more::Debug)]
pub enum CliError {
    /// The local key can't decrypt the records on the server.
    #[cfg(feature = "sync")]
    #[error("{}", SyncError::WrongKey)]
    #[debug("{self}")]
    WrongKey,

    /// `atuin login` was given a key that can't decrypt the records on the server, and logged out
    /// again.
    #[cfg(feature = "sync")]
    #[error("{}", fl!("login-wrong-key-body"))]
    #[debug("{self}")]
    LoginWrongKey,

    /// Anything else.
    #[error(transparent)]
    #[debug("{_0:?}")]
    Other(Box<dyn Error + Send + Sync>),
}

impl From<eyre::Report> for CliError {
    fn from(report: eyre::Report) -> Self {
        #[cfg(feature = "sync")]
        for err in report.chain() {
            if matches!(err.downcast_ref(), Some(SyncError::WrongKey)) {
                return Self::WrongKey;
            }
            if matches!(err.downcast_ref(), Some(Self::LoginWrongKey)) {
                return Self::LoginWrongKey;
            }
        }
        Self::Other(report.into())
    }
}
