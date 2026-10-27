//! Provides the location of Atuin's files.

use std::path::{Path, PathBuf};

use crate::env::{var_abspath, var_nonempty};
use crate::path::{PathBufExt as _, PathExt as _};

/// Error returned when the user's home directory can't be determined.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("could not determine home directory")]
pub struct HomeError;

/// Get the user's home directory.
///
/// Example: `/home/ellie`
///
/// Returns an error if the user's home directory can't be determined.
pub fn try_home_dir() -> Result<PathBuf, HomeError> {
    // `dirs::home_dir` doesn't go through this crate's env wrappers, so it can't be mocked. As
    // a workaround, a mock environment's `HOME` is the only home there is.
    #[cfg(any(test, feature = "test-utils"))]
    if crate::env::mock::is_installed() {
        return var_nonempty("HOME").map(PathBuf::from).ok_or(HomeError);
    }
    dirs::home_dir().ok_or(HomeError)
}

/// Variant of [`try_home_dir`] that panics instead of returning a [`HomeError`].
#[must_use]
pub fn home_dir() -> PathBuf {
    try_home_dir().unwrap_or_else(|err| panic!("{err}"))
}

/// Represents the path to Atuin's home directory.
///
/// See [`atuin_home`].
#[derive(Clone, Debug, PartialEq, Eq, derive_more::Deref)]
pub enum AtuinHome {
    /// The path came from `ATUIN_HOME`, which was nonempty and valid.
    Set(PathBuf),
    /// `ATUIN_HOME` was unset, so this is the default `~/.atuin`.
    Default(PathBuf),
}

impl AsRef<Path> for AtuinHome {
    fn as_ref(&self) -> &Path {
        self
    }
}

impl From<AtuinHome> for PathBuf {
    fn from(home: AtuinHome) -> Self {
        match home {
            AtuinHome::Set(path) | AtuinHome::Default(path) => path,
        }
    }
}

/// Get Atuin's home directory.
///
/// All Atuin data should be stored in this directory. By default, this is `~/.atuin`, but can
/// be overridden with the `ATUIN_HOME` environment variable, which must be an absolute path.
///
/// Returns an error if `ATUIN_HOME` is unset/invalid and the user's home directory can't be
/// determined for the default case.
pub fn try_atuin_home() -> Result<AtuinHome, HomeError> {
    if let Some(value) = var_abspath("ATUIN_HOME") {
        return Ok(AtuinHome::Set(value.into()));
    }
    Ok(AtuinHome::Default(try_home_dir()?.joined(".atuin")))
}

/// Variant of [`try_atuin_home`] that panics instead of returning a [`HomeError`].
#[must_use]
pub fn atuin_home() -> AtuinHome {
    try_atuin_home().unwrap_or_else(|err| panic!("{err}"))
}

/// Check whether `ATUIN_HOME` is set and valid (is an absolute path).
///
/// This function is slightly more efficient than checking the return value of [`try_atuin_home`].
#[must_use]
pub fn atuin_home_is_set() -> bool {
    var_abspath("ATUIN_HOME").is_some()
}

/// Check whether `ATUIN_CONFIG_DIR` is set, moving `config.toml` and `server.toml` out of Atuin's
/// home.
#[must_use]
pub fn atuin_config_dir_is_set() -> bool {
    var_nonempty("ATUIN_CONFIG_DIR").is_some()
}

/// Top-level config items affected by the `ATUIN_CONFIG_DIR` environment variable.
const ATUIN_CONFIG_DIR_ITEMS: [&str; 2] = ["config.toml", "server.toml"];

/// Top-level config items that Atuin AI stores in both project-level `.atuin` directories as well
/// as the global `~/.atuin`.
///
/// Before the switch to [`atuin_home`], a file like `~/.atuin/permissions.ai.toml` was scoped to
/// `$HOME`, like any other project directory. Now, `~/.atuin/permissions.ai.toml` is the new
/// location for the *global* permissions file. [`config_path`] implements logic to avoid
/// reinterpreting old `$HOME`-scoped files as global ones; this array contains all the affected
/// items.
const AI_ITEMS: [&str; 3] = ["permissions.ai.toml", "skills", "TERMINAL.md"];

/// Get the path of a config item.
///
/// `child` is relative to the config directory. The final path is determined as follows:
///
/// 1. If `child` is `config.toml` or `server.toml` and `ATUIN_CONFIG_DIR` is set,
///    `$ATUIN_CONFIG_DIR/child` is returned.
/// 2. If `ATUIN_HOME` is set and valid (an absolute path), `$ATUIN_HOME/child` is returned.
/// 3. If the first component of `child` is `permissions.ai.toml`, `skills`, or `TERMINAL.md`, and
///    `~/.atuin/config.toml` doesn't exist, `legacy_config_dir()/child`[^1] is returned.
/// 4. If the first component of `child` exists in `~/.atuin`, `~/.atuin/child` is returned.
/// 5. If the first component of `child` is `themes` and `ATUIN_THEME_DIR` is set,
///    `$ATUIN_THEME_DIR/child_tail` is returned, where `child_tail` is all of `child` except the
///    first component. This behavior is deprecated and exists only for backward compatibility.
/// 6. If the first component of `child` is `themes` and `ATUIN_CONFIG_DIR` is set,
///    `$ATUIN_CONFIG_DIR/themes` is returned. This behavior is deprecated and exists only for
///    backward compatibility.
/// 7. If the first component of `child` exists in `legacy_config_dir()`[^1],
///    `legacy_config_dir()/child` is returned.
/// 8. If `child` is `config.toml` or `server.toml`, and is present in `~/.config/atuin`,
///    `legacy_config_dir()/child`[^1] is returned (note that `XDG_CONFIG_HOME` is necessarily set
///    in this case, so `legacy_config_dir()` is not `~/.config/atuin`). This avoids creating the
///    file in `~/.atuin`, which would shadow `~/.config/atuin` in future invocations of Atuin with
///    `XDG_CONFIG_HOME` unset.
/// 9. Otherwise, `~/.atuin/child` is returned.
///
/// When these steps refer to checking the existence of a path that depends on the user's home
/// directory, like `~/.atuin/config.toml`, an inability to determine the user's home directory is
/// treated the same as the path not existing.
///
/// # Panics
///
/// Panics if `child` is empty or absolute, or if it starts with `.` or `..`. Also panics if the
/// user's home directory can't be determined and the path can't be calculated without it, which can
/// happen if step 9 is reached, or if the condition in step 3 is true.
///
/// [^1]: `legacy_config_dir()` is `$XDG_CONFIG_HOME/atuin`, or `~/.config/atuin` if unset.
#[must_use]
pub fn config_path(child: impl AsRef<Path>) -> PathBuf {
    let child = child.as_ref();
    let Some(std::path::Component::Normal(top)) = child.components().next() else {
        panic!("config path must be relative and start with a top-level item: {child:?}");
    };

    if ATUIN_CONFIG_DIR_ITEMS.iter().any(|name| *name == top)
        && let Some(value) = var_nonempty("ATUIN_CONFIG_DIR")
    {
        return PathBuf::from(value).joined(child);
    }

    let home = match try_atuin_home() {
        Ok(AtuinHome::Set(path)) => return path.joined(child),
        Ok(AtuinHome::Default(path)) => Ok(path),
        Err(e) => Err(e),
    };

    // The existence of `~/.atuin/config.toml` distinguishes new installations from old ones. With
    // `ATUIN_HOME` unset, if `~/.atuin/config.toml` is missing (indicating an old installation),
    // continue to use the legacy config dir for AI-related items, to avoid interpreting an existing
    // `~/.atuin/permissions.ai.toml`, which was previously scoped to `$HOME`, as the global file.
    if AI_ITEMS.iter().any(|name| *name == top)
        && !matches!(&home, Ok(home) if home.join("config.toml").exists())
    {
        return legacy_config_dir().unwrap_or_else(|err| panic!("{err}")).joined(child);
    }

    let home = match home {
        Ok(mut path) => {
            path.push(top);
            if path.exists() {
                path.pop(); // pop `top` from the path
                path.push(child);
                return path;
            }
            path.pop(); // pop `top` from the path
            Ok(path)
        }
        Err(e) => Err(e),
    };

    let is_themes = top == "themes";
    if is_themes && let Some(value) = var_nonempty("ATUIN_THEME_DIR") {
        // For backward compatibility, keep respecting the deprecated `ATUIN_THEME_DIR` variable,
        // but only if `ATUIN_HOME` is unset and `~/.atuin/themes` doesn't exist.
        let mut path = PathBuf::from(value);
        path.extend(child.components().skip(1));
        return path;
    }

    // Potential directories that might contain the config item. The first one that contains the
    // item, if any, is used.
    let potential_dirs = [
        // Historically, `ATUIN_CONFIG_DIR` affected the location of `config.toml`, `server.toml`,
        // and `themes`, but not any other config items such as `skills` or `permissions.ai.toml`.
        // This behavior has been simplified: `ATUIN_CONFIG_DIR` only affects `config.toml` and
        // `server.toml`; using it to control `themes` is deprecated.
        //
        // For backward compatibility, read themes from `$ATUIN_CONFIG_DIR/themes`, but only if
        // `ATUIN_HOME` is unset and `~/.atuin/themes` doesn't exist.
        is_themes.then(|| var_nonempty("ATUIN_CONFIG_DIR")).flatten().map(Into::into),
        // Atuin used to store config items in `$XDG_CONFIG_HOME/atuin`. For backward compatibility,
        // if `ATUIN_HOME` is unset, continue to read items from there when they don't exist in
        // `~/.atuin`.
        legacy_config_dir().ok(),
    ];

    if let Some(mut existing) =
        potential_dirs.into_iter().flatten().map(|dir| dir.joined(top)).find(|dir| dir.exists())
    {
        existing.pop(); // pop `top` from the path
        existing.push(child);
        return existing;
    }

    // If an autogenerated item (those in `ATUIN_CONFIG_DIR_ITEMS`) is present in `~/.config/atuin`,
    // use the legacy config dir, even if that's a different directory (due to `XDG_CONFIG_HOME`).
    // Creating the file in `~/.atuin` would shadow `~/.config/atuin` in future invocations of Atuin
    // with `XDG_CONFIG_HOME` unset.
    if ATUIN_CONFIG_DIR_ITEMS.iter().any(|name| *name == top)
        && legacy_config_dir_with(XdgDir::Default).is_ok_and(|dir| dir.joined(top).exists())
    {
        return legacy_config_dir().unwrap_or_else(|err| panic!("{err}")).joined(child);
    }

    // At this point, we cannot continue without being able to determine the user's home directory.
    home.unwrap_or_else(|err| panic!("{err}")).joined(child)
}

/// Represents an XDG Base Directory.
enum XdgDir {
    /// The directory has been explicitly set via an environment variable.
    Explicit(PathBuf),
    /// The default value of the XDG Base Directory should be used.
    Default,
}

impl XdgDir {
    /// Create an [`XdgDir`] from the given environment variable.
    pub fn from_var(name: &str) -> Self {
        match var_abspath(name) {
            Some(path) => Self::Explicit(path.into()),
            None => Self::Default,
        }
    }

    /// Return an iterator that yields both the custom XDG dir in the environment variable `name`,
    /// and the default dir when `name` is unset.
    ///
    /// If `name` is currently unset in this process, the iterator yields only the default dir.
    pub fn with_default(name: &str) -> impl Iterator<Item = Self> {
        let dir = Self::from_var(name);
        let default = matches!(dir, Self::Explicit(_)).then_some(Self::Default);
        std::iter::once(dir).chain(default)
    }
}

/// Existing legacy `config.toml` files: in [`legacy_config_dir`], or in `~/.config/atuin` even when
/// `XDG_CONFIG_HOME` points elsewhere, as it does for a test or a sandbox.
fn existing_legacy_config_files() -> impl Iterator<Item = PathBuf> {
    XdgDir::with_default("XDG_CONFIG_HOME")
        .filter_map(|xdg| legacy_config_dir_with(xdg).ok())
        .map(|dir| dir.joined("config.toml"))
        .filter(|path| path.exists())
}

/// A legacy `config.toml` that an existing `~/.atuin/config.toml` hides: Atuin reads only the
/// latter. Not when `ATUIN_CONFIG_DIR` or `ATUIN_HOME` picks the config on purpose.
#[must_use]
pub fn shadowed_legacy_config() -> Option<PathBuf> {
    if var_nonempty("ATUIN_CONFIG_DIR").is_some() {
        return None;
    }
    let Ok(AtuinHome::Default(home)) = try_atuin_home() else {
        return None;
    };
    let default = home.joined("config.toml");
    if !default.exists() {
        return None;
    }
    existing_legacy_config_files().find(|legacy| !legacy.is_same_path(&default))
}

/// Get the legacy config directory, used before the switch to [`atuin_home`].
///
/// This is `$XDG_CONFIG_HOME/atuin`, or `~/.config/atuin` if unset.
fn legacy_config_dir() -> Result<PathBuf, HomeError> {
    legacy_config_dir_with(XdgDir::from_var("XDG_CONFIG_HOME"))
}

/// Like [`legacy_config_dir`], but takes `XDG_CONFIG_HOME` as a parameter.
fn legacy_config_dir_with(xdg: XdgDir) -> Result<PathBuf, HomeError> {
    let path = match xdg {
        XdgDir::Explicit(path) => path,
        XdgDir::Default => try_home_dir()?.joined(".config"),
    };
    Ok(path.joined("atuin"))
}

/// Get the data directory used when `data_dir` isn't set in config.toml.
///
/// This is `$ATUIN_HOME/data`, but for compatibility with older installations, will fall back to
/// [`legacy_data_dir`] if `ATUIN_HOME` is unset, the new path doesn't exist, and at least one of
/// the following is true:
///
/// * [`legacy_data_dir()`] exists.
/// * `~/.local/share/atuin` exists. This avoids creating `~/.atuin/data` when `XDG_DATA_HOME` is
///   set, which would shadow `~/.local/share/atuin` in future invocations of Atuin with
///   `XDG_DATA_HOME` unset.
///
/// You should almost always use `atuin_client::settings::Settings::data_dir` instead, since that
/// respects `data_dir` in config.toml.
#[must_use]
pub fn unconfigured_data_dir() -> PathBuf {
    match atuin_home() {
        AtuinHome::Set(path) => path.joined("data"),
        AtuinHome::Default(path) => {
            let path = path.joined("data");
            if !path.exists() && existing_legacy_data_dirs().next().is_some() {
                return legacy_data_dir();
            }
            path
        }
    }
}

/// Existing legacy data dirs: [`legacy_data_dir`], or `~/.local/share/atuin` even when
/// `XDG_DATA_HOME` points elsewhere, as it does for a test or a sandbox.
fn existing_legacy_data_dirs() -> impl Iterator<Item = PathBuf> {
    XdgDir::with_default("XDG_DATA_HOME")
        .filter_map(|xdg| legacy_data_dir_with(xdg).ok())
        .filter(|dir| dir.exists())
}

/// A legacy data dir that an existing `~/.atuin/data` hides: Atuin uses only the latter. Not when
/// `ATUIN_HOME` or `data_dir` in `config.toml` picks the data dir on purpose.
///
/// `configured_data_dir` is the value of `Settings::data_dir`. If it is something other than
/// `~/.atuin/data`, this function returns [`None`], since that indicates that `data_dir` has
/// intentionally been set to a custom path.
#[must_use]
pub fn shadowed_legacy_data_dir(configured_data_dir: &Path) -> Option<PathBuf> {
    let Ok(AtuinHome::Default(home)) = try_atuin_home() else {
        return None;
    };
    let default = home.joined("data");
    if configured_data_dir != default || !default.exists() {
        return None;
    }
    existing_legacy_data_dirs().find(|legacy| !legacy.is_same_path(&default))
}

/// Get the legacy data directory, used before the switch to [`atuin_home`].
///
/// This is `$XDG_DATA_HOME/atuin`, or `~/.local/share/atuin` if unset.
#[must_use]
pub fn legacy_data_dir() -> PathBuf {
    legacy_data_dir_with(XdgDir::from_var("XDG_DATA_HOME")).unwrap_or_else(|err| panic!("{err}"))
}

/// Like [`legacy_data_dir`], but takes `XDG_DATA_HOME` as a parameter.
fn legacy_data_dir_with(xdg: XdgDir) -> Result<PathBuf, HomeError> {
    let path = match xdg {
        XdgDir::Explicit(path) => path,
        XdgDir::Default => try_home_dir()?.joined(".local").joined("share"),
    };
    Ok(path.joined("atuin"))
}

/// Get the logs directory.
///
/// This is `$ATUIN_HOME/logs`.
#[must_use]
pub fn logs_dir() -> PathBuf {
    PathBuf::from(atuin_home()).joined("logs")
}

#[must_use]
pub fn dotfiles_cache_dir() -> PathBuf {
    // atuin-dotfiles was removed before the switch to `atuin_home`. It always stored the dotfiles
    // cache in ~/.local/share/atuin, ignoring the `data_dir` setting in config.toml.
    // `unconfigured_data_dir` will fall back to the legacy data dir when `ATUIN_HOME` is unset
    // and ~/.atuin/data doesn't exist, so we will continue looking for the dotfiles cache in the
    // same location. The reason we're using `unconfigured_data_dir` instead of calling
    // `legacy_data_dir` directly is in case a user has manually moved ~/.local/share/atuin to
    // ~/.atuin/data to migrate to the new layout -- that will move the dotfiles cache too.
    //
    // It would potentially be more correct to check `atuin_common::settings::Settings::data_dir`
    // first, since that would respect `data_dir` in config.toml, but that would involve significant
    // changes to atuin-dotfiles. Not worth it for a removed feature.
    unconfigured_data_dir().joined("dotfiles").joined("cache")
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};

    use super::*;
    use crate::env::MockEnv;

    #[cfg(not(windows))]
    #[rstest]
    fn test_legacy_config_dir_xdg(env: MockEnv) {
        env.set("XDG_CONFIG_HOME", "/home/user/custom_config");
        assert_eq!(legacy_config_dir(), Ok(PathBuf::from("/home/user/custom_config/atuin")));
    }

    /// An empty `XDG_CONFIG_HOME` has to be treated as unset: the alternative is a relative path.
    #[cfg(not(windows))]
    #[rstest]
    fn test_legacy_config_dir_xdg_empty(env: MockEnv) {
        env.set("HOME", "/home/user");
        env.set("XDG_CONFIG_HOME", "");
        assert_eq!(legacy_config_dir(), Ok(PathBuf::from("/home/user/.config/atuin")));
    }

    #[cfg(not(windows))]
    #[rstest]
    fn test_legacy_config_dir(env: MockEnv) {
        env.set("HOME", "/home/user");
        assert_eq!(legacy_config_dir(), Ok(PathBuf::from("/home/user/.config/atuin")));
    }

    #[cfg(not(windows))]
    #[rstest]
    fn test_legacy_data_dir_xdg(env: MockEnv) {
        env.set("XDG_DATA_HOME", "/home/user/custom_data");
        assert_eq!(legacy_data_dir(), PathBuf::from("/home/user/custom_data/atuin"));
    }

    /// An empty `XDG_DATA_HOME` has to be treated as unset: the alternative is a relative path.
    #[cfg(not(windows))]
    #[rstest]
    fn test_legacy_data_dir_xdg_empty(env: MockEnv) {
        env.set("HOME", "/home/user");
        env.set("XDG_DATA_HOME", "");
        assert_eq!(legacy_data_dir(), PathBuf::from("/home/user/.local/share/atuin"));
    }

    #[cfg(not(windows))]
    #[rstest]
    fn test_legacy_data_dir(env: MockEnv) {
        env.set("HOME", "/home/user");
        assert_eq!(legacy_data_dir(), PathBuf::from("/home/user/.local/share/atuin"));
    }

    #[cfg(not(windows))]
    #[rstest]
    fn test_atuin_home(env: MockEnv) {
        env.set("HOME", "/home/user");
        assert_eq!(atuin_home(), AtuinHome::Default("/home/user/.atuin".into()));
    }

    /// An empty `ATUIN_HOME` has to be treated as unset: the alternative is the working directory.
    #[cfg(not(windows))]
    #[rstest]
    fn test_atuin_home_empty(env: MockEnv) {
        env.set("HOME", "/home/user");
        env.set("ATUIN_HOME", "");
        assert_eq!(atuin_home(), AtuinHome::Default("/home/user/.atuin".into()));
    }

    #[cfg(not(windows))]
    #[rstest]
    fn test_atuin_home_absolute(env: MockEnv) {
        env.set("ATUIN_HOME", "/profiles/work");
        assert_eq!(atuin_home(), AtuinHome::Set("/profiles/work".into()));
    }

    /// A relative `ATUIN_HOME` is ignored: it would resolve differently depending on the working
    /// directory, so the daemon and the client could end up using different homes.
    #[cfg(not(windows))]
    #[rstest]
    fn test_atuin_home_relative(env: MockEnv) {
        env.set("HOME", "/home/user");
        env.set("ATUIN_HOME", "profile");
        assert_eq!(atuin_home(), AtuinHome::Default("/home/user/.atuin".into()));
    }

    #[rstest]
    #[case::set(AtuinHome::Set("/atuin".into()))]
    #[case::default(AtuinHome::Default("/atuin".into()))]
    fn atuin_home_converts_to_its_path(#[case] home: AtuinHome) {
        let expected = Path::new("/atuin");
        assert_eq!(home.as_ref(), expected);
        assert_eq!(home.join("data"), expected.join("data"));
        assert_eq!(PathBuf::from(home), expected);
    }

    /// A mock environment, in which every variable starts out unset.
    #[fixture]
    fn env() -> MockEnv {
        MockEnv::install()
    }

    /// `$HOME` and the legacy XDG directories pointed into a temporary directory, which is
    /// removed on drop. `$ATUIN_HOME` is unset unless a test sets it with
    /// [`Homes::set_atuin_home`].
    struct Homes {
        tmp: tempfile::TempDir,
        env: MockEnv,
    }

    impl Homes {
        /// The default `$ATUIN_HOME`, `~/.atuin`.
        fn new_home(&self) -> PathBuf {
            self.tmp.path().join("home").joined(".atuin")
        }

        /// Set `$ATUIN_THEME_DIR` to `value`.
        fn set_theme_dir(&self, value: &Path) {
            self.env.set("ATUIN_THEME_DIR", value);
        }

        /// Set `$ATUIN_HOME` to a directory other than [`Self::new_home`], and return it.
        fn set_atuin_home(&self) -> PathBuf {
            let path = self.tmp.path().join("profile");
            self.env.set("ATUIN_HOME", &path);
            path
        }

        fn legacy_config(&self) -> PathBuf {
            self.tmp.path().join("xdg-config").joined("atuin")
        }

        /// Create `~/.atuin/config.toml`, which marks an installation that uses `~/.atuin` (a new
        /// one, or one that has moved there) rather than the legacy config dir.
        fn migrate(&self) {
            create(&self.new_home().joined("config.toml"));
        }

        fn legacy_data(&self) -> PathBuf {
            self.tmp.path().join("xdg-data").joined("atuin")
        }

        fn config_path(&self, child: &str, atuin_config_dir: Option<&str>) -> PathBuf {
            match atuin_config_dir {
                Some(dir) => self.env.set("ATUIN_CONFIG_DIR", dir),
                None => self.env.remove("ATUIN_CONFIG_DIR"),
            }
            config_path(child)
        }
    }

    /// Create `path` as a file if it has an extension, else as a directory.
    fn create(path: &Path) {
        if path.extension().is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        } else {
            std::fs::create_dir_all(path).unwrap();
        }
    }

    #[fixture]
    fn homes(env: MockEnv) -> Homes {
        let tmp = tempfile::tempdir().unwrap();
        env.set("HOME", tmp.path().join("home"));
        env.set("XDG_CONFIG_HOME", tmp.path().join("xdg-config"));
        env.set("XDG_DATA_HOME", tmp.path().join("xdg-data"));
        Homes { tmp, env }
    }

    #[rstest]
    fn unconfigured_data_dir_is_new_by_default(homes: Homes) {
        let expected = homes.new_home().joined("data");
        assert_eq!(unconfigured_data_dir(), expected);
    }

    #[rstest]
    fn unconfigured_data_dir_is_legacy_when_only_legacy_exists(homes: Homes) {
        create(&homes.legacy_data());
        // `$ATUIN_HOME` itself existing (e.g. for logs) doesn't count.
        create(&homes.new_home().joined("logs"));
        assert_eq!(unconfigured_data_dir(), homes.legacy_data());
    }

    #[rstest]
    fn unconfigured_data_dir_is_new_when_both_exist(homes: Homes) {
        create(&homes.legacy_data());
        create(&homes.new_home().joined("data"));
        let expected = homes.new_home().joined("data");
        assert_eq!(unconfigured_data_dir(), expected);
    }

    #[rstest]
    fn unconfigured_data_dir_ignores_legacy_when_atuin_home_is_set(homes: Homes) {
        create(&homes.legacy_data());
        let atuin_home = homes.set_atuin_home();
        assert_eq!(unconfigured_data_dir(), atuin_home.joined("data"));
    }

    #[rstest]
    fn legacy_config_is_shadowed_once_atuin_home_has_one(homes: Homes) {
        create(&homes.legacy_config().joined("config.toml"));
        assert_eq!(shadowed_legacy_config(), None);

        homes.migrate();
        assert_eq!(config_path("config.toml"), homes.new_home().joined("config.toml"));
        assert_eq!(shadowed_legacy_config(), Some(homes.legacy_config().joined("config.toml")));

        homes.set_atuin_home();
        assert_eq!(shadowed_legacy_config(), None, "ATUIN_HOME picks the config on purpose");
    }

    /// With `XDG_CONFIG_HOME` pointed away from `~/.config/atuin/config.toml`, a new config goes in
    /// `$XDG_CONFIG_HOME/atuin`, as before `~/.atuin`, rather than in a `~/.atuin` that would then
    /// hide it.
    #[rstest]
    fn missing_config_avoids_hiding_the_home_config(
        homes: Homes,
        #[values("config.toml", "server.toml")] item: &str,
    ) {
        assert_eq!(config_path(item), homes.new_home().joined(item), "a fresh install");

        create(&homes.tmp.path().join("home/.config/atuin").joined(item));
        assert_eq!(config_path(item), homes.legacy_config().joined(item));
        assert_eq!(shadowed_legacy_config(), None, "nothing hides it yet");

        let cfg = homes.tmp.path().join("cfg");
        homes.env.set("ATUIN_CONFIG_DIR", &cfg);
        assert_eq!(config_path(item), cfg.joined(item), "ATUIN_CONFIG_DIR picks it");

        homes.env.remove("ATUIN_CONFIG_DIR");
        let atuin_home = homes.set_atuin_home();
        assert_eq!(config_path(item), atuin_home.joined(item), "ATUIN_HOME picks it");
    }

    /// Items that Atuin doesn't create can't come to hide `~/.config/atuin`'s.
    #[rstest]
    fn missing_uncreated_item_resolves_to_atuin_home(homes: Homes) {
        create(&homes.tmp.path().join("home/.config/atuin/themes"));
        assert_eq!(config_path("themes"), homes.new_home().joined("themes"));
    }

    /// With `XDG_DATA_HOME` pointed away from `~/.local/share/atuin`, the data dir is
    /// `$XDG_DATA_HOME/atuin`, as before `~/.atuin`, rather than a `~/.atuin/data` that, once
    /// created, would hide it.
    #[rstest]
    fn unconfigured_data_dir_avoids_hiding_the_home_data_dir(homes: Homes) {
        let home_data = homes.tmp.path().join("home/.local/share/atuin");
        create(&home_data);
        assert_eq!(unconfigured_data_dir(), homes.legacy_data());
        assert_eq!(shadowed_legacy_data_dir(&homes.legacy_data()), None, "nothing hides it");
    }

    #[rstest]
    fn legacy_data_dir_is_shadowed_once_atuin_home_has_one(
        homes: Homes,
        #[values(false, true)] xdg_data: bool,
    ) {
        let legacy = if xdg_data {
            homes.legacy_data()
        } else {
            homes.tmp.path().join("home/.local/share/atuin")
        };
        create(&legacy);
        let new_data = homes.new_home().joined("data");
        assert_eq!(shadowed_legacy_data_dir(&new_data), None, "~/.atuin/data doesn't exist");

        create(&new_data);
        assert_eq!(unconfigured_data_dir(), new_data);
        assert_eq!(shadowed_legacy_data_dir(&new_data), Some(legacy));

        let custom = homes.tmp.path().join("custom");
        assert_eq!(shadowed_legacy_data_dir(&custom), None, "data_dir picks it on purpose");

        homes.set_atuin_home();
        let set = shadowed_legacy_data_dir(&new_data);
        assert_eq!(set, None, "ATUIN_HOME picks it on purpose");
    }

    #[rstest]
    fn config_items_are_resolved_independently(
        homes: Homes,
        #[values(
            "config.toml",
            "server.toml",
            "TERMINAL.md",
            "permissions.ai.toml",
            "skills",
            "themes"
        )]
        item: &str,
        #[values(false, true)] in_legacy: bool,
        #[values(false, true)] in_new: bool,
        #[values(false, true)] migrated: bool,
    ) {
        // A sibling item living in the legacy dir mustn't drag this one along with it, nor the
        // other way round.
        create(&homes.legacy_config().joined("sibling.toml"));
        create(&homes.new_home().joined("other.toml"));
        if in_legacy {
            create(&homes.legacy_config().joined(item));
        }
        if in_new {
            create(&homes.new_home().joined(item));
        }
        if migrated {
            homes.migrate();
        }
        let in_new = in_new || (migrated && item == "config.toml");

        // Until the installation uses `~/.atuin`, Atuin AI's items stay in the legacy dir.
        let base = if (AI_ITEMS.contains(&item) && !migrated) || (in_legacy && !in_new) {
            homes.legacy_config()
        } else {
            homes.new_home()
        };
        assert_eq!(homes.config_path(item, None), base.joined(item));
    }

    #[rstest]
    fn nested_config_paths_are_resolved_by_their_top_level_item(homes: Homes) {
        homes.migrate();
        create(&homes.legacy_config().joined("themes"));
        assert_eq!(
            homes.config_path("themes/dark.toml", None),
            homes.legacy_config().joined("themes/dark.toml")
        );
        assert_eq!(
            homes.config_path("skills/foo/SKILL.md", None),
            homes.new_home().joined("skills/foo/SKILL.md")
        );
    }

    /// Before the switch to `~/.atuin`, `~/.atuin/permissions.ai.toml` held approvals for the home
    /// directory only. Until the installation uses `~/.atuin`, it must not become the global file,
    /// even when there's no legacy global file: that would make its approvals apply everywhere.
    #[rstest]
    fn an_old_home_scoped_ai_file_is_not_the_global_one(
        homes: Homes,
        #[values("permissions.ai.toml", "TERMINAL.md", "skills")] item: &str,
    ) {
        create(&homes.new_home().joined(item));
        assert_eq!(homes.config_path(item, None), homes.legacy_config().joined(item));

        // Once the installation uses `~/.atuin`, that's where they are.
        homes.migrate();
        assert_eq!(homes.config_path(item, None), homes.new_home().joined(item));
    }

    /// Atuin AI's items stay in the legacy dir without a home directory, which works when
    /// `XDG_CONFIG_HOME` says where that is.
    #[rstest]
    fn ai_items_do_without_a_home_dir_given_xdg_config_home(
        env: MockEnv,
        #[values("permissions.ai.toml", "TERMINAL.md", "skills/foo/SKILL.md")] child: &str,
    ) {
        let config: PathBuf = if cfg!(windows) {
            "c:\\xdg-config"
        } else {
            "/xdg-config"
        }
        .into();
        env.set("XDG_CONFIG_HOME", &config);
        assert_eq!(try_home_dir(), Err(HomeError));
        assert_eq!(config_path(child), config.joined("atuin").joined(child));
    }

    /// Which Atuin home an installation uses.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Install {
        /// `ATUIN_HOME` is set.
        AtuinHomeSet,
        /// The default `~/.atuin`, on an installation from before it, which hasn't moved there.
        Legacy,
        /// The default `~/.atuin`, on a new installation or one that has moved there.
        Migrated,
    }

    #[rstest]
    fn config_items_are_looked_up_in_priority_order(
        homes: Homes,
        // Whether each item is one `$ATUIN_CONFIG_DIR` relocates, whether it is a theme, which is
        // still read from there for backward compatibility, and whether it is one of Atuin AI's,
        // which stay in the legacy dir until the installation uses `~/.atuin`.
        #[values(
            ("config.toml", true, false, false),
            ("server.toml", true, false, false),
            ("themes/dark.toml", false, true, false),
            ("permissions.ai.toml", false, false, true),
            ("TERMINAL.md", false, false, true),
            ("skills/foo/SKILL.md", false, false, true)
        )]
        (child, relocated, theme, ai): (&str, bool, bool, bool),
        #[values(false, true)] config_dir_set: bool,
        #[values(false, true)] in_config_dir: bool,
        #[values(Install::AtuinHomeSet, Install::Legacy, Install::Migrated)] install: Install,
        #[values(false, true)] in_home: bool,
        #[values(false, true)] in_legacy: bool,
    ) {
        let atuin_home_set = install == Install::AtuinHomeSet;
        let migrated = install == Install::Migrated;
        let home = if atuin_home_set {
            homes.set_atuin_home()
        } else {
            homes.new_home()
        };
        let config_dir = homes.tmp.path().join("config-dir");
        for (dir, present) in [
            (config_dir.clone(), in_config_dir),
            (home.clone(), in_home),
            (homes.legacy_config(), in_legacy),
        ] {
            if present {
                create(&dir.joined(child));
            }
        }
        if migrated {
            homes.migrate();
        }
        let in_home = in_home || (migrated && !atuin_home_set && child == "config.toml");

        // A set `ATUIN_HOME` is used whether or not the item exists there; the fallbacks only
        // apply to the default `~/.atuin`.
        let base = if config_dir_set && relocated {
            config_dir.clone()
        } else if atuin_home_set {
            home
        } else if ai && !migrated {
            homes.legacy_config()
        } else if in_home {
            home
        } else if theme && config_dir_set && in_config_dir {
            config_dir.clone()
        } else if in_legacy {
            homes.legacy_config()
        } else {
            home
        };
        let config_dir = config_dir_set.then(|| config_dir.to_str().unwrap());
        assert_eq!(homes.config_path(child, config_dir), base.joined(child));
    }

    /// `ATUIN_THEME_DIR` is only a fallback for the default `~/.atuin`, when it has no themes; a
    /// set `ATUIN_HOME` is used for themes whether or not it has any.
    #[rstest]
    fn atuin_theme_dir_is_used_only_without_atuin_home_or_its_themes(
        homes: Homes,
        #[values(("themes", ""), ("themes/dark.toml", "dark.toml"))] (child, within): (&str, &str),
        #[values(false, true)] atuin_home_set: bool,
        #[values(false, true)] in_home: bool,
    ) {
        let home = if atuin_home_set {
            homes.set_atuin_home()
        } else {
            homes.new_home()
        };
        // The themes exist in `$ATUIN_CONFIG_DIR` and the legacy dir, but not in
        // `$ATUIN_THEME_DIR`.
        let config_dir = homes.tmp.path().join("config-dir");
        for dir in [&config_dir, &homes.legacy_config()] {
            create(&dir.join("themes"));
        }
        if in_home {
            create(&home.join("themes"));
        }
        let theme_dir = homes.tmp.path().join("theme-dir");
        homes.set_theme_dir(&theme_dir);

        let expected = if atuin_home_set || in_home {
            home.joined(child)
        } else {
            theme_dir.joined(within)
        };
        let config_dir = config_dir.to_str().unwrap();
        assert_eq!(homes.config_path(child, Some(config_dir)), expected);
    }

    #[rstest]
    fn atuin_theme_dir_is_ignored_for_other_items(
        homes: Homes,
        #[values("config.toml", "TERMINAL.md", "skills/foo/SKILL.md")] child: &str,
    ) {
        homes.migrate();
        homes.set_theme_dir(&homes.tmp.path().join("theme-dir"));
        assert_eq!(homes.config_path(child, None), homes.new_home().joined(child));
    }

    /// An empty `ATUIN_THEME_DIR` has to be treated as unset.
    #[rstest]
    fn empty_atuin_theme_dir_is_unset(homes: Homes) {
        homes.set_theme_dir(Path::new(""));
        assert_eq!(
            homes.config_path("themes/dark.toml", None),
            homes.new_home().joined("themes/dark.toml")
        );
    }

    /// Items that don't need the home directory are still found when it can't be determined.
    #[rstest]
    #[case::atuin_config_dir(
        "config.toml",
        "ATUIN_CONFIG_DIR",
        "/etc/atuin",
        "/etc/atuin/config.toml"
    )]
    #[case::atuin_theme_dir("themes/dark.toml", "ATUIN_THEME_DIR", "/themes", "/themes/dark.toml")]
    fn config_path_does_without_a_home_dir_where_it_can(
        env: MockEnv,
        #[case] child: &str,
        #[case] var: &str,
        #[case] dir: &str,
        #[case] expected: &str,
    ) {
        env.set(var, dir);
        assert_eq!(try_home_dir(), Err(HomeError));
        assert_eq!(config_path(child), Path::new(expected));
    }

    #[rstest]
    #[should_panic(expected = "could not determine home directory")]
    fn config_path_panics_without_a_home_dir_as_a_last_resort() {
        let _env = MockEnv::install();
        let _ = config_path("skills");
    }

    #[rstest]
    #[case::empty("")]
    #[case::absolute("/etc/config.toml")]
    #[case::parent("../config.toml")]
    #[should_panic(expected = "must be relative")]
    fn config_path_rejects_non_items(homes: Homes, #[case] child: &str) {
        let _ = homes.config_path(child, None);
    }
}
