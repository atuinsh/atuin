use std::ffi::{CStr, OsString};
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use atuin_client::settings::{Daemon as DaemonSettings, Settings};
use atuin_common::os::macos::launchd;
use plist::{Dictionary, Value};
use service_manager::{
    LaunchdServiceManager, RestartPolicy, ServiceInstallCtx, ServiceLabel, ServiceManager,
    ServiceUninstallCtx,
};
use tracing::warn;

use crate::agent::AgentWorkingDir;
use crate::agent::backend::{AGENT_ARGS, AgentHandle, Backend, SpawnError, StopError, Subproc};
use crate::client::DaemonAddress;

/// Represents a backend that runs atuin-daemon as a launchd agent in the user's `gui/<uid>`
/// domain.
///
/// launchd holds the agent's socket at `daemon.socket_path`, where the daemon would bind it
/// itself, so clients find it as they find any other. It keeps listening between agents, and
/// starts the next one for the next client.
pub struct Launchd {
    daemon: DaemonSettings,
    /// Runs the agent when launchd won't.
    fallback: Subproc,
    manager: LaunchdServiceManager,
    working_dir: AgentWorkingDir,
}

impl Launchd {
    /// The name of the agent's socket among the `Sockets` in its plist.
    const SOCKET: &CStr = c"atuin";

    /// The flag that tells `atuin daemon start` that launchd holds its socket.
    const FLAG: &str = "--launchd";

    pub fn new(settings: &Settings) -> Self {
        Self {
            daemon: settings.daemon.clone(),
            fallback: Subproc::new(settings),
            manager: LaunchdServiceManager::user(),
            working_dir: AgentWorkingDir::for_settings(settings),
        }
    }

    /// Whether launchd can run the agent, which it can only for the user logged in at the
    /// console: nobody else has a `gui/<uid>` domain.
    pub fn is_available() -> bool {
        std::fs::metadata("/dev/console")
            .is_ok_and(|console| console.uid() == atuin_common::os::unix::uid())
    }

    /// Take the socket launchd holds for the agent, which only the agent launchd started can.
    pub fn activate_socket() -> Result<UnixListener, launchd::ActivateError> {
        // A socket at a path has the one descriptor.
        let fd = launchd::CheckedInSocket::activate(Self::SOCKET)?.into_iter().next();
        Ok(UnixListener::from(fd.expect("an activated socket has a descriptor")))
    }

    /// The agent's label, `sh.atuin.daemon`.
    fn label() -> ServiceLabel {
        ServiceLabel {
            qualifier: Some("sh".to_owned()),
            organization: Some("atuin".to_owned()),
            application: "daemon".to_owned(),
        }
    }

    /// Install the agent, replacing any copy of it that is loaded, and so have launchd start it.
    async fn install(&self) -> io::Result<()> {
        let socket = self.daemon.socket_path();
        // Without its directory, launchd loads the agent but can't place the socket.
        socket.create_default_dir_if_needed().map_err(io::Error::other)?;

        let plist = AgentPlist::for_this_client(self.working_dir, socket.into_owned())?;
        let ctx = ServiceInstallCtx {
            label: Self::label(),
            contents: Some(plist.to_xml()?),
            // Given `contents`, the crate writes it as the plist and ignores the rest.
            program: plist.exe,
            args: AGENT_ARGS.map(OsString::from).into(),
            username: None,
            working_directory: None,
            environment: None,
            autostart: true,
            restart_policy: RestartPolicy::Never,
        };

        let manager = self.manager.clone();
        tokio::task::spawn_blocking(move || manager.install(ctx))
            .await
            .expect("installing the launchd agent shouldn't panic")
    }

    /// Stop the agent and remove it, so that launchd doesn't start it again at the next login.
    async fn uninstall(&self) -> io::Result<()> {
        let manager = self.manager.clone();
        tokio::task::spawn_blocking(move || {
            manager.uninstall(ServiceUninstallCtx {
                label: Self::label(),
            })
        })
        .await
        .expect("uninstalling the launchd agent shouldn't panic")
    }

    /// Remove the socket's file that launchd leaves behind, once nothing listens on it.
    ///
    /// An agent started outside launchd still listens on it until it exits, and removes it itself.
    fn remove_stale_socket(&self) {
        let socket = self.daemon.socket_path();
        let path = socket.as_path();
        let stale = matches!(atuin_common::os::unix::socket_in_use(path), Ok(false))
            // `socket_in_use` is `Ok(false)` for a regular file at a misconfigured path too.
            && std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_socket());
        if stale
            && let Err(err) = std::fs::remove_file(path)
            && err.kind() != io::ErrorKind::NotFound
        {
            warn!(%err, path = %path.display(), "could not remove the launchd agent's socket");
        }
    }
}

impl Backend for Launchd {
    fn address(&self) -> DaemonAddress {
        DaemonAddress::configured(&self.daemon)
    }

    async fn launch(&self) -> Result<(), SpawnError> {
        // launchd refuses the agent when the user has turned it off in Login Items. Start it the
        // way clients always have, rather than not at all.
        if let Err(err) = self.install().await {
            warn!(%err, "could not run the daemon as a launchd agent; starting it directly");
            self.fallback.launch().await?;
        }
        Ok(())
    }

    async fn stop(&self, agent: &AgentHandle) -> Result<(), StopError> {
        // Once removed, launchd neither restarts the agent as it exits nor starts another for the
        // next client.
        self.uninstall().await.map_err(StopError::Launchd)?;

        // An older client, or `atuin daemon start`, may have started the agent outside launchd.
        agent.request_shutdown().await;

        self.remove_stale_socket();
        Ok(())
    }
}

/// The plist of a launchd agent that runs this client's executable.
struct AgentPlist {
    exe: PathBuf,
    working_dir: PathBuf,
    /// Where launchd holds the agent's socket.
    socket: PathBuf,
    /// The variables of this client's environment that the agent inherits.
    environment: Vec<(String, String)>,
}

impl AgentPlist {
    /// How long launchd waits for the agent to exit before it kills it: longer than the agent's own
    /// `5s` deadline to shut down.
    const EXIT_TIMEOUT_SECS: i64 = 10;

    fn for_this_client(working_dir: AgentWorkingDir, socket: PathBuf) -> io::Result<Self> {
        let working_dir = working_dir.resolve()?;
        Ok(Self {
            exe: std::env::current_exe()?,
            // launchd binds the socket from its own working directory, not the agent's.
            socket: working_dir.join(socket),
            working_dir,
            environment: std::env::vars().filter(|(name, _)| Self::is_inherited(name)).collect(),
        })
    }

    /// Whether the agent inherits the variable `name`: launchd starts it with a bare environment,
    /// and these configure atuin and say where it keeps its files.
    fn is_inherited(name: &str) -> bool {
        name.starts_with("ATUIN_") || name.starts_with("XDG_") || name == "TMPDIR"
    }

    fn to_xml(&self) -> io::Result<String> {
        let path = |path: &Path| {
            path.to_str().map(Value::from).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is not UTF-8, which a plist requires", path.display()),
                )
            })
        };

        let program_arguments = std::iter::once(path(&self.exe)?)
            .chain(AGENT_ARGS.map(Value::from))
            .chain([Value::from(Launchd::FLAG)])
            .collect();
        let environment = self
            .environment
            .iter()
            .map(|(name, value)| (name.clone(), Value::from(value.as_str())))
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
        Value::Dictionary(plist).to_writer_xml(&mut xml).map_err(io::Error::other)?;
        String::from_utf8(xml).map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStrExt;

    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::atuin_setting("ATUIN_DAEMON__SOCKET_PATH", true)]
    #[case::atuin_config_dir("ATUIN_CONFIG_DIR", true)]
    #[case::xdg("XDG_DATA_HOME", true)]
    #[case::tmpdir("TMPDIR", true)]
    #[case::path("PATH", false)]
    #[case::atuin_without_separator("ATUINX", false)]
    #[case::tmpdir_prefix("TMPDIR_OLD", false)]
    fn agent_inherits_only_atuin_configuration(#[case] name: &str, #[case] inherited: bool) {
        assert_eq!(AgentPlist::is_inherited(name), inherited);
    }

    /// A lossy path would point launchd at an executable that doesn't exist.
    #[rstest]
    fn non_utf8_executable_is_rejected_rather_than_mangled() {
        let plist = AgentPlist {
            exe: PathBuf::from(std::ffi::OsStr::from_bytes(b"/opt/\xffatuin")),
            working_dir: PathBuf::from("/"),
            socket: PathBuf::from("/tmp/atuin.sock"),
            environment: Vec::new(),
        };
        assert_eq!(plist.to_xml().unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
