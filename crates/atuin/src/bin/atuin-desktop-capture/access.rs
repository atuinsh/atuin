//! Getting the permission to use the accessibility API, and connecting to the API.

use std::fmt;
use std::process::ExitCode;
use std::time::Duration;

use atuin_common::error::WithSources;
use atuin_desktop_capture::link::process::CaptureExit;
use atuin_desktop_capture::{Access, Backend as _, Desktop, Error, Platform as _, Purpose};
use eyre::{WrapErr as _, bail};
use tokio::io::AsyncReadExt as _;

use crate::AccessAction;

/// The context of an error from connecting to the accessibility API.
const CONNECTING: &str = "connecting to the accessibility API";

/// Runs an `access` command, which prints nothing because the TUI of `atuin setup` owns the
/// screen.
pub async fn run(action: AccessAction) -> ExitCode {
    match action {
        AccessAction::Check => match Desktop::access() {
            Access::Granted => ExitCode::SUCCESS,
            Access::Denied => ExitCode::from(CaptureExit::NeedsPermission.code()),
        },
        AccessAction::Request { until_stdin_closes } => {
            if Desktop::access() == Access::Granted {
                return ExitCode::SUCCESS;
            }
            // The system shows its own dialog for this request.
            Desktop::request_access();
            Desktop::open_access_settings();
            let waited = if until_stdin_closes {
                until_closed(tokio::io::stdin(), wait_for_access()).await
            } else {
                wait_for_access().await;
                Waited::Done
            };
            match waited {
                Waited::Done => {
                    Desktop::close_access_request();
                    ExitCode::SUCCESS
                }
                Waited::Closed => ExitCode::FAILURE,
            }
        }
    }
}

/// Whether [`until_closed`] saw its future finish, or its input close first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waited {
    Done,
    Closed,
}

/// Waits for `wait`, unless `input` reaches its end or fails first.
async fn until_closed<R>(mut input: R, wait: impl Future<Output = ()>) -> Waited
where
    R: tokio::io::AsyncRead + Unpin,
{
    let closed = async {
        // Bytes that arrive mean nothing, so the loop reads past them.
        let mut buffer = [0; 64];
        while input.read(&mut buffer).await.is_ok_and(|read| read > 0) {}
    };
    tokio::select! {
        () = wait => Waited::Done,
        () = closed => Waited::Closed,
    }
}

/// Connects to the accessibility API for `purpose`.
///
/// # Errors
///
/// Fails when this process lacks the permission even after asking the system for it, and when
/// the connection fails.
pub async fn connect(purpose: Purpose) -> eyre::Result<Desktop> {
    if Desktop::access() == Access::Denied {
        Desktop::request_access();
        let instructions = Desktop::access_instructions().unwrap_or_default();
        bail!(
            "{} needs the Accessibility permission. {instructions} Then run this again.",
            crate::program_name()
        );
    }
    Desktop::connect(purpose).await.wrap_err(CONNECTING)
}

/// Returns once this process has the permission to use the accessibility API.
async fn wait_for_access() {
    while Desktop::access() == Access::Denied {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Whether a desktop session was there to connect to.
#[allow(
    clippy::large_enum_variant,
    reason = "the caller unpacks the presence right away, so boxing the desktop would only add an \
              allocation. The lint fires only on platforms with a large desktop, so an expect \
              would fail on the others."
)]
pub enum Presence {
    Present(Desktop),
    Absent(NoDesktop),
}

/// The reason that no desktop session could be reached.
#[derive(Debug)]
pub struct NoDesktop(Error);

impl fmt::Display for NoDesktop {
    /// Writes the error followed by each of its sources.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", WithSources(&self.0))
    }
}

/// Connects to the accessibility API for `purpose`, or reports that no desktop session is running.
///
/// # Errors
///
/// Fails when a desktop session is running but the connection fails.
pub async fn connect_if_present(purpose: Purpose) -> eyre::Result<Presence> {
    match Desktop::connect(purpose).await {
        Ok(backend) => Ok(Presence::Present(backend)),
        Err(error @ Error::Unavailable(_)) => Ok(Presence::Absent(NoDesktop(error))),
        Err(error) => Err(error).wrap_err(CONNECTING),
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[derive(Debug, Clone, Copy)]
    enum Cause {
        Unavailable,
        NoAnswer,
    }

    impl Cause {
        fn error(self) -> Error {
            match self {
                Self::Unavailable => Error::Unavailable(std::io::Error::other("no bus").into()),
                Self::NoAnswer => Error::NoAnswer,
            }
        }
    }

    #[rstest]
    #[tokio::test]
    async fn a_closed_input_ends_the_wait() {
        let waited = until_closed(tokio::io::empty(), std::future::pending()).await;
        assert_eq!(waited, Waited::Closed);
    }

    #[rstest]
    #[tokio::test]
    async fn an_open_input_lets_the_wait_finish() {
        // The input stays open while `_writer` lives.
        let (_writer, reader) = tokio::io::duplex(64);
        let waited = until_closed(reader, std::future::ready(())).await;
        assert_eq!(waited, Waited::Done);
    }

    #[rstest]
    #[tokio::test]
    async fn bytes_on_the_input_keep_the_wait_going() {
        let (mut writer, reader) = tokio::io::duplex(64);
        let wait = tokio::time::sleep(std::time::Duration::from_millis(50));
        let writes = async {
            tokio::io::AsyncWriteExt::write_all(&mut writer, b"keys").await.unwrap();
            std::future::pending::<()>().await;
        };
        let waited = tokio::select! {
            waited = until_closed(reader, wait) => waited,
            () = writes => unreachable!(),
        };
        assert_eq!(waited, Waited::Done);
    }

    #[rstest]
    #[case::with_a_source(Cause::Unavailable, "the accessibility service is unavailable: no bus")]
    #[case::without_a_source(Cause::NoAnswer, "the application did not answer")]
    fn no_desktop_names_every_source(#[case] cause: Cause, #[case] expected: &str) {
        let reason = NoDesktop(cause.error());
        assert_eq!(reason.to_string(), expected);
    }
}
