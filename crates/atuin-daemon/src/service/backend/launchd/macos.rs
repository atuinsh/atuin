use std::collections::BTreeSet;
use std::ffi::CStr;
use std::fs::Permissions;
use std::io;
use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt as _, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};

use atuin_client::settings::Daemon as DaemonSettings;
use atuin_common::os::macos::launchd;
use atuin_common::os::macos::login_items::LoginItem;
use atuin_common::os::unix::SecureTempDirError;
use atuin_common::path::{PathBufExt as _, ShellExpanded};
use plist::{Dictionary, Value};
use rustix::process::Pid;
use service_manager::{
    LaunchdServiceManager, RestartPolicy, ServiceInstallCtx, ServiceLabel, ServiceManager,
    ServiceStatus, ServiceStatusCtx, ServiceStopCtx, ServiceUninstallCtx,
};
use tokio::process::Command;
use tracing::warn;

use crate::service::RunMode;
use crate::service::address::{DaemonAddress, DaemonListener, SocketFile};
use crate::service::backend::{Backend, BackendError, ExternalRestartError, Subproc};
use crate::service::connection::Connection;
use crate::service::pidfile::Pidfile;
use crate::service::socket_holder::{Listening, SocketHolder};

/// Represents a backend that runs atuin-daemon as a launchd agent in the user's `gui/<uid>`
/// domain.
///
/// launchd starts the agent at login and again if it crashes. The agent binds the socket and
/// publishes it in the pidfile, just like a subprocess-managed daemon. We don't have launchd manage
/// the socket because this causes it to create the socket's parent directory as root, which causes
/// issues when that's also the directory where pty-proxy sockets are stored -- pty-proxy requires
/// the directory to be owned by the current user.
pub struct Launchd {
    daemon: DaemonSettings,
}

/// Errors from [`Launchd`].
#[derive(Debug, thiserror::Error)]
pub enum LaunchdError {
    #[error(
        "the daemon is switched off in System Settings, under General > Login Items, as `atuin`. \
         Switch it on there to run it"
    )]
    SwitchedOff,
    #[error("`daemon.enabled` is false, so the launchd agent removed itself")]
    Disabled,
    #[error(transparent)]
    AtuinHome(#[from] atuin_common::dirs::HomeError),
    #[error("could not create the daemon's working directory {}: {source}", .path.display())]
    WorkingDir {
        path: PathBuf,
        source: io::Error,
    },
    #[error("could not create the daemon's logs directory {}: {source}", .path.display())]
    LogsDir {
        path: PathBuf,
        source: io::Error,
    },
    #[error("could not create the daemon's socket directory: {0}")]
    SocketDir(#[from] SecureTempDirError),
    #[error("could not read {} for the variables its paths use: {source}", .path.display())]
    Config {
        path: PathBuf,
        source: io::Error,
    },
    #[error("{} is not UTF-8, which a plist requires", .0.display())]
    NotUtf8(PathBuf),
    #[error("could not write the launchd agent's plist: {0}")]
    Plist(#[from] plist::Error),
    #[error("could not find the launchd agent's plist: {0}")]
    PlistPath(io::Error),
    #[error("could not make the launchd agent's plist {} private: {source}", .path.display())]
    PlistMode {
        path: PathBuf,
        source: io::Error,
    },
    #[error("could not rewrite the launchd agent's plist {}: {source}", .path.display())]
    PlistRewrite {
        path: PathBuf,
        source: io::Error,
    },
    #[error("could not install the launchd agent: {0}")]
    Install(io::Error),
    #[error("could not stop the launchd agent: {0}")]
    Bootout(io::Error),
    #[error("could not remove the launchd agent: {0}")]
    Uninstall(io::Error),
}

impl Launchd {
    /// The name of the agent's socket among the `Sockets` in its plist.
    const SOCKET: &CStr = c"atuin";

    /// The agent, if it runs the daemon `daemon` describes for this client.
    pub(in crate::service) fn for_client(daemon: &DaemonSettings) -> Option<Self> {
        // The agent that runs is ours to stop from any of the user's sessions, such as over SSH
        // while someone else has the console. Stopped any other way, launchd would start it again
        // at the next login (or at the next connection for older agents that have launchd-managed
        // sockets), and it would take over from a daemon started in its place.
        let ours = Self::runs_the_daemon(daemon)
            // Without `autostart`, something else manages the daemon, such as home-manager. One
            // installed while it was on is still ours to stop, though.
            || (Self::serves_this_client() && (daemon.autostart || Self::is_installed()));
        ours.then(|| Self {
            daemon: daemon.clone(),
        })
    }

    /// Whether the daemon holding the pidfile `daemon` names is the agent.
    fn runs_the_daemon(daemon: &DaemonSettings) -> bool {
        let pidfile = Pidfile::new(&daemon.pidfile_path);
        pidfile.is_live()
            && pidfile.info().is_some_and(|info| info.run_mode == Some(RunMode::LaunchdAgent))
    }

    /// Whether launchd may listen on the agent's socket, as it does for older agents that have
    /// launchd-managed sockets.
    pub(in crate::service) fn holds_socket() -> bool {
        Self::serves_this_profile() && Self::is_installed()
    }

    /// Remove the agent, if this client has one, for a user turning the daemon off: only stopped,
    /// launchd would start it again at the next login.
    pub(in crate::service) async fn uninstall() -> Result<(), BackendError> {
        if !Self::serves_this_client() {
            return Ok(());
        }
        Self::remove().await
    }

    /// Whether the agent can run the daemon for this client: launchd runs it only for the user at
    /// the console, in that user's home, and for the default profile.
    fn serves_this_client() -> bool {
        Self::serves_this_profile() && Self::is_available()
    }

    /// Whether launchd can run the agent for this client, which it can only for the user logged in
    /// at the console, and only in that user's home.
    ///
    /// Nobody else has a `gui/<uid>` domain, and launchd starts the agent with `HOME` set to the
    /// user's home, whatever this client's is.
    fn is_available() -> bool {
        let console = std::fs::metadata("/dev/console")
            .is_ok_and(|console| console.uid() == atuin_common::os::unix::uid());
        // The user's agents live in `~/Library/LaunchAgents`, `~` being the home launchd uses.
        console
            && LaunchdServiceManager::user().domain.plist_dir().is_ok_and(|agents| {
                agents.parent().and_then(Path::parent)
                    == atuin_common::dirs::try_home_dir().ok().as_deref()
            })
    }

    /// Whether the agent's plist is installed, whether or not launchd has it loaded.
    fn is_installed() -> bool {
        Self::plist_path().is_ok_and(|plist| plist.exists())
    }

    /// Whether the agent serves this profile: launchd has the one `sh.atuin.daemon`, so a profile's
    /// agent, with its own `ATUIN_HOME` or `ATUIN_CONFIG_DIR`, would replace the default one's.
    fn serves_this_profile() -> bool {
        !atuin_common::dirs::atuin_home_is_set() && !atuin_common::dirs::atuin_config_dir_is_set()
    }

    /// Who holds the socket to serve on, if launchd started this process as the agent.
    ///
    /// The agent binds its own socket, or if there's a launchd-managed socket from an older agent,
    /// takes the socket and drops it from the agent's plist.
    ///
    /// Checking in only takes the socket launchd holds, so it may come before the pidfile. An agent
    /// that `daemon.enabled` has since switched off removes itself instead, rather than start again
    /// at every login.
    pub(in crate::service) fn check_in(
        daemon: &DaemonSettings,
    ) -> Result<Option<SocketHolder>, BackendError> {
        let socket_path = daemon.socket_path().into_owned();
        let holder = match Self::activate_socket() {
            Ok(listener) => {
                tracing::info!("listening on launchd socket {}", socket_path.display());
                Self::unpin_socket(daemon);
                SocketHolder::Launchd(Listening {
                    listener: DaemonListener::from(listener),
                    socket: Some(SocketFile::kept_fresh(socket_path)),
                })
            }
            // On an older agent with a launchd-managed socket: launchd couldn't create the socket's
            // directory, so the agent binds the socket itself. Because its plist pins the socket in
            // `ATUIN_DAEMON__SOCKET_PATH`, Atuin sees `daemon.socket_path` as a configured path
            // rather than a default one, and doesn't create the parent directory.
            Err(launchd::ActivateError::Sandboxed(_)) => {
                if let Some(dir) = socket_path.parent()
                    && !dir.exists()
                {
                    atuin_common::os::unix::create_secure_temp_dir(dir)
                        .map_err(|err| BackendError::new(LaunchdError::SocketDir(err)))?;
                }
                tracing::info!("launchd couldn't place its socket {}", socket_path.display());
                Self::unpin_socket(daemon);
                SocketHolder::Daemon
            }
            // The agent binds its own socket when its plist doesn't tell launchd to manage it.
            Err(launchd::ActivateError::NotManaged | launchd::ActivateError::NotFound(_))
                if Self::job_label()
                    .is_some_and(|label| label == Self::label().to_qualified_name()) =>
            {
                SocketHolder::Daemon
            }
            // Started outside launchd, or by a job of someone else's, such as `brew services`.
            Err(launchd::ActivateError::NotManaged | launchd::ActivateError::NotFound(_)) => {
                return Ok(None);
            }
            Err(
                err @ (launchd::ActivateError::AlreadyActivated(_)
                | launchd::ActivateError::Other(_)),
            ) => return Err(BackendError::new(err)),
        };
        if !daemon.enabled {
            Self::leave();
            return Err(BackendError::new(LaunchdError::Disabled));
        }
        Ok(Some(holder))
    }

    /// Reconfigure an agent that uses a launchd-managed socket to manage its own socket instead.
    ///
    /// Older agents used to have launchd manage their socket. This method undoes that part of the
    /// configuration.
    ///
    /// Takes effect on the next login. Until then, the agent keeps the socket launchd holds.
    fn unpin_socket(daemon: &DaemonSettings) {
        // Where the agent binds by default, which is where launchd held the socket unless the
        // config named a different path.
        let default_socket = DaemonSettings {
            socket_path: None,
            ..daemon.clone()
        }
        .socket_path()
        .into_owned();

        // The default socket path that a client without `TMPDIR` would use -- the same subpath, but
        // in `/tmp`.
        let tmpdirless_socket = default_socket
            .strip_prefix(atuin_common::os::unix::tmp_dir())
            .ok()
            .map(|socket| Path::new("/tmp").join(socket));

        let unpinned = || -> Result<bool, LaunchdError> {
            let path = Self::plist_path().map_err(LaunchdError::PlistPath)?;
            let mut plist = Value::from_file(&path)?;
            let Some(dict) = plist.as_dictionary_mut() else {
                return Ok(false);
            };
            if !AgentPlist::unpin_socket(dict, &default_socket, tmpdirless_socket.as_deref()) {
                return Ok(false);
            }

            // Written next to it and renamed over it, so that launchd never loads half a plist.
            let staged = path.with_extension("plist.new");
            let write = || -> io::Result<()> {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&staged)?;
                plist.to_writer_xml(&mut file).map_err(io::Error::other)?;
                file.flush()?;
                file.sync_all()?;
                std::fs::rename(&staged, &path)
            };
            write().map_err(|source| LaunchdError::PlistRewrite { path, source })?;
            Ok(true)
        };

        match unpinned() {
            Ok(true) => tracing::info!(
                "removed the socket from the launchd agent's plist: the agent will manage its own \
                 socket now, starting at next login"
            ),
            Ok(false) => {}
            Err(err) => warn!("could not remove the socket from the launchd agent's plist: {err}"),
        }
    }

    /// Whether launchd is running the agent, whether or not it's listening yet.
    async fn is_running() -> bool {
        let status = tokio::task::spawn_blocking(|| {
            LaunchdServiceManager::user().status(ServiceStatusCtx {
                label: Self::label(),
            })
        })
        .await
        .expect("asking launchd for the agent's status shouldn't panic");
        match status {
            Ok(status) => matches!(status, ServiceStatus::Running),
            Err(err) => {
                warn!(%err, "could not ask launchd whether the agent is running");
                false
            }
        }
    }

    /// Remove the agent, if it serves this profile, for the launchd job `label`, which runs the
    /// daemon in its place: the two would fight for the pidfile at every login, and clients leave
    /// the daemon to the job.
    pub(in crate::service) async fn make_way_for(label: &str) {
        if !Self::serves_this_profile() || !Self::is_installed() {
            return;
        }
        tracing::info!("launchd job `{label}` runs the daemon, so removing Atuin's launchd agent");
        if let Err(err) = Self::remove().await {
            warn!("{err}");
        }
    }

    /// Remove the agent, whether or not it runs.
    async fn remove() -> Result<(), BackendError> {
        tokio::task::spawn_blocking(|| {
            LaunchdServiceManager::user().uninstall(ServiceUninstallCtx {
                label: Self::label(),
            })
        })
        .await
        .expect("uninstalling the launchd agent shouldn't panic")
        .map_err(|err| BackendError::new(LaunchdError::Uninstall(err)))
    }

    /// The label of the launchd job other than the agent that started this process, if one did.
    pub(in crate::service) fn external_job_label() -> Option<String> {
        Self::job_label().filter(|label| *label != Self::label().to_qualified_name())
    }

    /// The label of the launchd job that started this process, if one did.
    fn job_label() -> Option<String> {
        // launchd names the job to the process it spawns, but what that process runs inherits the
        // name, such as the shells of a terminal app launched from the Dock.
        if !Lineage::of_this_process().is_launchd_job() {
            return None;
        }
        let label = atuin_common::env::var_os("XPC_SERVICE_NAME")?.into_string().ok()?;
        (label != "0").then_some(label)
    }

    /// Have launchd restart this user's job `label`: stop it, which launchd does with `SIGTERM`,
    /// and start it again.
    pub(in crate::service) async fn restart_job(label: &str) -> Result<(), ExternalRestartError> {
        let target = format!("gui/{}/{label}", atuin_common::os::unix::uid());
        let command = format!("launchctl kickstart -k {target}");
        match Command::new("launchctl").args(["kickstart", "-k", &target]).status().await {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(ExternalRestartError::Failed { command, status }),
            Err(source) => Err(ExternalRestartError::Spawn { command, source }),
        }
    }

    /// The socket launchd holds for the agent, if launchd started this process as the agent.
    fn activate_socket() -> Result<UnixListener, launchd::ActivateError> {
        let socket = launchd::CheckedInSocket::activate(Self::SOCKET)?;
        Ok(UnixListener::from(OwnedFd::from(socket)))
    }

    /// The agent's label, `sh.atuin.daemon`.
    fn label() -> ServiceLabel {
        ServiceLabel {
            qualifier: Some("sh".to_owned()),
            organization: Some("atuin".to_owned()),
            application: "daemon".to_owned(),
        }
    }

    fn plist_path() -> io::Result<PathBuf> {
        let label = Self::label().to_qualified_name();
        Ok(LaunchdServiceManager::user().domain.plist_dir()?.joined(format!("{label}.plist")))
    }

    /// Remove the agent from within it: its plist, so that launchd doesn't load it at the next
    /// login, and then its job, which stops this process.
    fn leave() {
        // Removed first, as booting the job out stops this process, possibly before it could.
        if let Err(err) = Self::plist_path().and_then(std::fs::remove_file)
            && err.kind() != io::ErrorKind::NotFound
        {
            warn!(%err, "could not remove the launchd agent's plist");
        }

        let domain = LaunchdServiceManager::user().domain;
        let target = format!("{domain}/{}", Self::label().to_qualified_name());
        let bootout = std::process::Command::new("launchctl")
            .args(["bootout", &target])
            // Out of the job's process group, which launchd kills once this process exits.
            .process_group(0)
            // Not waited on: launchd finishes booting the job out once this process has exited.
            .spawn();
        if let Err(err) = bootout {
            warn!(%err, "could not boot out the launchd agent");
        }
    }

    /// Install the agent, replacing any copy of it that is loaded, and so have launchd start it.
    async fn install(&self) -> Result<(), LaunchdError> {
        let plist = AgentPlist::for_this_client()?;
        let ctx = ServiceInstallCtx {
            label: Self::label(),
            contents: Some(plist.to_xml()?),
            // Given `contents`, the crate writes it as the plist and ignores the rest.
            program: plist.exe,
            args: Vec::new(),
            username: None,
            working_directory: None,
            environment: None,
            autostart: true,
            restart_policy: RestartPolicy::Never,
        };
        let path = Self::plist_path().map_err(LaunchdError::PlistPath)?;

        tokio::task::spawn_blocking(move || {
            LaunchdServiceManager::user().install(ctx).map_err(LaunchdError::Install)?;
            // TODO(markovejnovic): service-manager writes the plist under this client's umask, so
            //                      others may read it until it is made private here. Write it
            //                      private in the first place.
            std::fs::set_permissions(&path, Permissions::from_mode(0o600))
                .map_err(|source| LaunchdError::PlistMode { path, source })
        })
        .await
        .expect("installing the launchd agent shouldn't panic")
    }

    /// Stop the agent, which launchd then neither restarts nor starts for the next client until
    /// the next login, and stop listening on its socket.
    async fn bootout() -> Result<(), LaunchdError> {
        tokio::task::spawn_blocking(|| {
            match LaunchdServiceManager::user().stop(ServiceStopCtx {
                label: Self::label(),
            }) {
                // Not installed, as when this client started the agent outside launchd.
                Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
                stopped => stopped.map_err(LaunchdError::Bootout),
            }
        })
        .await
        .expect("booting out the launchd agent shouldn't panic")
    }
}

impl Backend for Launchd {
    /// Where the agent listens.
    ///
    /// If there's a live pidfile, use the socket path in the pidfile; otherwise use the configured
    /// path. Generally these should match, but using the pidfile adds some robustness to
    /// configuration mismatches.
    fn address(&self) -> DaemonAddress {
        let pidfile = Pidfile::new(&self.daemon.pidfile_path);
        pidfile
            .is_live()
            .then(|| pidfile.info()?.socket_path)
            .flatten()
            .map_or_else(|| DaemonAddress::configured(&self.daemon), DaemonAddress::from)
    }

    async fn launch(&self) -> Result<(), BackendError> {
        // launchd would load the agent the user switched off, behind System Settings' back.
        let switched_off = Self::plist_path()
            .is_ok_and(|plist| LoginItem::of_legacy_plist(&plist) == LoginItem::SwitchedOff);
        if switched_off {
            return Err(BackendError::new(LaunchdError::SwitchedOff));
        }
        // launchd starts the agent at login; a client started immediately after login could access
        // it before it starts listening. Make sure we simply wait for it rather than reinstall in
        // this case, since that would cause it to be restarted again.
        if Self::is_running().await {
            tracing::debug!("the launchd agent is running, so waiting for it to listen");
            return Ok(());
        }
        self.install().await.map_err(BackendError::new)
    }

    async fn stop(&self, service: &Connection) -> Result<(), BackendError> {
        Self::bootout().await.map_err(BackendError::new)?;

        // An older client, or `atuin daemon start`, may have started the agent outside launchd.
        service.request_shutdown().await;

        // launchd leaves the file of a socket it held behind, unlike an agent that bound its own,
        // which removes it as it exits. Where the agent was found, which the config may have moved
        // the socket from since.
        if let Err(err) = SocketFile::remove_if_stale(service.address().as_ref()) {
            warn!("{err}");
        }
        Ok(())
    }

    async fn evict(&self) -> Result<(), BackendError> {
        // Booted out, the agent stops, and launchd stops listening on the socket a daemon started
        // in its place has to bind.
        Self::bootout().await.map_err(BackendError::new)?;
        Subproc::new(&self.daemon).evict().await
    }
}

/// Where a process stands among the others, as far as telling a launchd job's own process from what
/// it runs.
#[derive(Debug, Clone, Copy)]
struct Lineage {
    pid: Pid,
    /// [`None`] for a process with no parent, as only the kernel's are.
    parent: Option<Pid>,
    group: Pid,
}

impl Lineage {
    fn of_this_process() -> Self {
        Self {
            pid: rustix::process::getpid(),
            parent: rustix::process::getppid(),
            group: rustix::process::getpgrp(),
        }
    }

    /// Whether launchd spawned the process as a job's.
    ///
    /// launchd spawns a job's process as its child, leading a process group of its own. It also
    /// adopts a process whose parent exited, as `(atuin daemon start &)` leaves one, but that one
    /// stays in the group it was started in.
    fn is_launchd_job(self) -> bool {
        self.parent.is_some_and(Pid::is_init) && self.group == self.pid
    }
}

/// The plist of a launchd agent that runs this client's executable.
struct AgentPlist {
    exe: PathBuf,
    working_dir: PathBuf,
    /// Where the agent's stderr goes, for the errors it exits with: it logs to its own file.
    stderr: PathBuf,
    /// The variables of this client's environment that the agent inherits.
    environment: Vec<(String, String)>,
}

impl AgentPlist {
    /// How long launchd waits for the agent to exit before it kills it, and so how long stopping
    /// one that hangs takes.
    const EXIT_TIMEOUT_SECS: i64 = 10;

    /// The variables the agent reads besides atuin's settings and the XDG directories, which
    /// launchd doesn't set: it starts the agent with only `HOME`, `USER`, `SHELL`, a `TMPDIR` and
    /// a bare `PATH`.
    // TODO(markovejnovic): Ideally we'd have a richer environment utility that would allow us to
    //                      categorize between "config" env vars and "ephemeral env vars".
    ///
    /// `TMPDIR` isn't included here since we always set it to [`Self::tmpdir_env_pair`].
    const INHERITED: [&str; 15] = [
        // The programs it runs, such as `git` and `node`.
        "PATH",
        // The proxies sync goes through.
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
        // Where the harnesses whose sessions it captures keep them.
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "OPENCODE_DB",
        "OPENCODE_DISABLE_CHANNEL_DB",
        "PI_CODING_AGENT_DIR",
        "PI_CODING_AGENT_SESSION_DIR",
    ];

    /// The `ATUIN_` variables that aren't settings: those a shell sets to describe itself and the
    /// command it runs, and secrets, which would be written to the plist.
    const NOT_INHERITED: [&str; 23] = [
        "ATUIN_SESSION",
        "ATUIN_HISTORY_ID",
        "ATUIN_SHLVL",
        "ATUIN_SHELL",
        "ATUIN_PID",
        "ATUIN_QUERY",
        "ATUIN_STTY",
        "ATUIN_PREEXEC_BACKEND",
        "ATUIN_COMMAND_LINE",
        "ATUIN_TERMINAL",
        "ATUIN_TMUX_POPUP",
        "ATUIN_TMUX_POPUP_WIDTH",
        "ATUIN_TMUX_POPUP_HEIGHT",
        "ATUIN_PTY_PROXY_ACTIVE",
        "ATUIN_PTY_PROXY_SOCKET",
        "ATUIN_PTY_PROXY_FAILED",
        "ATUIN_HISTORY_AUTHOR",
        "ATUIN_HISTORY_AUTHOR_KIND",
        "ATUIN_HISTORY_INTENT",
        "ATUIN_PASSWORD",
        "ATUIN_ENCRYPTION_KEY",
        "ATUIN_GITHUB_TOKEN",
        "ATUIN_DB_URI",
    ];

    /// The plist for the agent, running from the atuin home as a daemon this client forks does.
    fn for_this_client() -> Result<Self, LaunchdError> {
        let working_dir = PathBuf::from(atuin_common::dirs::try_atuin_home()?);
        std::fs::create_dir_all(&working_dir).map_err(|source| LaunchdError::WorkingDir {
            path: working_dir.clone(),
            source,
        })?;
        // launchd opens the agent's stderr before it runs the agent, but doesn't create the
        // directory it goes in.
        let logs = atuin_common::dirs::logs_dir();
        std::fs::create_dir_all(&logs).map_err(|source| LaunchdError::LogsDir {
            path: logs.clone(),
            source,
        })?;
        let config_variables = Self::config_variables()?;
        Ok(Self {
            exe: atuin_common::os::process::current_exe(),
            working_dir,
            stderr: logs.joined("daemon.stderr.log"),
            // A plist holds only UTF-8, and `vars` would panic on anything else.
            environment: std::env::vars_os()
                .filter_map(|(name, value)| {
                    let name = name
                        .into_string()
                        .ok()
                        .filter(|name| Self::is_inherited(name, &config_variables))?;
                    Some((name, value.into_string().ok()?))
                })
                .chain([Self::tmpdir_env_pair()?])
                .collect(),
        })
    }

    /// The `TMPDIR` key-value pair for [`Self::environment`]. The value is the temporary directory
    /// that this client uses.
    ///
    /// The daemon and client use `$TMPDIR` for the default socket path. Setting it explicitly in
    /// the plist makes sure they match.
    fn tmpdir_env_pair() -> Result<(String, String), LaunchdError> {
        let tmp = atuin_common::os::unix::tmp_dir().into_path_buf();
        let tmp =
            tmp.into_os_string().into_string().map_err(|tmp| LaunchdError::NotUtf8(tmp.into()))?;
        Ok(("TMPDIR".to_owned(), tmp))
    }

    /// The variables the paths in the config expand, which the agent reads again: without them,
    /// it couldn't load the config. Set as of this install, which `atuin daemon restart` repeats.
    fn config_variables() -> Result<BTreeSet<String>, LaunchdError> {
        let path = atuin_common::dirs::config_path("config.toml");
        match std::fs::read_to_string(&path) {
            // Over the whole file, a variable named outside a path, such as in a comment, is
            // inherited for nothing, but none that a path expands is missed.
            Ok(config) => Ok(ShellExpanded::variables(&config)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(BTreeSet::new()),
            Err(source) => Err(LaunchdError::Config { path, source }),
        }
    }

    /// Whether the agent inherits the variable `name` from this client's environment, whose config
    /// expands `config_variables`.
    fn is_inherited(name: &str, config_variables: &BTreeSet<String>) -> bool {
        !Self::NOT_INHERITED.contains(&name)
            && (name.starts_with("ATUIN_")
                || name.starts_with("XDG_")
                || Self::INHERITED.contains(&name)
                || config_variables.contains(name))
    }

    fn to_xml(&self) -> Result<String, LaunchdError> {
        let utf8: fn(&Path) -> Result<&str, LaunchdError> =
            |path| path.to_str().ok_or_else(|| LaunchdError::NotUtf8(path.to_owned()));
        let path = |path: &Path| utf8(path).map(Value::from);

        let program_arguments =
            std::iter::once(path(&self.exe)?).chain(["daemon", "start"].map(Value::from)).collect();
        let environment = self
            .environment
            .iter()
            .map(|(name, value)| (name.clone(), Value::from(value.as_str())))
            .collect();
        let keep_alive = Dictionary::from_iter([("Crashed".to_owned(), Value::Boolean(true))]);

        let plist = Dictionary::from_iter([
            ("Label".to_owned(), Value::from(Launchd::label().to_qualified_name())),
            ("ProgramArguments".to_owned(), Value::Array(program_arguments)),
            ("EnvironmentVariables".to_owned(), Value::Dictionary(environment)),
            ("WorkingDirectory".to_owned(), path(&self.working_dir)?),
            ("StandardErrorPath".to_owned(), path(&self.stderr)?),
            // The files the agent creates are the user's alone, as a forked daemon's are.
            ("Umask".to_owned(), Value::from(0o077_i64)),
            ("RunAtLoad".to_owned(), Value::Boolean(true)),
            // Restart an agent that crashed, but not one that exited, as an agent does when it
            // finds another holding the pidfile: launchd would restart that one every
            // `ThrottleInterval` for as long as the other runs.
            ("KeepAlive".to_owned(), Value::Dictionary(keep_alive)),
            // Left unset, launchd throttles the agent's CPU and I/O, which a shell waits on.
            ("ProcessType".to_owned(), Value::from("Interactive")),
            ("ExitTimeOut".to_owned(), Value::from(Self::EXIT_TIMEOUT_SECS)),
        ]);

        let mut xml = Vec::new();
        Value::Dictionary(plist).to_writer_xml(&mut xml)?;
        Ok(String::from_utf8(xml).expect("a plist's XML is UTF-8"))
    }

    /// Drop the launchd-managed socket from `plist`, which older agents used to use.
    ///
    /// The old plist also put the socket path in the `ATUIN_DAEMON__SOCKET_PATH` environment
    /// variable. This function removes that when it matches the default socket path, so Atuin will
    /// see `daemon.socket_path` as a default path and thus create its parent directory.
    ///
    /// The socket path is also unpinned when it matches `tmpdirless_socket` and the plist doesn't
    /// set `TMPDIR`. Such a plist was installed by a client without `TMPDIR`, which would have used
    /// `tmpdirless_socket` as its default socket path. Additionally, in this case, we set `TMPDIR`
    /// to `/tmp` (as is done for new plists) so the agent keeps using the same path.
    fn unpin_socket(
        plist: &mut Dictionary,
        default_socket: &Path,
        tmpdirless_socket: Option<&Path>,
    ) -> bool {
        if plist.remove("Sockets").is_none() {
            return false;
        }

        let Some(env) = plist.get_mut("EnvironmentVariables").and_then(Value::as_dictionary_mut)
        else {
            return true;
        };

        let pin = env.get("ATUIN_DAEMON__SOCKET_PATH").and_then(Value::as_string);
        let pins = |socket: Option<&Path>| {
            pin.is_some()
                && pin == socket.and_then(Path::to_str).map(ShellExpanded::escape).as_deref()
        };
        let pins_default = pins(Some(default_socket));
        let pins_tmpdirless = !env.contains_key("TMPDIR") && pins(tmpdirless_socket);

        if pins_default || pins_tmpdirless {
            env.remove("ATUIN_DAEMON__SOCKET_PATH");
        }
        if pins_tmpdirless {
            env.insert("TMPDIR".to_owned(), Value::from("/tmp"));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;

    use rstest::{fixture, rstest};

    use super::*;

    #[fixture]
    fn plist() -> AgentPlist {
        AgentPlist {
            exe: PathBuf::from("/opt/atuin"),
            working_dir: PathBuf::from("/"),
            stderr: PathBuf::from("/tmp/daemon.stderr.log"),
            environment: Vec::new(),
        }
    }

    #[rstest]
    #[case::atuin_setting("ATUIN_DAEMON__SOCKET_PATH", true)]
    #[case::atuin_config_dir("ATUIN_CONFIG_DIR", true)]
    #[case::setting_named_like_shell_state("ATUIN_SESSION_PATH", true)]
    #[case::nested_setting_named_like_shell_state("ATUIN_PTY_PROXY__ENABLED", true)]
    #[case::shell_session("ATUIN_SESSION", false)]
    #[case::shell_command("ATUIN_HISTORY_ID", false)]
    #[case::secret("ATUIN_PASSWORD", false)]
    #[case::xdg("XDG_DATA_HOME", true)]
    #[case::tmpdir_set_separately("TMPDIR", false)]
    #[case::path("PATH", true)]
    #[case::proxy("https_proxy", true)]
    #[case::harness_config("CLAUDE_CONFIG_DIR", true)]
    #[case::unread("EDITOR", false)]
    #[case::login_session("SSH_AUTH_SOCK", false)]
    #[case::launchd_label("XPC_SERVICE_NAME", false)]
    #[case::atuin_without_separator("ATUINX", false)]
    #[case::tmpdir_prefix("TMPDIR_OLD", false)]
    #[case::config_path_variable("HISTDIR", true)]
    #[case::secret_in_the_config("ATUIN_PASSWORD", false)]
    fn agent_inherits_only_the_variables_it_reads(#[case] name: &str, #[case] inherited: bool) {
        // A config whose paths expand these.
        let config_variables = BTreeSet::from(["HISTDIR".to_owned(), "ATUIN_PASSWORD".to_owned()]);
        assert_eq!(AgentPlist::is_inherited(name, &config_variables), inherited);
    }

    /// launchd holds no socket for the agent, and so creates no directory for one as root: the
    /// agent binds its own, where the config says.
    #[rstest]
    fn launchd_holds_no_socket_for_the_agent(plist: AgentPlist) {
        let xml = Value::from_reader_xml(plist.to_xml().unwrap().as_bytes()).unwrap();
        let xml = xml.as_dictionary().unwrap();
        assert!(!xml.contains_key("Sockets"));
        let env = xml.get("EnvironmentVariables").and_then(Value::as_dictionary).unwrap();
        assert!(!env.contains_key("ATUIN_DAEMON__SOCKET_PATH"));
    }

    /// An agent's plist from before agents bound their own socket, holding `socket` and pinning
    /// it in its environment as they did.
    fn pinned_plist(socket: &str) -> Dictionary {
        let socket_dict = Dictionary::from_iter([
            ("SockPathName".to_owned(), Value::from(socket)),
            ("SockPathMode".to_owned(), Value::from(0o600_i64)),
        ]);
        let env = Dictionary::from_iter([
            ("ATUIN_DAEMON__SOCKET_PATH".to_owned(), Value::from(ShellExpanded::escape(socket))),
            ("PATH".to_owned(), Value::from("/usr/bin:/bin")),
        ]);
        Dictionary::from_iter([
            ("Label".to_owned(), Value::from("sh.atuin.daemon")),
            ("EnvironmentVariables".to_owned(), Value::Dictionary(env)),
            (
                "Sockets".to_owned(),
                Value::Dictionary(Dictionary::from_iter([(
                    "atuin".to_owned(),
                    Value::Dictionary(socket_dict),
                )])),
            ),
        ])
    }

    /// Unpinned, an old plist holds no socket. Its pin goes only if it was the default socket,
    /// which the agent then binds by default, creating its directory; one the config named stays.
    /// The rest of the plist is left as it was.
    #[rstest]
    #[case::default_socket("/var/folders/x/T/atuin-501/atuin.sock", false)]
    #[case::default_socket_with_dollar("/var/folders/x/T/a$b/atuin-501/atuin.sock", false)]
    #[case::configured_socket("/Users/me/atuin.sock", true)]
    fn unpinning_drops_the_socket_and_only_a_default_pin(
        #[case] socket: &str,
        #[case] pin_kept: bool,
    ) {
        let default_socket = if pin_kept {
            Path::new("/var/folders/x/T/atuin-501/atuin.sock")
        } else {
            Path::new(socket)
        };
        let mut plist = pinned_plist(socket);

        assert!(AgentPlist::unpin_socket(&mut plist, default_socket, None));

        assert!(!plist.contains_key("Sockets"));
        assert_eq!(plist.get("Label").and_then(Value::as_string), Some("sh.atuin.daemon"));
        let env = plist.get("EnvironmentVariables").and_then(Value::as_dictionary).unwrap();
        assert_eq!(env.contains_key("ATUIN_DAEMON__SOCKET_PATH"), pin_kept);
        assert_eq!(env.get("PATH").and_then(Value::as_string), Some("/usr/bin:/bin"));
    }

    /// A client without `TMPDIR` pinned the socket under `/tmp`. launchd gives the agent a
    /// `TMPDIR` of its own. Unpinning removes the pin and sets `TMPDIR` to `/tmp` in the plist. The
    /// agent's default socket is then the one that the pin named.
    ///
    /// A plist that set `TMPDIR` came from a client with `TMPDIR`. A pin under `/tmp` in that plist
    /// is one that the config named, and it stays.
    #[rstest]
    #[case::installed_without_tmpdir(None, false, Some("/tmp"))]
    #[case::installed_with_tmpdir(Some("/Users/me/tmp"), true, Some("/Users/me/tmp"))]
    fn unpinning_a_tmp_pin_sets_tmpdir_for_a_client_without_one(
        #[case] tmpdir: Option<&str>,
        #[case] pin_kept: bool,
        #[case] tmpdir_after: Option<&str>,
    ) {
        let mut plist = pinned_plist("/tmp/atuin-501/atuin.sock");
        if let Some(tmpdir) = tmpdir {
            let env = plist.get_mut("EnvironmentVariables").and_then(Value::as_dictionary_mut);
            env.unwrap().insert("TMPDIR".to_owned(), Value::from(tmpdir));
        }

        assert!(AgentPlist::unpin_socket(
            &mut plist,
            Path::new("/var/folders/x/T/atuin-501/atuin.sock"),
            Some(Path::new("/tmp/atuin-501/atuin.sock")),
        ));

        let env = plist.get("EnvironmentVariables").and_then(Value::as_dictionary).unwrap();
        assert_eq!(env.contains_key("ATUIN_DAEMON__SOCKET_PATH"), pin_kept);
        assert_eq!(env.get("TMPDIR").and_then(Value::as_string), tmpdir_after);
    }

    /// The agent runs with the temporary directory that this client uses. That directory is
    /// `/tmp` when `TMPDIR` is unset or relative.
    #[rstest]
    #[case::set(Some("/var/folders/x/T/"), "/var/folders/x/T/")]
    #[case::unset(None, "/tmp")]
    #[case::relative(Some("tmp"), "/tmp")]
    fn the_agent_gets_this_clients_tmpdir(#[case] tmpdir: Option<&str>, #[case] expected: &str) {
        let env = atuin_common::env::MockEnv::install();
        match tmpdir {
            Some(tmpdir) => env.set("TMPDIR", tmpdir),
            None => env.remove("TMPDIR"),
        }
        let (name, value) = AgentPlist::tmpdir_env_pair().unwrap();
        assert_eq!((name.as_str(), value.as_str()), ("TMPDIR", expected));
    }

    /// A plist that holds no socket is left alone, and reported unchanged, so it isn't rewritten.
    #[rstest]
    fn unpinning_a_plist_without_a_socket_changes_nothing(plist: AgentPlist) {
        let xml = Value::from_reader_xml(plist.to_xml().unwrap().as_bytes()).unwrap();
        let mut dict = xml.into_dictionary().unwrap();
        let before = dict.clone();

        assert!(!AgentPlist::unpin_socket(&mut dict, Path::new("/tmp/atuin-501/atuin.sock"), None));
        assert_eq!(dict, before);
    }

    #[rstest]
    #[case::launchd_job(500, 1, 500, true)]
    #[case::run_from_a_shell(500, 400, 500, false)]
    #[case::adopted_by_launchd(500, 1, 400, false)]
    fn only_a_process_launchd_spawned_is_a_job(
        #[case] pid: i32,
        #[case] parent: i32,
        #[case] group: i32,
        #[case] job: bool,
    ) {
        let lineage = Lineage {
            pid: Pid::from_raw(pid).unwrap(),
            parent: Pid::from_raw(parent),
            group: Pid::from_raw(group).unwrap(),
        };
        assert_eq!(lineage.is_launchd_job(), job);
    }

    /// An app launched from the Dock passes its launchd label on to what it runs, such as its
    /// terminal's shells, and so does a job that runs a tmux server. Taken for the daemon's job,
    /// `atuin daemon restart` would restart the app.
    #[rstest]
    #[case::app("application.com.microsoft.VSCode.81468291.81468297")]
    #[case::job("local.tmux")]
    fn a_label_this_process_inherited_is_no_job_of_its_own(#[case] label: &str) {
        let env = atuin_common::env::MockEnv::install();
        env.set("XPC_SERVICE_NAME", label);
        // The test runner spawned this test, not launchd.
        assert_eq!(Launchd::external_job_label(), None);
    }

    /// A lossy path would point launchd at an executable that doesn't exist.
    #[rstest]
    fn non_utf8_executable_is_rejected_rather_than_mangled(plist: AgentPlist) {
        let exe = PathBuf::from(std::ffi::OsStr::from_bytes(b"/opt/\xffatuin"));
        let plist = AgentPlist {
            exe: exe.clone(),
            ..plist
        };
        assert!(matches!(plist.to_xml(), Err(LaunchdError::NotUtf8(path)) if path == exe));
    }
}
