//! Cli invocation errors.
use std::error::Error;

#[cfg(feature = "sync")]
use atuin_client::record::sync::SyncError;

#[cfg(feature = "sync")]
use crate::i18n::fl;

/// A command that refused to run, with a message that tells the user what to change.
///
/// `main` prints the message without the place in the code that raised it. A refusal that a
/// caller wraps with more context prints as any other error.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Refusal(pub String);

/// A command that failed, as `main` returns it.
// `main` prints it with `Debug`. The messages of a wrong key and of a refusal say how to fix the
// problem, and where it was raised would only bury that.
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

    #[error("{0}")]
    #[debug("{self}")]
    Refusal(Refusal),

    /// Anything else.
    #[error(transparent)]
    #[debug("{_0:?}")]
    Other(Box<dyn Error + Send + Sync>),
}

impl From<eyre::Report> for CliError {
    fn from(report: eyre::Report) -> Self {
        // Only a refusal at the head of the chain becomes `Self::Refusal`. `downcast` also finds a
        // refusal under added context, and converting that refusal would lose the context.
        let report = if matches!(report.chain().next(), Some(err) if err.is::<Refusal>()) {
            match report.downcast::<Refusal>() {
                Ok(refusal) => return Self::Refusal(refusal),
                Err(report) => report,
            }
        } else {
            report
        };
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

#[cfg(test)]
mod tests {
    use eyre::eyre;
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn a_refusal_prints_only_its_message() {
        let report = eyre::Report::from(Refusal("pass --yes".to_owned()));

        assert_eq!(format!("{:?}", CliError::from(report)), "pass --yes");
    }

    #[rstest]
    fn a_wrapped_refusal_keeps_its_context() {
        let report = eyre::Report::from(Refusal("pass --yes".to_owned())).wrap_err("deleting");

        let printed = format!("{:?}", CliError::from(report));

        assert!(printed.contains("deleting"), "{printed}");
        assert!(printed.contains("pass --yes"), "{printed}");
    }

    #[rstest]
    fn other_errors_keep_their_location() {
        let printed = format!("{:?}", CliError::from(eyre!("the daemon failed")));

        assert!(printed.contains("Location:"), "{printed}");
    }
}
