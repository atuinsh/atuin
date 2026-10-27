use std::collections::BTreeSet;
use std::ffi::CStr;
use std::fs::Permissions;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
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
    ServiceStopCtx, ServiceUninstallCtx,
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
/// launchd holds the agent's socket, keeps listening on it between agents, and starts the next one
/// for the next client. The agent publishes the socket in its pidfile, so clients find it as they
/// find any other daemon.
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
        // for the next client, and a daemon started in its place couldn't bind launchd's socket.
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

    /// Whether launchd listens on the agent's socket, as it does for as long as the agent is
    /// installed.
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
                SocketHolder::Launchd(Listening {
                    listener: DaemonListener::from(listener),
                    socket: Some(SocketFile::kept_fresh(socket_path)),
                })
            }
            // launchd may not create the socket's directory in `$TMPDIR`, which macOS empties at
            // boot. The agent binds the socket itself, as a forked daemon does, until it is next
            // installed, but pinned in the plist, the socket is no default whose directory binding
            // creates.
            Err(launchd::ActivateError::Sandboxed(_)) => {
                if let Some(dir) = socket_path.parent()
                    && !dir.exists()
                {
                    atuin_common::os::unix::create_secure_temp_dir(dir)
                        .map_err(|err| BackendError::new(LaunchdError::SocketDir(err)))?;
                }
                tracing::info!("launchd couldn't place its socket {}", socket_path.display());
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
        let socket = self.daemon.socket_path();
        // Without its directory, launchd loads the agent but can't place the socket.
        socket.create_default_dir_if_needed()?;

        let plist = AgentPlist::for_this_client(socket.into_owned())?;
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
    fn address(&self) -> DaemonAddress {
        DaemonAddress::configured(&self.daemon)
    }

    async fn launch(&self) -> Result<(), BackendError> {
        // launchd would load the agent the user switched off, behind System Settings' back.
        let switched_off = Self::plist_path()
            .is_ok_and(|plist| LoginItem::of_legacy_plist(&plist) == LoginItem::SwitchedOff);
        if switched_off {
            return Err(BackendError::new(LaunchdError::SwitchedOff));
        }
        self.install().await.map_err(BackendError::new)
    }

    async fn stop(&self, service: &Connection) -> Result<(), BackendError> {
        Self::bootout().await.map_err(BackendError::new)?;

        // An older client, or `atuin daemon start`, may have started the agent outside launchd.
        service.request_shutdown().await;

        // launchd leaves its socket's file behind, unlike an agent started outside launchd, which
        // removes it as it exits. Where the agent was found, which the config may have moved the
        // socket from since.
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
    /// Where launchd holds the agent's socket.
    socket: PathBuf,
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
    const INHERITED: [&str; 16] = [
        // The one this client's `TMPDIR` set.
        "TMPDIR",
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

    /// The plist for the agent listening at `socket`, running from the atuin home as a daemon this
    /// client forks does.
    fn for_this_client(socket: PathBuf) -> Result<Self, LaunchdError> {
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
            socket,
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
                .collect(),
        })
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
            // launchd holds the socket the agent was installed with, even once the config moves
            // it, so the agent publishes that one in its pidfile for clients to find. The agent
            // expands it as it does the config's paths, so it must expand back to itself.
            .chain([(
                "ATUIN_DAEMON__SOCKET_PATH".to_owned(),
                Value::from(ShellExpanded::escape(utf8(&self.socket)?)),
            )])
            .collect();
        let socket = Dictionary::from_iter([
            ("SockPathName".to_owned(), path(&self.socket)?),
            // Only the user may connect, as to a socket the daemon binds under its `077` umask.
            ("SockPathMode".to_owned(), Value::from(0o600_i64)),
        ]);
        let sockets = Dictionary::from_iter([(
            Launchd::SOCKET.to_string_lossy().into_owned(),
            Value::Dictionary(socket),
        )]);
        let keep_alive = Dictionary::from_iter([("Crashed".to_owned(), Value::Boolean(true))]);

        let plist = Dictionary::from_iter([
            ("Label".to_owned(), Value::from(Launchd::label().to_qualified_name())),
            ("ProgramArguments".to_owned(), Value::Array(program_arguments)),
            ("EnvironmentVariables".to_owned(), Value::Dictionary(environment)),
            ("WorkingDirectory".to_owned(), path(&self.working_dir)?),
            ("StandardErrorPath".to_owned(), path(&self.stderr)?),
            // The files the agent creates are the user's alone, as a forked daemon's are.
            ("Umask".to_owned(), Value::from(0o077_i64)),
            ("Sockets".to_owned(), Value::Dictionary(sockets)),
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
            socket: PathBuf::from("/tmp/atuin.sock"),
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
    #[case::tmpdir("TMPDIR", true)]
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

    /// The agent reads its pinned socket back through the config's expansion, which must give the
    /// path the client worked out, `$` and all.
    #[rstest]
    fn pinned_socket_expands_back_to_itself(plist: AgentPlist) {
        let socket = PathBuf::from("/tmp/atuin-$__atuin_unset.sock");
        let plist = AgentPlist {
            socket: socket.clone(),
            ..plist
        };
        let xml = Value::from_reader_xml(plist.to_xml().unwrap().as_bytes()).unwrap();
        let pinned = xml
            .as_dictionary()
            .and_then(|plist| plist.get("EnvironmentVariables")?.as_dictionary())
            .and_then(|env| env.get("ATUIN_DAEMON__SOCKET_PATH")?.as_string())
            .unwrap();

        let expanded: PathBuf =
            serde_with::As::<ShellExpanded>::deserialize(serde::de::value::StrDeserializer::<
                serde::de::value::Error,
            >::new(pinned))
            .unwrap();
        assert_eq!(expanded, socket);
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
