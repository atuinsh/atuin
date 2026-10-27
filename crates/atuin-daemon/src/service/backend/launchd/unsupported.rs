use atuin_client::settings::Daemon as DaemonSettings;

use crate::service::address::DaemonAddress;
use crate::service::backend::{Backend, BackendError, ExternalRestartError};
use crate::service::connection::Connection;
use crate::service::socket_holder::SocketHolder;

/// Atuin's launchd agent, which there is never one of without launchd.
pub enum Launchd {}

impl Launchd {
    pub(in crate::service) const fn for_client(_daemon: &DaemonSettings) -> Option<Self> {
        None
    }

    pub(in crate::service) const fn holds_socket() -> bool {
        false
    }

    #[expect(clippy::unused_async, reason = "the API of macOS's, which awaits launchctl")]
    pub(in crate::service) async fn uninstall() -> Result<(), BackendError> {
        Ok(())
    }

    #[expect(clippy::unnecessary_wraps, reason = "the API of macOS's, where checking in can fail")]
    pub(in crate::service) const fn check_in(
        _daemon: &DaemonSettings,
    ) -> Result<Option<SocketHolder>, BackendError> {
        Ok(None)
    }

    #[expect(clippy::unused_async, reason = "the API of macOS's, which awaits launchctl")]
    pub(in crate::service) async fn make_way_for(_label: &str) {}

    pub(in crate::service) const fn external_job_label() -> Option<String> {
        None
    }

    pub(in crate::service) fn restart_job(
        _label: &str,
    ) -> impl Future<Output = Result<(), ExternalRestartError>> {
        std::future::ready(Err(ExternalRestartError::Missing { manager: "launchd" }))
    }
}

impl Backend for Launchd {
    fn address(&self) -> DaemonAddress {
        match *self {}
    }

    async fn launch(&self) -> Result<(), BackendError> {
        match *self {}
    }

    async fn stop(&self, _service: &Connection) -> Result<(), BackendError> {
        match *self {}
    }

    async fn evict(&self) -> Result<(), BackendError> {
        match *self {}
    }
}
