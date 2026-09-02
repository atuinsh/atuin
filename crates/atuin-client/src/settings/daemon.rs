use std::path::PathBuf;
#[cfg(unix)]
use std::{borrow::Cow, path::Path};

#[cfg(unix)]
use atuin_common::os::unix::{SecureTempDirError, create_secure_temp_dir};
#[cfg(unix)]
use atuin_common::path::EnvDependentPathBuf;
use serde::{Deserialize, Serialize};

#[cfg(unix)]
const SOCKET_NAME: &str = "atuin.sock";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Daemon {
    /// Use the daemon to sync
    /// If enabled, history hooks are routed through the daemon.
    #[serde(alias = "enable")]
    pub enabled: bool,

    /// Automatically start and manage a local daemon when needed.
    pub autostart: bool,

    /// The daemon will handle sync on an interval. How often to sync, in seconds.
    pub sync_frequency: u64,

    /// The path to the unix socket used by the daemon.
    /// When unset, [`Daemon::socket_path`] picks the default location.
    pub socket_path: Option<PathBuf>,

    /// Path to the daemon pidfile used for process coordination.
    pub pidfile_path: String,

    /// Use a socket passed via systemd's socket activation protocol, instead of the path
    pub systemd_socket: bool,

    /// The port that should be used for TCP on non unix systems
    pub tcp_port: u64,
}

impl Default for Daemon {
    fn default() -> Self {
        Self {
            enabled: false,
            autostart: false,
            sync_frequency: 300,
            socket_path: None,
            pidfile_path: "".to_string(),
            systemd_socket: false,
            tcp_port: 8889,
        }
    }
}

#[cfg(unix)]
impl Daemon {
    /// The socket path we should use when creating a new socket.
    #[must_use]
    pub fn socket_path(&self) -> SocketPath<'_> {
        self.socket_path_ctx(DefaultSocketCtx)
    }

    /// The socket path we should use when trying to connect to an existing socket.
    ///
    /// This is the first path in [`Self::potential_socket_paths`] that exists, or if none exist,
    /// the first path.
    #[must_use]
    pub fn existing_socket_path(&self) -> Cow<'_, Path> {
        self.existing_socket_path_ctx(DefaultSocketCtx)
    }

    /// The list of paths at which an existing daemon socket might live.
    ///
    /// If the user has manually configured a socket path in config.toml, this method yields only
    /// that path. Otherwise, it yields:
    ///
    /// 1. If `systemd_socket` is true and `$XDG_RUNTIME_DIR` is non-empty,
    ///    `$XDG_RUNTIME_DIR/atuin.sock`.
    /// 2. `/tmp/atuin-$UID/atuin.sock`.
    /// 3. If `$XDG_RUNTIME_DIR` is non-empty, `$XDG_RUNTIME_DIR/atuin.sock` (legacy path), unless
    ///    step #1 already yielded that path.
    /// 4. If `$XDG_DATA_HOME` is non-empty, `$XDG_DATA_HOME/atuin/atuin.sock` (legacy path).
    /// 5. If `$XDG_DATA_HOME` is unset or empty, `~/.local/share/atuin/atuin.sock` (legacy path).
    pub fn potential_socket_paths(&self) -> impl Iterator<Item = Cow<'_, Path>> + use<'_> {
        self.potential_socket_paths_ctx(DefaultSocketCtx)
    }

    fn socket_path_ctx(&self, ctx: impl SocketCtx) -> SocketPath<'_> {
        if let Some(path) = &self.socket_path {
            return SocketPath::UserDefined(path);
        }

        // systemd units typically listen on `%t/atuin.sock`, which is `$XDG_RUNTIME_DIR/atuin.sock`
        // for user units, so we should default to that path when `systemd_socket` is true.
        if self.systemd_socket
            && let Some(path) = ctx.runtime_socket_path()
        {
            return SocketPath::Default(path);
        }

        SocketPath::Default(ctx.default_socket_path().primary)
    }

    fn existing_socket_path_ctx(&self, ctx: impl SocketCtx) -> Cow<'_, Path> {
        let mut candidates = self.potential_socket_paths_ctx(ctx);
        let primary =
            candidates.next().expect("there is always at least one potential socket path");

        if primary.exists() {
            return primary;
        }
        candidates.find(|path| path.exists()).unwrap_or(primary)
    }

    fn potential_socket_paths_ctx(
        &self,
        ctx: impl SocketCtx,
    ) -> impl Iterator<Item = Cow<'_, Path>> {
        let is_user_defined = self.socket_path.is_some();
        let defaults = (!is_user_defined)
            .then(|| self.default_socket_paths(ctx))
            .into_iter()
            .flatten()
            .map(Cow::Owned);
        self.socket_path.as_deref().map(Cow::Borrowed).into_iter().chain(defaults)
    }

    fn default_socket_paths(&self, ctx: impl SocketCtx) -> impl Iterator<Item = PathBuf> {
        let runtime_path = self.systemd_socket.then(|| ctx.runtime_socket_path()).flatten();
        // If `runtime_socket_path` is `Some`, `ctx.legacy_socket_path()` will return the same path
        // (both pointing to `$XDG_RUNTIME_DIR`), so don't include the legacy path in that case.
        let legacy_path = runtime_path.is_none().then(|| ctx.legacy_socket_path());
        let default_socket_path = ctx.default_socket_path();
        [runtime_path, Some(default_socket_path.primary), legacy_path, default_socket_path.envless]
            .into_iter()
            .flatten()
    }
}

/// Environment-dependent variables used in the calculation of the daemon socket path.
///
/// This is a trait for testing reasons; tests can provide custom implementations of this trait to
/// avoid writing to shared directories like `/tmp/atuin-$UID`, which could conflict with a real
/// instance of Atuin. Non-test code should always use [`DefaultSocketCtx`].
#[cfg(unix)]
trait SocketCtx: Copy + Sized {
    fn tmp_dir(&self) -> PathBuf {
        atuin_common::os::unix::tmp_dir()
    }

    /// Like [`Self::tmp_dir`] but not dependent on `$TMPDIR`.
    ///
    /// This is a workaround for systems in which the daemon is spawned in an environment that
    /// has `$TMPDIR` unset, but where `$TMPDIR` *is* set when the client is run.
    fn envless_tmp_dir(&self) -> &Path {
        Path::new("/tmp")
    }

    fn runtime_dir(&self) -> Option<PathBuf> {
        atuin_common::utils::env_abspath("XDG_RUNTIME_DIR")
    }

    fn data_dir(&self) -> PathBuf {
        atuin_common::utils::data_dir()
    }

    fn uid(&self) -> std::ffi::c_uint {
        atuin_common::os::unix::uid()
    }

    fn default_socket_path(&self) -> EnvDependentPathBuf {
        let subdir_name = format!("atuin-{}", self.uid());
        let make_socket_path = |tmp: PathBuf| tmp.join(&subdir_name).join(SOCKET_NAME);

        let tmp = self.tmp_dir();
        let envless_tmp = self.envless_tmp_dir();
        let envless_tmp = (tmp != envless_tmp).then(|| PathBuf::from(envless_tmp));

        EnvDependentPathBuf {
            primary: make_socket_path(tmp),
            envless: envless_tmp.map(make_socket_path),
        }
    }

    fn runtime_socket_path(&self) -> Option<PathBuf> {
        self.runtime_dir().map(|dir| dir.join(SOCKET_NAME))
    }

    fn legacy_socket_path(&self) -> PathBuf {
        self.runtime_socket_path().unwrap_or_else(|| self.data_dir().join(SOCKET_NAME))
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Default)]
struct DefaultSocketCtx;
#[cfg(unix)]
impl SocketCtx for DefaultSocketCtx {}

#[cfg(unix)]
pub enum SocketPath<'a> {
    /// A socket path that the user has manually configured in config.toml.
    UserDefined(&'a Path),
    /// The default socket path, used when the path is not manually set in config.toml.
    Default(PathBuf),
}

#[cfg(unix)]
impl<'a> SocketPath<'a> {
    /// Create the default socket directory if needed.
    ///
    /// This only applies to the default socket path: the directory holding a user-specified path
    /// from config.toml is the user's to create, and `$XDG_RUNTIME_DIR` is created for us.
    pub fn create_default_dir_if_needed(&self) -> Result<(), SecureTempDirError> {
        let Self::Default(path) = self else {
            return Ok(());
        };
        let dir = path.parent().expect("default socket path always has a parent");
        create_secure_temp_dir(dir)?;
        Ok(())
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        match self {
            Self::UserDefined(path) => path,
            Self::Default(path) => path,
        }
    }

    #[must_use]
    pub fn into_cow(self) -> Cow<'a, Path> {
        match self {
            Self::UserDefined(path) => path.into(),
            Self::Default(path) => path.into(),
        }
    }

    #[must_use]
    pub fn into_owned(self) -> PathBuf {
        self.into_cow().into_owned()
    }
}

#[cfg(unix)]
impl AsRef<Path> for SocketPath<'_> {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

#[cfg(unix)]
impl<'a> From<SocketPath<'a>> for Cow<'a, Path> {
    fn from(path: SocketPath<'a>) -> Self {
        path.into_cow()
    }
}

#[cfg(all(unix, test))]
mod unix_tests {
    use std::fs::Permissions;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    use rstest::*;
    use tempfile::TempDir;

    use super::*;

    #[fixture]
    fn tmp() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    /// Behaves like the real default socket path, but is actually in a scratch directory rather
    /// than `/tmp/atuin-$UID`, which could conflict with a real Atuin instance.
    struct DefaultSocket {
        dir: PathBuf,
        _tmp: TempDir,
    }

    impl DefaultSocket {
        fn path(&self) -> SocketPath<'static> {
            SocketPath::Default(self.dir.join("atuin.sock"))
        }
    }

    #[fixture]
    fn default_socket(tmp: TempDir) -> DefaultSocket {
        DefaultSocket {
            dir: tmp.path().join("atuin-1234"),
            _tmp: tmp,
        }
    }

    fn daemon(socket_path: Option<PathBuf>, systemd_socket: bool) -> Daemon {
        Daemon {
            socket_path,
            systemd_socket,
            ..Daemon::default()
        }
    }

    const TMPDIR: &str = "/var/folders/xy";
    /// Where the socket lives when `$TMPDIR` is set to [`TMPDIR`].
    const TMPDIR_DEFAULT: &str = "/var/folders/xy/atuin-1234/atuin.sock";
    /// Where the socket lives when `$TMPDIR` is unset. Also the `$TMPDIR`-independent fallback.
    const DEFAULT: &str = "/tmp/atuin-1234/atuin.sock";
    const RUNTIME: &str = "/run/user/1234/atuin.sock";
    const LEGACY: &str = "/home/user/.local/share/atuin/atuin.sock";
    const CUSTOM: &str = "/custom/atuin.sock";

    /// A [`SocketCtx`] with every input fixed, so that each case can say which of them it expects
    /// the socket path to be taken from, and so that no test touches a real Atuin instance's
    /// directories.
    #[derive(Clone, Copy)]
    struct TestCtx<'a> {
        tmp_dir: &'a Path,
        envless_tmp_dir: &'a Path,
        runtime_dir: Option<&'a Path>,
    }

    impl Default for TestCtx<'_> {
        fn default() -> Self {
            Self {
                tmp_dir: Path::new("/tmp"),
                envless_tmp_dir: Path::new("/tmp"),
                runtime_dir: Some(Path::new("/run/user/1234")),
            }
        }
    }

    impl SocketCtx for TestCtx<'_> {
        fn tmp_dir(&self) -> PathBuf {
            self.tmp_dir.into()
        }

        fn envless_tmp_dir(&self) -> &Path {
            self.envless_tmp_dir
        }

        fn runtime_dir(&self) -> Option<PathBuf> {
            self.runtime_dir.map(Into::into)
        }

        fn data_dir(&self) -> PathBuf {
            "/home/user/.local/share/atuin".into()
        }

        fn uid(&self) -> std::ffi::c_uint {
            1234
        }
    }

    /// The socket has to be at the same path in every environment on a machine, so with nothing
    /// configured it depends on nothing but the temporary directory and the uid.
    #[rstest]
    #[case::default_tmp_dir("/tmp", DEFAULT)]
    #[case::tmpdir_set(TMPDIR, TMPDIR_DEFAULT)]
    fn the_default_socket_lives_in_a_per_uid_tmp_dir(
        #[case] tmp_dir: &str,
        #[case] expected: &str,
    ) {
        let ctx = TestCtx {
            tmp_dir: Path::new(tmp_dir),
            ..TestCtx::default()
        };

        assert_eq!(ctx.default_socket_path().primary, Path::new(expected));
    }

    #[rstest]
    // With nothing configured we use the per-uid default, and only fall back to where an older
    // version of Atuin would have left a socket. `$TMPDIR` is unset here, so the default already
    // is the `/tmp` path and it is not yielded a second time.
    #[case::unconfigured("/tmp", None, false, true, vec![DEFAULT, RUNTIME])]
    #[case::unconfigured_without_runtime_dir("/tmp", None, false, false, vec![DEFAULT, LEGACY])]
    // A socket-activated unit listens on `%t/atuin.sock`, which is also the legacy path.
    #[case::systemd("/tmp", None, true, true, vec![RUNTIME, DEFAULT])]
    #[case::systemd_without_runtime_dir("/tmp", None, true, false, vec![DEFAULT, LEGACY])]
    // A configured path is the only one we ever consider.
    #[case::configured("/tmp", Some(CUSTOM), false, true, vec![CUSTOM])]
    #[case::configured_with_systemd("/tmp", Some(CUSTOM), true, true, vec![CUSTOM])]
    // With `$TMPDIR` set, the `$TMPDIR` socket still wins, but `/tmp` is tried last, because the
    // daemon may have been started in an environment where `$TMPDIR` was unset.
    #[case::tmpdir_set(TMPDIR, None, false, true, vec![TMPDIR_DEFAULT, RUNTIME, DEFAULT])]
    #[case::tmpdir_set_without_runtime_dir(
        TMPDIR, None, false, false, vec![TMPDIR_DEFAULT, LEGACY, DEFAULT]
    )]
    #[case::tmpdir_set_with_systemd(
        TMPDIR, None, true, true, vec![RUNTIME, TMPDIR_DEFAULT, DEFAULT]
    )]
    // A configured path stays the only one we consider, `$TMPDIR` or not.
    #[case::tmpdir_set_and_configured(TMPDIR, Some(CUSTOM), false, true, vec![CUSTOM])]
    fn socket_paths_are_tried_in_priority_order(
        #[case] tmp_dir: &str,
        #[case] configured: Option<&str>,
        #[case] systemd_socket: bool,
        #[case] runtime_dir: bool,
        #[case] expected: Vec<&str>,
    ) {
        let daemon = daemon(configured.map(PathBuf::from), systemd_socket);
        let ctx = TestCtx {
            tmp_dir: Path::new(tmp_dir),
            runtime_dir: runtime_dir.then_some(Path::new("/run/user/1234")),
            ..TestCtx::default()
        };

        assert_eq!(
            daemon.potential_socket_paths_ctx(ctx).map(Cow::into_owned).collect::<Vec<_>>(),
            expected.iter().map(PathBuf::from).collect::<Vec<_>>(),
        );
        assert_eq!(daemon.socket_path_ctx(ctx).as_path(), Path::new(expected[0]));
    }

    /// The path we connect to is the first one that is actually there, so that a daemon listening
    /// on a fallback path is still found.
    #[rstest]
    #[case::none_exist(&[], TMPDIR_DEFAULT)]
    #[case::only_the_runtime_fallback_exists(&[RUNTIME], RUNTIME)]
    // The case this fallback exists for: a daemon started without `$TMPDIR` is listening on
    // `/tmp`, while the client that looks for it has `$TMPDIR` set.
    #[case::only_the_envless_fallback_exists(&[DEFAULT], DEFAULT)]
    #[case::runtime_fallback_beats_envless(&[RUNTIME, DEFAULT], RUNTIME)]
    #[case::all_exist(&[TMPDIR_DEFAULT, RUNTIME, DEFAULT], TMPDIR_DEFAULT)]
    fn the_existing_socket_is_the_first_one_present(
        tmp: TempDir,
        #[case] present: &[&str],
        #[case] expected: &str,
    ) {
        // Prefix a path with `tmp`.
        let scoped = |path: &str| tmp.path().join(path.trim_start_matches('/'));
        let (tmp_dir, envless_tmp_dir, runtime_dir) =
            (scoped(TMPDIR), scoped("/tmp"), scoped("/run/user/1234"));
        let ctx = TestCtx {
            tmp_dir: &tmp_dir,
            envless_tmp_dir: &envless_tmp_dir,
            runtime_dir: Some(&runtime_dir),
        };

        for path in present {
            let path = scoped(path);
            fs_err::create_dir_all(path.parent().unwrap()).unwrap();
            fs_err::File::create(path).unwrap();
        }

        assert_eq!(daemon(None, false).existing_socket_path_ctx(ctx), scoped(expected));
    }

    #[rstest]
    fn no_directory_is_created_for_a_user_defined_socket(tmp: TempDir) {
        let dir = tmp.path().join("custom");
        let daemon = daemon(Some(dir.join("atuin.sock")), false);

        daemon.socket_path().create_default_dir_if_needed().unwrap();

        assert!(!dir.exists(), "created a directory for a user-defined path");
        assert_eq!(daemon.existing_socket_path(), dir.join("atuin.sock"));
    }

    #[rstest]
    fn default_socket_dir_is_created_privately_then_reused(default_socket: DefaultSocket) {
        default_socket.path().create_default_dir_if_needed().unwrap();
        let mode = fs_err::metadata(&default_socket.dir).unwrap().mode();
        assert_eq!(mode & 0o777, 0o700);

        default_socket.path().create_default_dir_if_needed().unwrap();
    }

    #[rstest]
    #[case::group_readable(0o740)]
    #[case::other_readable(0o704)]
    #[case::world_writable(0o777)]
    fn a_socket_dir_reachable_by_others_is_rejected(
        default_socket: DefaultSocket,
        #[case] mode: u32,
    ) {
        default_socket.path().create_default_dir_if_needed().unwrap();
        fs_err::set_permissions(&default_socket.dir, Permissions::from_mode(mode)).unwrap();

        let Err(SecureTempDirError::WrongPermissions { permissions, .. }) =
            default_socket.path().create_default_dir_if_needed()
        else {
            panic!("a socket directory with mode {mode:03o} must be rejected");
        };
        assert_eq!(permissions, mode);
    }

    #[rstest]
    fn a_symlinked_socket_dir_is_rejected(default_socket: DefaultSocket) {
        symlink("/tmp", &default_socket.dir).unwrap();

        assert!(matches!(
            default_socket.path().create_default_dir_if_needed(),
            Err(SecureTempDirError::NotADirectory(_))
        ));
    }
}
