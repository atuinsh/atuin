//! Recognizes what Homebrew installed, from paths and launchd labels alone.
//!
//! Homebrew installs each version of a formula in a keg, `<prefix>/Cellar/<formula>/<version>`.
//! It links the keg's executables into `<prefix>/bin`, and the keg itself to
//! `<prefix>/opt/<formula>`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::path::{AbsPathBuf, PathBufExt as _};

/// The prefixes of the labels of the launchd jobs that `brew services` installs. The first is the
/// current one, and the second is the one that older versions of Homebrew used.
const SERVICE_LABEL_PREFIXES: [&str; 2] = ["sh.brew.", "homebrew.mxcl."];

/// The formula whose keg holds the executable that `executable` resolves to.
#[must_use]
pub fn keg_formula(executable: &Path) -> Option<String> {
    let resolved = AbsPathBuf::canonicalize(executable).ok()?;
    formula_in_keg(&resolved).map(str::to_owned)
}

/// The formula whose executable `executable` links to from `<prefix>/bin`. `brew unlink` with
/// that formula removes the link and keeps the keg.
#[must_use]
pub fn linked_formula(executable: &Path) -> Option<String> {
    let bin = executable.parent()?;
    if bin.file_name()? != "bin" {
        return None;
    }
    let cellar = AbsPathBuf::canonicalize(bin.parent()?).ok()?.into_path_buf().joined("Cellar");
    let resolved = AbsPathBuf::canonicalize(executable).ok()?;
    if !resolved.starts_with(&cellar) {
        return None;
    }
    formula_in_keg(&resolved).map(str::to_owned)
}

/// The formula of the keg that holds `path`, when `path` is
/// `<prefix>/Cellar/<formula>/<version>/bin/<name>`.
fn formula_in_keg(path: &Path) -> Option<&str> {
    let bin = path.parent()?;
    let version = bin.parent()?;
    let formula = version.parent()?;
    let cellar = formula.parent()?;
    if bin.file_name()? != "bin" || cellar.file_name()? != "Cellar" {
        return None;
    }
    formula.file_name()?.to_str()
}

/// The formula that the `brew services` launchd job `label` runs.
#[must_use]
pub fn service_formula(label: &str) -> Option<&str> {
    SERVICE_LABEL_PREFIXES.iter().find_map(|prefix| label.strip_prefix(prefix))
}

/// The launch agent that `brew services start` installed for `formula` in the home directory
/// `home`, if it did. launchd starts the agent at each login.
#[must_use]
pub fn launch_agent(home: &Path, formula: &str) -> Option<PathBuf> {
    let mut plist = home.join("Library/LaunchAgents/plist");
    let mut name = OsString::new();
    for prefix in SERVICE_LABEL_PREFIXES {
        name.clear();
        name.push(prefix);
        name.push(formula);
        name.push(".plist");
        plist.set_file_name(&name);
        if plist.is_file() {
            return Some(plist);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    /// A Homebrew prefix with Atuin 18.25.1 installed and linked, as `brew install atuin` leaves
    /// it, and a release of Atuin outside Homebrew.
    #[fixture]
    fn prefix() -> TempDir {
        let prefix = tempfile::tempdir().unwrap();
        let keg_bin = prefix.path().join("Cellar/atuin/18.25.1/bin");
        std::fs::create_dir_all(&keg_bin).unwrap();
        std::fs::write(keg_bin.join("atuin"), "").unwrap();
        for dir in ["bin", "opt", "elsewhere/bin"] {
            std::fs::create_dir_all(prefix.path().join(dir)).unwrap();
        }
        std::fs::write(prefix.path().join("elsewhere/bin/atuin"), "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink("../Cellar/atuin/18.25.1/bin/atuin", prefix.path().join("bin/atuin")).unwrap();
            symlink("../Cellar/atuin/18.25.1", prefix.path().join("opt/atuin")).unwrap();
            symlink("../../bin/atuin", prefix.path().join("elsewhere/bin/linked")).unwrap();
        }
        prefix
    }

    /// Only the link in `<prefix>/bin` is what `brew unlink` removes. Every path that leads to
    /// the keg is Homebrew's `atuin`.
    #[cfg(unix)]
    #[rstest]
    #[case::bin_link("bin/atuin", Some("atuin"), Some("atuin"))]
    #[case::opt_link("opt/atuin/bin/atuin", None, Some("atuin"))]
    #[case::keg("Cellar/atuin/18.25.1/bin/atuin", None, Some("atuin"))]
    #[case::link_elsewhere_to_the_bin_link("elsewhere/bin/linked", None, Some("atuin"))]
    #[case::another_atuin("elsewhere/bin/atuin", None, None)]
    #[case::missing("bin/missing", None, None)]
    fn finds_the_formula_of_an_executable(
        prefix: TempDir,
        #[case] executable: &str,
        #[case] linked: Option<&str>,
        #[case] keg: Option<&str>,
    ) {
        let executable = prefix.path().join(executable);
        assert_eq!(linked_formula(&executable).as_deref(), linked);
        assert_eq!(keg_formula(&executable).as_deref(), keg);
    }

    #[rstest]
    #[case::current("sh.brew.atuin", Some("atuin"))]
    #[case::legacy("homebrew.mxcl.atuin", Some("atuin"))]
    #[case::atuins_own_agent("sh.atuin.daemon", None)]
    #[case::other_job("org.nix-community.home.atuin-daemon", None)]
    fn finds_the_formula_of_a_service(#[case] label: &str, #[case] expected: Option<&str>) {
        assert_eq!(service_formula(label), expected);
    }

    #[rstest]
    #[case::current(Some("sh.brew.atuin.plist"), true)]
    #[case::legacy(Some("homebrew.mxcl.atuin.plist"), true)]
    #[case::another_formula(Some("sh.brew.tailscale.plist"), false)]
    #[case::none(None, false)]
    fn finds_the_launch_agent_of_a_service(#[case] plist: Option<&str>, #[case] found: bool) {
        let home = tempfile::tempdir().unwrap();
        let agents = home.path().join("Library/LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        if let Some(plist) = plist {
            std::fs::write(agents.join(plist), "").unwrap();
        }
        let expected = plist.filter(|_| found).map(|plist| agents.join(plist));
        assert_eq!(launch_agent(home.path(), "atuin"), expected);
    }
}
