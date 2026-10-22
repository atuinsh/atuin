use atuin_client::settings::Daemon as DaemonSettings;
use atuin_common::os::linux::cgroup::Cgroups;

use crate::service::address::DaemonListener;
use crate::service::socket_holder::{ListenError, Listening};

/// systemd, the service manager.
pub enum Systemd {}

impl Systemd {
    /// The socket systemd passed this process, as `daemon.systemd_socket` says it does.
    pub(in crate::service) fn take_socket(
        daemon: &DaemonSettings,
    ) -> Result<Listening, ListenError> {
        tracing::info!("getting systemd socket");
        let listener = listenfd::ListenFd::from_env()
            .take_unix_listener(0)
            .map_err(ListenError::TakeSystemd)?
            .ok_or(ListenError::NoSystemdSocket)?;

        let socket_path = daemon.socket_path();
        match listener.local_addr() {
            Ok(addr) => match addr.as_pathname() {
                Some(actual_path) => {
                    tracing::info!("listening on systemd socket: {actual_path:?}");
                    if actual_path != socket_path.as_path() {
                        tracing::warn!(
                            "systemd socket is not at configured client path: {:?}",
                            socket_path.as_path(),
                        );
                    }
                }
                None => tracing::warn!(
                    "could not detect systemd socket path, ensure that it's at the configured \
                     path: {:?}, error: systemd socket missing path",
                    socket_path.as_path(),
                ),
            },
            Err(err) => tracing::warn!(
                "could not detect systemd socket path, ensure that it's at the configured path: \
                 {:?}, error: {err:?}",
                socket_path.as_path(),
            ),
        }

        Ok(Listening {
            listener: DaemonListener::from(listener),
            socket: None,
        })
    }

    /// The systemd user service that runs this process, if one does.
    pub(in crate::service) fn unit_of_this_process() -> Option<String> {
        match Cgroups::of_this_process() {
            Ok(cgroups) => Self::user_unit(&cgroups),
            Err(err) => {
                tracing::warn!(%err, "can't tell whether a systemd unit runs the daemon");
                None
            }
        }
    }

    /// The systemd user service that `cgroups` puts a process in, such as `atuin-daemon.service`
    /// in `/user.slice/user-1000.slice/user@1000.service/app.slice/atuin-daemon.service`.
    fn user_unit(cgroups: &Cgroups) -> Option<String> {
        // systemd's own hierarchy: cgroup v2's, or v1's `name=systemd`.
        let path = cgroups.v2().or_else(|| cgroups.v1_named("systemd"))?;
        let unit = path.file_name()?.to_str()?;
        // Only a user's unit is one `systemctl --user` reaches. A shell in a terminal, and what it
        // runs, is in a `.scope`.
        let users = path.parent()?.to_str()?.contains("/user@");
        (users && unit.ends_with(".service")).then(|| unit.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::user_service(
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/atuin-daemon.service\n",
        Some("atuin-daemon.service")
    )]
    #[case::cgroup_v1(
        "12:pids:/user.slice\n1:name=systemd:/user.slice/user-1000.slice/user@1000.service/\
         atuin-daemon.service\n",
        Some("atuin-daemon.service")
    )]
    #[case::terminal(
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.gnome.Terminal.slice/\
         vte-spawn-1.scope\n",
        None
    )]
    #[case::login_session("0::/user.slice/user-1000.slice/session-2.scope\n", None)]
    #[case::system_service("0::/system.slice/atuin.service\n", None)]
    #[case::user_manager("0::/user.slice/user-1000.slice/user@1000.service\n", None)]
    #[case::empty("", None)]
    fn only_a_user_service_runs_the_daemon(#[case] cgroups: &str, #[case] unit: Option<&str>) {
        let cgroups: Cgroups = cgroups.parse().unwrap();
        assert_eq!(Systemd::user_unit(&cgroups).as_deref(), unit);
    }
}
