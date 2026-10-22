use crate::service::backend::ExternalRestartError;

/// launchd, which there is none of off macOS.
pub enum Launchd {}

impl Launchd {
    pub(in crate::service) const fn external_job_label() -> Option<String> {
        None
    }

    pub(in crate::service) fn restart_job(
        _label: &str,
    ) -> impl Future<Output = Result<(), ExternalRestartError>> {
        std::future::ready(Err(ExternalRestartError::Missing { manager: "launchd" }))
    }
}
