//! The section of `atuin doctor` that describes desktop capture.
//!
//! Gathering the report changes nothing: it leaves a daemon that isn't running stopped, and it
//! leaves the signed copy of the helper as it is.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use atuin_client::settings::{DesktopCapture, Settings};
use atuin_daemon::grpc::desktop_capture::pb::{State, StatusReply};
use atuin_daemon::grpc::versioning::BUILD_STR;
use atuin_daemon::{DesktopCaptureClient, Signing};
use serde::Serialize;

use super::conflict::Conflict;
use super::setup::{HelperAccess, Logins, helper_access_at, installed_helper};

/// How long the report waits for the daemon's answer.
const DAEMON_TIMEOUT: Duration = Duration::from_secs(3);

/// What `atuin doctor` reports about desktop capture.
#[derive(Debug, Serialize)]
pub struct Report<'a> {
    config: &'a DesktopCapture,
    /// The build of this `atuin`. A running helper always has the daemon's build, so a different
    /// build means that the daemon is running another `atuin`.
    build: &'static str,
    daemon: DaemonReport,
    #[serde(flatten)]
    conflict: Conflict,
    helper: HelperReport,
    /// The variables that capture needs in each session on Linux, as this process sees them.
    #[serde(skip_serializing_if = "Option::is_none")]
    session_variables: Option<BTreeMap<&'static str, Option<String>>>,
}

/// What the daemon says about capture.
#[derive(Debug, Serialize)]
struct DaemonReport {
    state: DaemonState,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Whether the user ran `atuin desktop-capture stop`.
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped: Option<bool>,
}

/// The state of the daemon's supervisor, or `unreachable` when the daemon didn't answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DaemonState {
    Unspecified,
    Disabled,
    NoApps,
    Stopped,
    Starting,
    Running,
    NeedsPermission,
    Failed,
    NoDesktop,
    ScreenLocked,
    Unreachable,
}

impl From<State> for DaemonState {
    fn from(state: State) -> Self {
        match state {
            State::Unspecified => Self::Unspecified,
            State::Disabled => Self::Disabled,
            State::NoApps => Self::NoApps,
            State::Stopped => Self::Stopped,
            State::Starting => Self::Starting,
            State::Running => Self::Running,
            State::NeedsPermission => Self::NeedsPermission,
            State::Failed => Self::Failed,
            State::NoDesktop => Self::NoDesktop,
            State::ScreenLocked => Self::ScreenLocked,
        }
    }
}

/// The helper that capture runs.
#[derive(Debug, Serialize)]
struct HelperReport {
    path: PathBuf,
    exists: bool,
    /// Whether the helper has the Accessibility permission, on macOS.
    #[serde(skip_serializing_if = "Option::is_none")]
    permission: Option<Permission>,
    /// Why the check of the permission failed, when [`Self::permission`] is
    /// [`Permission::Unknown`].
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_error: Option<String>,
    /// Whether the certificate that signs the helper exists, on macOS.
    #[serde(skip_serializing_if = "Option::is_none")]
    certificate: Option<bool>,
}

/// Whether the helper has the Accessibility permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Permission {
    Granted,
    Denied,
    /// The check failed.
    Unknown,
}

/// Describes desktop capture, or returns `None` on a platform where capture doesn't run.
pub async fn report(settings: &Settings) -> Option<Report<'_>> {
    if !cfg!(any(target_os = "linux", target_os = "macos")) {
        return None;
    }
    let session_variables = Logins::for_platform().map(|logins| {
        let values = logins.session_variables().map(|variable| {
            let value = atuin_common::env::var_os(variable.name)
                .map(|value| value.to_string_lossy().into_owned());
            (variable.name, value)
        });
        values.collect()
    });
    Some(Report {
        config: &settings.desktop_capture,
        build: BUILD_STR,
        daemon: DaemonReport::ask(settings).await,
        conflict: Conflict::find(settings).await,
        helper: HelperReport::find(settings).await,
        session_variables,
    })
}

impl Report<'_> {
    /// Explains why another `atuin` will stop desktop capture, and how to stop it. `None` when
    /// capture is off, or when nothing else takes the daemon.
    pub fn conflict(&self) -> Option<String> {
        if !self.config.enabled {
            return None;
        }
        self.conflict.explain()
    }
}

impl DaemonReport {
    /// Asks the daemon for the state of capture, without starting a daemon.
    async fn ask(settings: &Settings) -> Self {
        let status = async {
            let mut client = DesktopCaptureClient::from_settings(settings).await?;
            eyre::Ok(client.status().await?)
        };
        match tokio::time::timeout(DAEMON_TIMEOUT, status).await {
            Ok(Ok(reply)) => Self::from_reply(reply),
            Ok(Err(error)) => Self::unreachable(format!("{error:#}")),
            Err(_) => Self::unreachable(format!(
                "the daemon didn't answer within {} seconds",
                DAEMON_TIMEOUT.as_secs()
            )),
        }
    }

    fn from_reply(reply: StatusReply) -> Self {
        Self {
            state: reply.state().into(),
            error: Some(reply.error).filter(|error| !error.is_empty()),
            stopped: Some(reply.stopped),
        }
    }

    fn unreachable(error: String) -> Self {
        Self {
            state: DaemonState::Unreachable,
            error: Some(error),
            stopped: None,
        }
    }
}

impl HelperReport {
    async fn find(settings: &Settings) -> Self {
        let signing = Signing::for_platform();
        let path = installed_helper(settings, signing.as_ref()).await;
        let exists = tokio::fs::try_exists(&path).await.unwrap_or(false);
        let (permission, permission_error) = match &signing {
            Some(_) if exists => match helper_access_at(&path).await {
                Ok(HelperAccess::Granted) => (Some(Permission::Granted), None),
                Ok(HelperAccess::Denied) => (Some(Permission::Denied), None),
                Err(error) => (Some(Permission::Unknown), Some(format!("{error:#}"))),
            },
            _ => (None, None),
        };
        let certificate = match &signing {
            Some(signing) => Some(signing.has_certificate().await),
            None => None,
        };
        Self {
            path,
            exists,
            permission,
            permission_error,
            certificate,
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    #[rstest]
    #[case::running(State::Running, "", "\"running\"", None)]
    #[case::failed(State::Failed, "boom", "\"failed\"", Some("boom"))]
    #[case::needs_permission(State::NeedsPermission, "", "\"needs_permission\"", None)]
    #[case::no_desktop(State::NoDesktop, "", "\"no_desktop\"", None)]
    #[case::screen_locked(State::ScreenLocked, "", "\"screen_locked\"", None)]
    fn names_the_state_of_the_daemon(
        #[case] state: State,
        #[case] error: &str,
        #[case] name: &str,
        #[case] expected_error: Option<&str>,
    ) {
        let reply = StatusReply {
            state: state.into(),
            error: error.to_owned(),
            ..StatusReply::default()
        };

        let report = DaemonReport::from_reply(reply);

        assert_eq!(serde_json::to_string(&report.state).unwrap(), name);
        assert_eq!(report.error.as_deref(), expected_error);
    }

    /// `permission` is a string in every case, and a failed check adds its error beside it.
    #[rstest]
    #[case::granted(Some(Permission::Granted), None, json!({"permission": "granted"}))]
    #[case::unknown(
        Some(Permission::Unknown),
        Some("no answer"),
        json!({"permission": "unknown", "permission_error": "no answer"})
    )]
    #[case::linux(None, None, json!({}))]
    fn the_permission_is_a_string(
        #[case] permission: Option<Permission>,
        #[case] error: Option<&str>,
        #[case] mut expected: serde_json::Value,
    ) {
        let report = HelperReport {
            path: PathBuf::from("/helper"),
            exists: true,
            permission,
            permission_error: error.map(str::to_owned),
            certificate: None,
        };

        let fields = expected.as_object_mut().unwrap();
        fields.insert("path".to_owned(), json!("/helper"));
        fields.insert("exists".to_owned(), json!(true));
        assert_eq!(serde_json::to_value(&report).unwrap(), expected);
    }
}
