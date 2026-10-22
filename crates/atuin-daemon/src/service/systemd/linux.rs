use std::io;
use std::path::{Path, PathBuf};

use atuin_client::settings::Daemon as DaemonSettings;
use atuin_common::os::linux::cgroup::{CgroupV1, CgroupV2};
use tokio::process::Command;

use crate::service::address::DaemonListener;
use crate::service::backend::ExternalRestartError;
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
        // What a unit's process runs shares the unit's cgroup, such as a daemon started from a
        // shell in `tmux.service`, but is no more the unit's to stop than the shell is.
        if !Self::started_this_process() {
            return None;
        }
        match Self::cgroup_of_this_process() {
            Ok(cgroup) => Self::user_unit(&cgroup?),
            Err(err) => {
                tracing::warn!(%err, "can't tell whether a systemd unit runs the daemon");
                None
            }
        }
    }

    /// Have systemd restart the user service `unit`.
    pub(in crate::service) async fn restart_unit(unit: &str) -> Result<(), ExternalRestartError> {
        let command = format!("systemctl --user restart {unit}");
        match Command::new("systemctl").args(["--user", "restart", unit]).status().await {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(ExternalRestartError::Failed { command, status }),
            Err(source) => Err(ExternalRestartError::Spawn { command, source }),
        }
    }

    /// Whether systemd started this very process, as systemd 248 and later tell it. Under an older
    /// one, the daemon can't tell what started it.
    fn started_this_process() -> bool {
        atuin_common::env::var("SYSTEMD_EXEC_PID")
            .is_ok_and(|pid| pid.parse() == Ok(std::process::id()))
    }

    /// This process's cgroup in systemd's own hierarchy: cgroup v2's, or else v1's `name=systemd`.
    fn cgroup_of_this_process() -> io::Result<Option<PathBuf>> {
        Ok(match CgroupV2::active_process()? {
            Some(cgroup) => Some(cgroup.into()),
            None => CgroupV1::active_process("name=systemd")?.map(PathBuf::from),
        })
    }

    /// The systemd user service whose cgroup is `cgroup`, such as `atuin-daemon.service` for
    /// `/user.slice/user-1000.slice/user@1000.service/app.slice/atuin-daemon.service`.
    fn user_unit(cgroup: &Path) -> Option<String> {
        let unit = cgroup.file_name()?.to_str()?;
        // Only a user's unit is one `systemctl --user` reaches. A shell in a terminal, and what it
        // runs, is in a `.scope`.
        let users = cgroup.parent()?.to_str()?.contains("/user@");
        (users && unit.ends_with(".service")).then(|| unit.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::env::MockEnv;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::by_systemd(Some(std::process::id().to_string()), true)]
    #[case::by_what_systemd_started(Some("1".to_owned()), false)]
    #[case::by_anything_else(None, false)]
    fn only_the_process_systemd_started_is_its_units(
        #[case] exec_pid: Option<String>,
        #[case] started: bool,
    ) {
        let env = MockEnv::install();
        if let Some(pid) = exec_pid {
            env.set("SYSTEMD_EXEC_PID", pid);
        }
        assert_eq!(Systemd::started_this_process(), started);
    }

    #[rstest]
    #[case::user_service(
        "/user.slice/user-1000.slice/user@1000.service/app.slice/atuin-daemon.service",
        Some("atuin-daemon.service")
    )]
    #[case::terminal(
        "/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.gnome.Terminal.slice/\
         vte-spawn-1.scope",
        None
    )]
    #[case::login_session("/user.slice/user-1000.slice/session-2.scope", None)]
    #[case::system_service("/system.slice/atuin.service", None)]
    #[case::user_manager("/user.slice/user-1000.slice/user@1000.service", None)]
    #[case::root("/", None)]
    fn only_a_user_service_runs_the_daemon(#[case] cgroup: &str, #[case] unit: Option<&str>) {
        assert_eq!(Systemd::user_unit(Path::new(cgroup)).as_deref(), unit);
    }
}
