//! Which Hermes home an install writes to: the one a bare `hermes` uses, so the plugin and the
//! MCP server land where the user's Hermes looks. Reading sessions is wider: the reader scans the
//! root and every profile.

use std::path::{Path, PathBuf};

use crate::path::PathBufExt as _;

/// A Hermes home to install into, and the profile that names it to `hermes -p`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallHome {
    path: PathBuf,
    profile: String,
}

/// What `-p` takes for a Hermes root.
pub(super) const DEFAULT: &str = "default";

impl InstallHome {
    /// The home a bare `hermes` uses, given `home` (`HERMES_HOME`, else `~/.hermes`). As Hermes
    /// resolves it (`_apply_profile_override` in `hermes_cli/main.py`, v0.23.0): a `home` that is
    /// a profile's directory (`<root>/profiles/<name>`) as it is; else the profile that
    /// `<home>/active_profile` names (`hermes profile use`); else `home` itself, the root.
    ///
    /// An `active_profile` that names no profile here, or isn't a valid name, leaves the root:
    /// Hermes refuses to start with one, so there is no home it would use instead.
    #[must_use]
    pub fn resolve(home: &Path) -> Self {
        if let Some(name) = profile_dir_name(home) {
            return Self {
                path: home.to_owned(),
                profile: name.to_owned(),
            };
        }
        let active = std::fs::read_to_string(home.join("active_profile")).ok().and_then(|text| {
            let name = text.trim_start_matches('\u{feff}').trim().to_lowercase();
            (is_profile_name(&name) && name != DEFAULT).then_some(name)
        });
        match active {
            Some(name) if home.join("profiles").joined(&name).is_dir() => Self {
                path: home.join("profiles").joined(&name),
                profile: name,
            },
            _ => Self {
                path: home.to_owned(),
                profile: DEFAULT.to_owned(),
            },
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What `hermes -p` takes to reach [`Self::path`]: the profile's name, or `default` for a
    /// root.
    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }
}

/// The profile `home` is the directory of, if it is one.
fn profile_dir_name(home: &Path) -> Option<&str> {
    if home.parent()?.file_name()? != "profiles" {
        return None;
    }
    home.file_name()?.to_str()
}

/// Hermes's profile names: `^[a-z0-9][a-z0-9_-]{0,63}$` (`_PROFILE_NAME_RE`).
fn is_profile_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// A root holding the profile `work`, and the `active_profile` file it has, if any.
    fn root(active_profile: Option<&str>) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("profiles").joined("work")).unwrap();
        if let Some(active) = active_profile {
            std::fs::write(root.path().join("active_profile"), active).unwrap();
        }
        root
    }

    #[rstest]
    #[case::no_active_profile(None)]
    #[case::default(Some("default\n"))]
    #[case::default_in_capitals(Some("Default"))]
    #[case::empty(Some(""))]
    #[case::missing_profile(Some("play"))]
    #[case::not_a_name(Some("../work"))]
    fn the_root_unless_a_profile_is_active(#[case] active_profile: Option<&str>) {
        let root = root(active_profile);

        let home = InstallHome::resolve(root.path());

        assert_eq!(home.path(), root.path());
        assert_eq!(home.profile(), "default");
    }

    #[rstest]
    #[case::plain("work")]
    #[case::as_hermes_writes_it("work\n")]
    #[case::in_capitals("Work")]
    #[case::with_a_byte_order_mark("\u{feff}work\n")]
    fn the_active_profile(#[case] active_profile: &str) {
        let root = root(Some(active_profile));

        let home = InstallHome::resolve(root.path());

        assert_eq!(home.path(), root.path().join("profiles").joined("work"));
        assert_eq!(home.profile(), "work");
    }

    /// `HERMES_HOME` naming a profile is that profile, whatever the root's `active_profile` says.
    #[rstest]
    fn a_profile_directory_as_it_is() {
        let root = root(Some("play"));
        let work = root.path().join("profiles").joined("work");

        let home = InstallHome::resolve(&work);

        assert_eq!(home.path(), work);
        assert_eq!(home.profile(), "work");
    }

    #[rstest]
    #[case::short("a", true)]
    #[case::digits_and_marks("0-work_2", true)]
    #[case::longest(&"a".repeat(64), true)]
    #[case::too_long(&"a".repeat(65), false)]
    #[case::leading_mark("-work", false)]
    #[case::capitals("Work", false)]
    #[case::path("../work", false)]
    #[case::empty("", false)]
    fn profile_names(#[case] name: &str, #[case] valid: bool) {
        assert_eq!(is_profile_name(name), valid);
    }
}
