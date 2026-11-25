//! Variables that keep applications from exposing their content to accessibility clients.

use std::path::{Path, PathBuf};

use atuin_common::dirs::try_home_dir;
use atuin_common::path::PathBufExt as _;
use atuin_desktop_capture::{Desktop, Platform as _, SessionVariable};

use super::session::ENVIRONMENT_FILE;
use crate::i18n::fl;

/// A variable that hides applications, at the value that hides them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
#[display("{}={}", variable.name, variable.hiding_value)]
pub struct Rule {
    variable: &'static SessionVariable,
}

/// The variables that hide applications, which the desktop backend names.
fn rules() -> impl Iterator<Item = Rule> {
    Desktop::session_variables().iter().map(|variable| Rule { variable })
}

/// Where a hiding variable is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The environment of this process.
    ThisSession,
    File(PathBuf),
}

/// A variable that hides applications, and where it is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hiding {
    pub rule: Rule,
    pub source: Source,
}

impl Hiding {
    /// Where the variable is set and what it does, in a phrase for the user.
    pub fn describe(&self) -> String {
        let assignment = self.rule.to_string();
        match &self.source {
            Source::ThisSession => fl!("desktop-capture-hiding-session", assignment = assignment),
            Source::File(path) => fl!(
                "desktop-capture-hiding-file",
                assignment = assignment,
                path = path.display().to_string()
            ),
        }
    }

    /// Where the variable is set, and that setup's login files override it, in a phrase for the
    /// user.
    pub fn describe_overridden(&self) -> String {
        let assignment = self.rule.to_string();
        match &self.source {
            Source::ThisSession => {
                fl!("desktop-capture-hiding-session-overridden", assignment = assignment)
            }
            Source::File(path) => fl!(
                "desktop-capture-hiding-file-overridden",
                assignment = assignment,
                path = path.display().to_string()
            ),
        }
    }

    /// Whether the variables that setup's login files set take the place of this assignment.
    ///
    /// `found` holds every hiding variable that setup found. An assignment of this session counts
    /// as overridden unless a file in `found` that setup doesn't override explains it.
    pub fn is_overridden(&self, found: &[Self], files: SetupFiles) -> bool {
        let set_by_setup = self.rule.variable.setup_value().is_some();
        set_by_setup && match &self.source {
            Source::ThisSession => !found.iter().any(|other| {
                other.rule == self.rule
                    && matches!(&other.source, Source::File(path) if !overrides_file(path, files))
            }),
            Source::File(path) => overrides_file(path, files),
        }
    }
}

/// Which files setup writes for the sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupFiles {
    /// The login shells' files and the file in `environment.d`.
    WithEnvironmentFile,
    /// The login shells' files only, as on a system without systemd.
    WithoutEnvironmentFile,
}

/// Whether setup's `files` set a variable again after every session read it from `path`.
///
/// The service manager reads the files of every `environment.d` in the order of their names, and
/// openSUSE's `sway-run.sh` reads them after `/etc/sway/env`. systemd links `/etc/environment`
/// into `environment.d` as `99-environment.conf`, after setup's file, so the sessions that take
/// their variables from the service manager keep what `/etc/environment` sets.
fn overrides_file(path: &Path, files: SetupFiles) -> bool {
    if path == Path::new("/etc/environment") {
        return false;
    }
    let in_environment_d = path.parent().is_some_and(|dir| dir.ends_with("environment.d"));
    let read_after_setups_file = in_environment_d
        && path
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes() > ENVIRONMENT_FILE.as_bytes());
    files == SetupFiles::WithEnvironmentFile && !read_after_setups_file
}

/// Finds the hiding variables in this process's environment and in the files that can set
/// variables for any session.
pub fn find() -> impl Iterator<Item = Hiding> {
    let here = rules()
        .filter(|rule| {
            let SessionVariable {
                name, hiding_value, ..
            } = rule.variable;
            atuin_common::env::var_os(name).is_some_and(|value| value == *hiding_value)
        })
        .map(|rule| Hiding {
            rule,
            source: Source::ThisSession,
        });
    let in_files = files().flat_map(|file| {
        let content = std::fs::read_to_string(&file).unwrap_or_default();
        // The hits borrow `content`, which lives only in this closure.
        hiding_in(&content)
            .map(|rule| Hiding {
                rule,
                source: Source::File(file.clone()),
            })
            .collect::<Vec<_>>()
    });
    here.chain(in_files)
}

/// The files that can set variables for any session.
fn files() -> impl Iterator<Item = PathBuf> {
    let user = try_home_dir().ok().map(|home| home.joined(".config/environment.d"));
    let dirs = std::iter::once(PathBuf::from("/etc/environment.d")).chain(user);
    std::iter::once(PathBuf::from("/etc/environment"))
        .chain(dirs.flat_map(|dir| conf_files(&dir)))
        .chain(std::iter::once(PathBuf::from("/etc/sway/env")))
}

/// The `.conf` files of `dir`, in the order that systemd reads them.
fn conf_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "conf"))
        .collect();
    files.sort();
    files
}

/// The rules that the assignments in `content` break, in the syntax of `environment.d` or of
/// `sh`.
fn hiding_in(content: &str) -> impl Iterator<Item = Rule> + '_ {
    content.lines().filter_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (name, value) = line.split_once('=')?;
        let name = name.trim();
        let value = value.trim().trim_matches(['"', '\'']);
        rules().find(|rule| rule.variable.name == name && rule.variable.hiding_value == value)
    })
}

/// The rules come from the Linux backend, the only one that names session variables.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::plain("NO_AT_BRIDGE=1", Some("NO_AT_BRIDGE=1"))]
    #[case::exported("export QT_ACCESSIBILITY=0", Some("QT_ACCESSIBILITY=0"))]
    #[case::quoted("GTK_A11Y=\"none\"", Some("GTK_A11Y=none"))]
    #[case::spaces("  ACCESSIBILITY_ENABLED = '0'  ", Some("ACCESSIBILITY_ENABLED=0"))]
    #[case::comment("# NO_AT_BRIDGE=1", None)]
    #[case::other_value("ACCESSIBILITY_ENABLED=1", None)]
    #[case::other_name("PATH=/bin", None)]
    fn finds_hiding_assignments(#[case] line: &str, #[case] expected: Option<&str>) {
        assert_eq!(hiding_in(line).next().map(|rule| rule.to_string()).as_deref(), expected);
    }

    /// Setup's login files override the variables that they set, apart from an assignment in
    /// a file of `environment.d` that comes after setup's file.
    #[rstest]
    #[case::sway_env("NO_AT_BRIDGE", Source::File("/etc/sway/env".into()), true)]
    #[case::etc_environment("ACCESSIBILITY_ENABLED", Source::File("/etc/environment".into()), false)]
    #[case::this_session("NO_AT_BRIDGE", Source::ThisSession, true)]
    #[case::earlier_conf("NO_AT_BRIDGE", Source::File("/etc/environment.d/50-a11y.conf".into()), true)]
    #[case::later_conf("NO_AT_BRIDGE", Source::File("/etc/environment.d/90-a11y.conf".into()), false)]
    #[case::another_variable("GTK_A11Y", Source::File("/etc/sway/env".into()), false)]
    fn setup_overrides_the_variables_that_it_sets(
        #[case] name: &str,
        #[case] source: Source,
        #[case] expected: bool,
    ) {
        let rule = rules().find(|rule| rule.variable.name == name).unwrap();
        let hiding = Hiding { rule, source };
        assert_eq!(hiding.is_overridden(&[], SetupFiles::WithEnvironmentFile), expected);
    }

    /// Without the file in `environment.d`, nothing is overridden.
    #[rstest]
    #[case::sway_env("/etc/sway/env", false)]
    #[case::earlier_conf("/etc/environment.d/50-a11y.conf", false)]
    #[case::etc_environment("/etc/environment", false)]
    fn without_the_environment_file(#[case] path: &str, #[case] expected: bool) {
        let rule = rules().find(|rule| rule.variable.name == "NO_AT_BRIDGE").unwrap();
        let hiding = Hiding {
            rule,
            source: Source::File(path.into()),
        };
        assert_eq!(hiding.is_overridden(&[], SetupFiles::WithoutEnvironmentFile), expected);
    }

    /// The session's assignment isn't overridden when a file that setup doesn't override set it.
    #[rstest]
    #[case::a_later_file("/etc/environment.d/90-a11y.conf", false)]
    #[case::an_earlier_file("/etc/environment.d/50-a11y.conf", true)]
    #[case::etc_environment("/etc/environment", false)]
    fn the_session_follows_its_file(#[case] path: &str, #[case] expected: bool) {
        let rule = rules().find(|rule| rule.variable.name == "NO_AT_BRIDGE").unwrap();
        let found = [Hiding {
            rule,
            source: Source::File(path.into()),
        }];
        let session = Hiding {
            rule,
            source: Source::ThisSession,
        };
        assert_eq!(session.is_overridden(&found, SetupFiles::WithEnvironmentFile), expected);
    }
}
