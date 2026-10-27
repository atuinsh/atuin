use std::time::Duration;

use atuin_client::settings::Daemon as DaemonSettings;
use atuin_common::fs::lock::LockError;

use crate::service::address::{DaemonListener, SocketFile};
use crate::service::backend::{BackendError, Launchd, Subproc};
use crate::service::pidfile::{
    AcquireGuardError, LockPidfileError, Pidfile, PidfileGuard, PidfileInfo, PidfileWaitError,
};
use crate::service::run_mode::{ExternalManager, Launch, RunMode};
use crate::service::socket_holder::{ListenError, SocketHolder};

/// What a daemon holds while it is the service: the pidfile, and the socket it serves on.
pub struct ServiceClaim {
    // Fields drop in order, and the socket has to go first: a daemon launched once the pidfile is
    // free binds the same path, and would lose its socket to this one.
    _socket: Option<SocketFile>,
    _pidfile: PidfileGuard,
}

/// Errors returned by [`ServiceClaim::take`].
#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error(transparent)]
    CheckIn(#[from] BackendError),
    #[error(transparent)]
    Pidfile(#[from] AcquireGuardError),
    /// The daemon the agent took over from didn't release the pidfile.
    #[error(transparent)]
    TakeOver(#[from] PidfileWaitError),
    #[error(transparent)]
    Listen(#[from] ListenError),
}

impl ServiceClaim {
    /// Make this process, `launch`ed so, the service `daemon` describes, returning the listener to
    /// serve it on.
    ///
    /// The runtime it runs on keeps a socket it binds fresh until the claim is dropped.
    pub async fn take(
        daemon: &DaemonSettings,
        launch: Launch,
    ) -> Result<(Self, DaemonListener), ClaimError> {
        // Before the pidfile, which records how this process runs.
        let launchd = Launchd::check_in(daemon)?;
        let run_mode = match launchd {
            Some(_) => RunMode::LaunchdAgent,
            None => RunMode::detect(daemon, launch),
        };
        if let RunMode::External(Some(ExternalManager::LaunchdJob { label })) = &run_mode {
            Launchd::make_way_for(label).await;
        }

        // Only the pidfile's holder may clear the sockets a dead daemon left behind.
        let pidfile = Self::take_pidfile(daemon, &PidfileInfo::current(daemon, run_mode)).await?;
        let listening =
            launchd.unwrap_or_else(|| SocketHolder::configured(daemon)).listen(daemon)?;

        let claim = Self {
            _socket: listening.socket,
            _pidfile: pidfile,
        };
        Ok((claim, listening.listener))
    }

    /// Lock the pidfile `daemon` names for this process, which `info` describes.
    ///
    /// Atuin's agent takes it from a daemon a client forked, rather than exit and leave clients to
    /// wait on launchd's socket, with no daemon behind it, for as long as that one runs.
    async fn take_pidfile(
        daemon: &DaemonSettings,
        info: &PidfileInfo,
    ) -> Result<PidfileGuard, ClaimError> {
        /// How long the daemon taken over from may hold the pidfile once it has been killed, which
        /// leaves it only to exit.
        const RELEASED: Duration = Duration::from_secs(1);

        let pidfile = Pidfile::new(&daemon.pidfile_path);
        let held = match pidfile.acquire(info) {
            Err(AcquireGuardError::Lock(
                held @ LockPidfileError {
                    inner: LockError::WouldBlock,
                    ..
                },
            )) => held,
            acquired => return Ok(acquired?),
        };
        // At login, launchd starts the agent on the socket path of any daemon a client forked while
        // nobody was at the console, as over SSH. One from before the pidfile said how it runs would
        // remove launchd's socket as it exits, but clients replace those before installing the agent.
        let forked = || {
            pidfile.info().is_some_and(|holder| match holder.run_mode {
                Some(RunMode::DaemonizeForked) => true,
                Some(RunMode::LaunchdAgent | RunMode::External(_)) | None => false,
            })
        };
        if info.run_mode != Some(RunMode::LaunchdAgent) || !forked() {
            return Err(AcquireGuardError::Lock(held).into());
        }

        tracing::info!("taking over from the daemon a client forked");
        Subproc::new(daemon).terminate().await;
        pidfile.released(RELEASED).await?;
        Ok(pidfile.acquire(info)?)
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::settings::Settings;
    use atuin_common::path::AbsPathBuf;
    use rstest::{fixture, rstest};

    use super::*;

    struct Service {
        _dir: tempfile::TempDir,
        settings: Settings,
    }

    #[fixture]
    fn service() -> Service {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        settings.daemon.pidfile_path =
            AbsPathBuf::new(dir.path().join("atuin-daemon.pid")).unwrap();
        Service {
            _dir: dir,
            settings,
        }
    }

    impl Service {
        fn pidfile(&self) -> Pidfile {
            Pidfile::new(&self.settings.daemon.pidfile_path)
        }

        /// Run a daemon a client forked, played by `sleep`, that holds the pidfile until it exits,
        /// as the returned thread waits for.
        #[cfg(unix)]
        fn run_forked(&self) -> std::thread::JoinHandle<()> {
            use std::process::{Command, Stdio};

            let mut sleep = Command::new("sleep")
                .arg("30")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let info = PidfileInfo {
                pid: sleep.id(),
                ..PidfileInfo::current(&self.settings.daemon, RunMode::DaemonizeForked)
            };
            let pidfile = self.pidfile().acquire(&info).unwrap();
            std::thread::spawn(move || {
                sleep.wait().unwrap();
                drop(pidfile);
            })
        }
    }

    /// launchd puts the agent's socket over that of a daemon a client forked, and the agent takes
    /// over from it.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn the_agent_takes_over_from_a_daemon_a_client_forked(service: Service) {
        let holder = service.run_forked();
        let info = PidfileInfo::current(&service.settings.daemon, RunMode::LaunchdAgent);

        let _taken = ServiceClaim::take_pidfile(&service.settings.daemon, &info).await.unwrap();

        assert_eq!(service.pidfile().info(), Some(info));
        // It released the pidfile as it exited.
        holder.join().unwrap();
    }

    /// Anything else keeps the pidfile from the agent, as a daemon keeps it from anything but the
    /// agent. This test plays the holder: had the claim signalled it, the test would have died.
    #[rstest]
    #[case::launchd_job(
        RunMode::LaunchdAgent,
        Some(RunMode::External(Some(ExternalManager::LaunchdJob {
            label: "homebrew.mxcl.atuin".to_owned(),
        })))
    )]
    #[case::undetected(RunMode::LaunchdAgent, Some(RunMode::External(None)))]
    #[case::older_daemon(RunMode::LaunchdAgent, None)]
    #[case::not_the_agent(RunMode::External(None), Some(RunMode::DaemonizeForked))]
    #[tokio::test]
    async fn only_the_agent_takes_over_and_only_from_a_forked_daemon(
        service: Service,
        #[case] claimant: RunMode,
        #[case] holder: Option<RunMode>,
    ) {
        let held = PidfileInfo {
            run_mode: holder,
            ..PidfileInfo::current(&service.settings.daemon, RunMode::DaemonizeForked)
        };
        let _holder = service.pidfile().acquire(&held).unwrap();
        let info = PidfileInfo::current(&service.settings.daemon, claimant);

        let taken = ServiceClaim::take_pidfile(&service.settings.daemon, &info).await;

        assert!(
            matches!(
                taken,
                Err(ClaimError::Pidfile(AcquireGuardError::Lock(LockPidfileError {
                    inner: LockError::WouldBlock,
                    ..
                })))
            ),
            "{:?}",
            taken.map(drop)
        );
    }
}
