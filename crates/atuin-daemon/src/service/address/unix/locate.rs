use std::path::{Path, PathBuf};

use atuin_client::settings::Daemon as DaemonSettings;

use super::DaemonAddress;
use crate::service::pidfile::Pidfile;

/// Errors from [`DaemonAddress::locate`].
#[derive(Debug, thiserror::Error)]
pub enum LocateError {
    /// The socket is in use by another program.
    ///
    /// This error can be returned when `ATUIN_HOME` is set. Atuin started storing the socket path
    /// in the pidfile before `ATUIN_HOME` was introduced, so if there's no socket path in the
    /// pidfile but the socket already exists, it's an indication that the socket is in use by
    /// another program (potentially another Atuin daemon running with a different value of
    /// `ATUIN_HOME`).
    #[error("{}", Self::display_in_use(.path, *.user_defined))]
    InUse {
        /// The path to the socket.
        path: PathBuf,
        /// Whether the path came from `daemon.socket_path` in config.toml.
        user_defined: bool,
    },
}

impl LocateError {
    /// Helper for displaying [`Self::InUse`].
    fn display_in_use(path: &Path, user_defined: bool) -> impl std::fmt::Display + use<'_> {
        std::fmt::from_fn(move |f| {
            writeln!(f, "daemon socket '{}' is in use by another program", path.display())?;
            let config_path = atuin_common::dirs::config_path("config.toml");
            if user_defined {
                write!(
                    f,
                    "please ensure `daemon.socket_path` has a unique value in {}",
                    config_path.display(),
                )
            } else {
                write!(
                    f,
                    "please set an explicit `daemon.socket_path` in {}",
                    config_path.display()
                )
            }
        })
    }
}

impl DaemonAddress {
    /// Where the running daemon `daemon` describes listens.
    ///
    /// If the daemon is running and has recorded its socket path in the pidfile, this function returns
    /// that. Otherwise:
    ///
    /// * If `ATUIN_HOME` is unset (the default), [`daemon.existing_socket_path()`][0] is returned,
    ///   which will check for existing sockets in legacy locations.
    ///
    /// * If `ATUIN_HOME` is set, [`daemon.preferred_socket_path()`][1] is returned, which does
    ///   not check for legacy sockets, as Atuin stopped using them before `ATUIN_HOME` was introduced.
    ///   In addition, an error is returned if that socket already exists: Atuin started storing the
    ///   socket path in the pidfile before `ATUIN_HOME`, so if an existing socket isn't in our pidfile,
    ///   it almost certainly isn't ours. This behavior is intended to reduce the chance of the client
    ///   connecting to the wrong daemon.
    ///
    /// [0]: atuin_client::settings::Daemon::existing_socket_path
    /// [1]: atuin_client::settings::Daemon::preferred_socket_path
    ///
    /// TODO(markovejnovic): Eventually get to the point where we don't need to take the settings.
    pub fn locate(daemon: &DaemonSettings, pidfile: &Pidfile) -> Result<Self, LocateError> {
        let existing_socket_path = || daemon.existing_socket_path().into_owned();

        let socket_path = pidfile.info().and_then(|info| info.socket_path);

        if !atuin_common::dirs::atuin_home_is_set() {
            return Ok(Self(socket_path.unwrap_or_else(existing_socket_path)));
        }

        // If `ATUIN_HOME` is set, require the pidfile to be live to avoid connecting to the wrong
        // daemon. This should potentially be done even when `ATUIN_HOME` isn't set, but that would be a
        // larger and possibly riskier change. For now, scope the check to when `ATUIN_HOME` is set,
        // which is when we would most expect there to be multiple daemons running on the user's system.
        if let Some(path) = socket_path.filter(|_| pidfile.is_live()) {
            return Ok(Self(path));
        }

        // If there's no socket path in the pidfile (or the pidfile was missing/stale) and `ATUIN_HOME`
        // is set, don't look for or connect to existing sockets. Atuin switched to storing the socket
        // path in the pidfile before `ATUIN_HOME` was introduced, so an existing socket is almost
        // certainly that of another program, potentially another daemon running with a different value
        // of `ATUIN_HOME`, which is conceptually a separate profile that we should not connect to.
        let path = daemon.preferred_socket_path().into_owned();

        // Avoid connecting to an existing, in-use socket. We match on `Ok(true)` rather than
        // `Ok(true) | Err(_)` because an error doesn't necessarily mean the socket is in use (e.g.,
        // `EACCES`), and many such errors will be encountered again later in the pipeline and handled
        // better there.
        if path.exists() && matches!(atuin_common::os::unix::socket_in_use(&path), Ok(true)) {
            return Err(LocateError::InUse {
                path,
                user_defined: daemon.socket_path.is_some(),
            });
        }
        Ok(Self(path))
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::settings::Settings;
    use atuin_common::env::MockEnv;
    use atuin_common::path::{AbsPathBuf, PathBufExt as _};
    use rstest::rstest;

    use super::*;
    use crate::service::RunMode;
    use crate::service::pidfile::{AcquireGuardError, PidfileGuard, PidfileInfo};

    /// Lock the pidfile `daemon` configures, as a daemon started with it does.
    fn acquire(daemon: &DaemonSettings) -> Result<PidfileGuard, AcquireGuardError> {
        Pidfile::new(&daemon.pidfile_path)
            .acquire(&PidfileInfo::current(daemon, RunMode::DaemonizeForked))
    }

    /// Where the pidfile `daemon` configures says the daemon listens.
    fn socket_path(daemon: &DaemonSettings) -> Result<PathBuf, LocateError> {
        DaemonAddress::locate(daemon, &Pidfile::new(&daemon.pidfile_path)).map(|address| address.0)
    }

    /// Settings for a client whose daemon socket is configured at `socket_path`.
    fn settings(pidfile: &Path, socket_path: &Path, systemd_socket: bool) -> Settings {
        let mut settings = Settings::default();
        settings.daemon.pidfile_path = AbsPathBuf::new(pidfile.to_owned()).unwrap();
        settings.daemon.socket_path = Some(AbsPathBuf::new(socket_path.to_owned()).unwrap());
        settings.daemon.systemd_socket = systemd_socket;
        settings
    }

    /// A daemon configured for `old.sock` has exited, leaving its pidfile behind, and the client is
    /// now configured for `new.sock`.
    #[rstest]
    #[case::pidfile_path_is_used(false, false, "old.sock")]
    #[case::systemd_daemon_records_no_path(true, false, "new.sock")]
    #[case::both_systemd(true, true, "new.sock")]
    fn test_socket_path_with_stale_pidfile(
        #[case] daemon_systemd_socket: bool,
        #[case] client_systemd_socket: bool,
        #[case] expected: &str,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), false);

        let pidfile = dir.path().join("atuin-daemon.pid");

        let old = settings(&pidfile, &dir.path().join("old.sock"), daemon_systemd_socket);
        drop(acquire(&old.daemon).unwrap());

        let new = settings(&pidfile, &dir.path().join("new.sock"), client_systemd_socket);
        assert_eq!(socket_path(&new.daemon).unwrap(), dir.path().join(expected));
    }

    #[rstest]
    fn test_socket_path_without_pidfile(#[values(false, true)] systemd_socket: bool) {
        let dir = tempfile::tempdir().unwrap();
        let settings = settings(
            &dir.path().join("atuin-daemon.pid"),
            &dir.path().join("a.sock"),
            systemd_socket,
        );
        assert_eq!(socket_path(&settings.daemon).unwrap(), dir.path().join("a.sock"));
    }

    /// A mock environment with `HOME` and `TMPDIR` in `dir`, and `ATUIN_HOME` too if
    /// `atuin_home_set`, so that neither the settings nor the default socket path involve the
    /// real home or `/tmp`.
    fn env(dir: &Path, atuin_home_set: bool) -> MockEnv {
        let env = MockEnv::install();
        env.set("HOME", dir.join("home"));
        env.set("TMPDIR", dir.join("tmp"));
        if atuin_home_set {
            env.set("ATUIN_HOME", dir.join("profile"));
        }
        env
    }

    /// Listen on a Unix socket at `path`, as a running daemon does, until the listener is
    /// dropped. Dropping it leaves the socket file behind with nothing listening, as a daemon that
    /// was killed does.
    fn listen(path: &Path) -> std::os::unix::net::UnixListener {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::net::UnixListener::bind(path).unwrap()
    }

    /// The default socket path for `dir` as [`env`] sets it up, or the configured one.
    fn preferred_path(dir: &Path, configured: bool) -> PathBuf {
        if configured {
            dir.join("configured.sock")
        } else {
            dir.join("tmp")
                .joined(format!("atuin-{}", atuin_common::os::unix::uid()))
                .joined("atuin.sock")
        }
    }

    /// Settings with the socket at [`preferred_path`], and a pidfile in `dir`.
    fn settings_in(dir: &Path, configured: bool) -> Settings {
        let mut settings =
            settings(&dir.join("atuin-daemon.pid"), &dir.join("configured.sock"), false);
        if !configured {
            settings.daemon.socket_path = None;
        }
        assert_eq!(settings.daemon.preferred_socket_path(), preferred_path(dir, configured));
        settings
    }

    /// With `ATUIN_HOME` set, a live socket that the pidfile doesn't name belongs to something
    /// else, such as a daemon for another `ATUIN_HOME`, so the client refuses it rather than
    /// connect. Without `ATUIN_HOME`, an existing socket is connected to as before.
    #[rstest]
    fn an_unrecorded_live_socket_is_refused_only_with_atuin_home(
        #[values(false, true)] atuin_home_set: bool,
        #[values(false, true)] configured: bool,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), atuin_home_set);
        let settings = settings_in(dir.path(), configured);
        let path = preferred_path(dir.path(), configured);

        // Nothing's there yet, so it's free for this profile's daemon. (Without `ATUIN_HOME`, the
        // client would also fall back to sockets elsewhere, like the real `/tmp/atuin-$UID`.)
        if atuin_home_set {
            assert_eq!(socket_path(&settings.daemon).unwrap(), path);
        }

        let _listener = listen(&path);
        let result = socket_path(&settings.daemon);
        if !atuin_home_set {
            assert_eq!(result.unwrap(), path);
            return;
        }
        let err = result.unwrap_err();
        let message = err.to_string();
        let LocateError::InUse {
            path: in_use,
            user_defined,
        } = err;
        assert_eq!(in_use, path);
        assert_eq!(user_defined, configured);
        assert!(message.contains(&path.display().to_string()), "{message}");
        let hint = if configured {
            "has a unique value"
        } else {
            "set an explicit"
        };
        assert!(message.contains(hint), "{message}");
    }

    /// With `ATUIN_HOME` set, a socket left behind with nothing listening, as by a daemon that was
    /// killed, is free: the client uses it, and autostart replaces it.
    #[rstest]
    fn with_atuin_home_a_stale_socket_is_free(#[values(false, true)] configured: bool) {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), true);
        let settings = settings_in(dir.path(), configured);
        let path = preferred_path(dir.path(), configured);

        drop(listen(&path));
        assert!(path.exists(), "the stale socket should be left behind");
        assert_eq!(socket_path(&settings.daemon).unwrap(), path);
    }

    /// With `ATUIN_HOME` set, the socket the pidfile names is used while its daemon is running,
    /// whatever else exists.
    #[rstest]
    fn with_atuin_home_the_recorded_socket_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), true);
        let pidfile = dir.path().join("atuin-daemon.pid");

        let daemon = settings(&pidfile, &dir.path().join("daemon.sock"), false);
        let _guard = acquire(&daemon.daemon).unwrap();

        let client = settings(&pidfile, &dir.path().join("other.sock"), false);
        let _listener = listen(&dir.path().join("other.sock"));
        assert_eq!(socket_path(&client.daemon).unwrap(), dir.path().join("daemon.sock"));
    }

    /// With `ATUIN_HOME` set, a socket recorded by a daemon that has since exited isn't trusted:
    /// another profile's daemon may be using that path now. The client goes by its own settings,
    /// and refuses the path if something is live there.
    #[rstest]
    fn with_atuin_home_a_stale_pidfile_is_not_trusted(#[values(false, true)] live: bool) {
        let dir = tempfile::tempdir().unwrap();
        let _env = env(dir.path(), true);
        let pidfile = dir.path().join("atuin-daemon.pid");

        // A daemon recorded `daemon.sock`, then exited, leaving its pidfile behind. Its path is
        // now live, as if another profile's daemon had taken it.
        let daemon = settings(&pidfile, &dir.path().join("daemon.sock"), false);
        drop(acquire(&daemon.daemon).unwrap());
        let _taken = listen(&dir.path().join("daemon.sock"));

        let client = settings_in(dir.path(), true);
        let path = preferred_path(dir.path(), true);
        let _listener = live.then(|| listen(&path));
        match socket_path(&client.daemon) {
            Ok(found) => {
                assert!(!live, "a live socket at {} was accepted", path.display());
                assert_eq!(found, path);
            }
            Err(LocateError::InUse { path: in_use, .. }) => {
                assert!(live, "a free socket path was refused");
                assert_eq!(in_use, path);
            }
        }
    }
}
