//! Provides the location of Atuin's files.

use std::path::{Path, PathBuf};

use crate::env::{var_abspath, var_nonempty};

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
        return Ok(AtuinHome::Set(value));
    }
    let mut path = try_home_dir()?;
    path.push(".atuin");
    Ok(AtuinHome::Default(path))
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

/// Top-level config items affected by the `ATUIN_CONFIG_DIR` environment variable.
const ATUIN_CONFIG_DIR_ITEMS: [&str; 2] = ["config.toml", "server.toml"];

/// Get the path of a config item.
///
/// `child` is relative to the config directory. The final path is determined as follows:
///
/// 1. If `child` is `config.toml` or `server.toml` and `ATUIN_CONFIG_DIR` is set,
///    `$ATUIN_CONFIG_DIR/child` is returned.
/// 2. If the first component of `child` exists in [`atuin_home()`], `atuin_home()/child` is
///    returned.
/// 3. If the first component of `child` is `themes` and `ATUIN_THEME_DIR` is set,
///    `$ATUIN_THEME_DIR` is returned. This behavior is deprecated and exists only for backward
///    compatibility.
/// 4. If the first component of `child` is `themes` and `ATUIN_CONFIG_DIR` is set,
///    `$ATUIN_CONFIG_DIR/themes` is returned. This behavior is deprecated and exists only for
///    backward compatibility.
/// 5. If the first component of `child` exists in `legacy_config_dir()`[^1] and `ATUIN_HOME` is not
///    set, `legacy_config_dir()/child` is returned.
/// 6. Otherwise, `atuin_home()/child` is returned.
///
/// # Panics
///
/// Panics if `child` is empty or absolute, or if it starts with `.` or `..`. Also panics if it
/// gets to step 6 and the home directory can't be determined; the earlier steps don't need it.
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
        let mut path = PathBuf::from(value);
        path.push(child);
        return path;
    }

    // The home directory may be unavailable (e.g. a server running as a user without one), so
    // only the last resort below, which can't do without it, panics over that.
    let home = try_atuin_home();
    if let Ok(home) = &home {
        let mut path = PathBuf::from(home.clone());
        path.push(top);
        if path.exists() {
            path.pop(); // pop `top` from the path
            path.push(child);
            return path;
        }
    }

    let atuin_home_is_set = matches!(home, Ok(AtuinHome::Set(_)));
    // If `top` is "themes", check legacy theme locations, but only if `ATUIN_HOME` isn't set.
    let check_legacy_themes = !atuin_home_is_set && top == "themes";
    if check_legacy_themes && let Some(value) = var_nonempty("ATUIN_THEME_DIR") {
        // For backward compatibility, keep respecting the deprecated `ATUIN_THEME_DIR`
        // variable, but only if `$ATUIN_HOME/themes` doesn't exist.
        let mut path = PathBuf::from(value);
        path.extend(child.components().skip(1));
        return path;
    }

    // Potential directories that might contain the config item. The first one that contains the
    // item, if any, is used.
    let potential_dirs = [
        // Historically, `ATUIN_CONFIG_DIR` affected the location of `config.toml`,
        // `server.toml`, and `themes`, but not any other config items such as `skills` or
        // `permissions.ai.toml`. This behavior has been simplified: `ATUIN_CONFIG_DIR` only
        // affects `config.toml` and `server.toml`; using it to control `themes` is deprecated.
        //
        // For backward compatibility, read themes from `$ATUIN_CONFIG_DIR/themes`, but only
        // if `$ATUIN_HOME/themes` doesn't exist.
        check_legacy_themes.then(|| var_nonempty("ATUIN_CONFIG_DIR")).flatten().map(Into::into),
        // Atuin used to store config items in `$XDG_CONFIG_HOME/atuin`. For backward
        // compatibility, continue to read items from there when they don't exist in
        // `$ATUIN_HOME`, but only if `ATUIN_HOME` is unset -- setting `ATUIN_HOME` should
        // always give you a fresh profile.
        (!atuin_home_is_set).then(legacy_config_dir).and_then(Result::ok),
    ];

    if let Some(mut existing) = potential_dirs
        .into_iter()
        .flatten()
        .map(|mut dir| {
            dir.push(top);
            dir
        })
        .find(|dir| dir.exists())
    {
        existing.pop(); // pop `top` from the path
        existing.push(child);
        return existing;
    }

    let mut path = PathBuf::from(home.unwrap_or_else(|err| panic!("{err}")));
    path.push(child);
    path
}

/// Get the legacy config directory, used before the switch to [`atuin_home`].
///
/// This is `$XDG_CONFIG_HOME/atuin`, or `~/.config/atuin` if unset.
///
/// # Errors
///
/// If `XDG_CONFIG_HOME` is unset and the home directory can't be determined.
fn legacy_config_dir() -> Result<PathBuf, HomeError> {
    let mut path = match var_abspath("XDG_CONFIG_HOME") {
        Some(path) => path,
        None => {
            let mut path = try_home_dir()?;
            path.push(".config");
            path
        }
    };
    path.push("atuin");
    Ok(path)
}

/// Get the data directory used when `data_dir` isn't set in config.toml.
///
/// This is `$ATUIN_HOME/data`, but will fall back to [`legacy_data_dir`] if only that path exists
/// and `ATUIN_HOME` is unset, for compatibility with older installations.
///
/// You should almost always use `atuin_client::settings::Settings::data_dir` instead, since that
/// respects `data_dir` in config.toml.
#[must_use]
pub fn unconfigured_data_dir() -> PathBuf {
    match atuin_home() {
        AtuinHome::Set(mut path) => {
            path.push("data");
            path
        }
        AtuinHome::Default(mut path) => {
            path.push("data");
            if !path.exists() {
                let old_path = legacy_data_dir();
                if old_path.exists() {
                    return old_path;
                }
            }
            path
        }
    }
}

/// Get the legacy data directory, used before the switch to [`atuin_home`].
///
/// This is `$XDG_DATA_HOME/atuin`, or `~/.local/share/atuin` if unset.
#[must_use]
pub fn legacy_data_dir() -> PathBuf {
    let mut path = var_abspath("XDG_DATA_HOME").unwrap_or_else(|| {
        let mut path = home_dir();
        path.push(".local");
        path.push("share");
        path
    });
    path.push("atuin");
    path
}

/// Get the logs directory.
///
/// This is `$ATUIN_HOME/logs`.
#[must_use]
pub fn logs_dir() -> PathBuf {
    let mut path = PathBuf::from(atuin_home());
    path.push("logs");
    path
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
    let mut path = unconfigured_data_dir();
    path.push("dotfiles");
    path.push("cache");
    path
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
            self.tmp.path().join("home").join(".atuin")
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
            self.tmp.path().join("xdg-config").join("atuin")
        }

        fn legacy_data(&self) -> PathBuf {
            self.tmp.path().join("xdg-data").join("atuin")
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
        let expected = homes.new_home().join("data");
        assert_eq!(unconfigured_data_dir(), expected);
    }

    #[rstest]
    fn unconfigured_data_dir_is_legacy_when_only_legacy_exists(homes: Homes) {
        create(&homes.legacy_data());
        // `$ATUIN_HOME` itself existing (e.g. for logs) doesn't count.
        create(&homes.new_home().join("logs"));
        assert_eq!(unconfigured_data_dir(), homes.legacy_data());
    }

    #[rstest]
    fn unconfigured_data_dir_is_new_when_both_exist(homes: Homes) {
        create(&homes.legacy_data());
        create(&homes.new_home().join("data"));
        let expected = homes.new_home().join("data");
        assert_eq!(unconfigured_data_dir(), expected);
    }

    #[rstest]
    fn unconfigured_data_dir_ignores_legacy_when_atuin_home_is_set(homes: Homes) {
        create(&homes.legacy_data());
        let atuin_home = homes.set_atuin_home();
        assert_eq!(unconfigured_data_dir(), atuin_home.join("data"));
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
    ) {
        // A sibling item living in the legacy dir mustn't drag this one along with it, nor the
        // other way round.
        create(&homes.legacy_config().join("sibling.toml"));
        create(&homes.new_home().join("other.toml"));
        if in_legacy {
            create(&homes.legacy_config().join(item));
        }
        if in_new {
            create(&homes.new_home().join(item));
        }

        let base = if in_legacy && !in_new {
            homes.legacy_config()
        } else {
            homes.new_home()
        };
        assert_eq!(homes.config_path(item, None), base.join(item));
    }

    #[rstest]
    fn nested_config_paths_are_resolved_by_their_top_level_item(homes: Homes) {
        create(&homes.legacy_config().join("themes"));
        assert_eq!(
            homes.config_path("themes/dark.toml", None),
            homes.legacy_config().join("themes/dark.toml")
        );
        assert_eq!(
            homes.config_path("skills/foo/SKILL.md", None),
            homes.new_home().join("skills/foo/SKILL.md")
        );
    }

    #[rstest]
    fn config_items_are_looked_up_in_priority_order(
        homes: Homes,
        // Whether each item is one `$ATUIN_CONFIG_DIR` relocates, and whether it is a theme, which
        // is still read from there for backward compatibility.
        #[values(
            ("config.toml", true, false),
            ("server.toml", true, false),
            ("themes/dark.toml", false, true),
            ("TERMINAL.md", false, false),
            ("skills/foo/SKILL.md", false, false)
        )]
        (child, relocated, theme): (&str, bool, bool),
        #[values(false, true)] config_dir_set: bool,
        #[values(false, true)] in_config_dir: bool,
        #[values(false, true)] atuin_home_set: bool,
        #[values(false, true)] in_home: bool,
        #[values(false, true)] in_legacy: bool,
    ) {
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
                create(&dir.join(child));
            }
        }

        let base = if config_dir_set && relocated {
            config_dir.clone()
        } else if in_home {
            home
        } else if theme && config_dir_set && in_config_dir {
            config_dir.clone()
        } else if in_legacy && !atuin_home_set {
            homes.legacy_config()
        } else {
            home
        };
        let config_dir = config_dir_set.then(|| config_dir.to_str().unwrap());
        assert_eq!(homes.config_path(child, config_dir), base.join(child));
    }

    #[rstest]
    fn atuin_theme_dir_is_used_unless_atuin_home_has_themes(
        homes: Homes,
        #[values(("themes", ""), ("themes/dark.toml", "dark.toml"))] (child, within): (&str, &str),
        #[values(false, true)] in_home: bool,
    ) {
        // The themes exist in `$ATUIN_CONFIG_DIR` and the legacy dir, but not in
        // `$ATUIN_THEME_DIR`.
        let config_dir = homes.tmp.path().join("config-dir");
        for dir in [&config_dir, &homes.legacy_config()] {
            create(&dir.join("themes"));
        }
        if in_home {
            create(&homes.new_home().join("themes"));
        }
        let theme_dir = homes.tmp.path().join("theme-dir");
        homes.set_theme_dir(&theme_dir);

        let expected = if in_home {
            homes.new_home().join(child)
        } else {
            theme_dir.join(within)
        };
        let config_dir = config_dir.to_str().unwrap();
        assert_eq!(homes.config_path(child, Some(config_dir)), expected);
    }

    #[rstest]
    fn atuin_theme_dir_is_ignored_for_other_items(
        homes: Homes,
        #[values("config.toml", "TERMINAL.md", "skills/foo/SKILL.md")] child: &str,
    ) {
        homes.set_theme_dir(&homes.tmp.path().join("theme-dir"));
        assert_eq!(homes.config_path(child, None), homes.new_home().join(child));
    }

    /// An empty `ATUIN_THEME_DIR` has to be treated as unset.
    #[rstest]
    fn empty_atuin_theme_dir_is_unset(homes: Homes) {
        homes.set_theme_dir(Path::new(""));
        assert_eq!(
            homes.config_path("themes/dark.toml", None),
            homes.new_home().join("themes/dark.toml")
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
