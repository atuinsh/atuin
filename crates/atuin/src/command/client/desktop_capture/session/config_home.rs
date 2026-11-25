//! Whether the user's logins set `XDG_CONFIG_HOME`, which decides where fish, Nushell, and the
//! desktop's autostart look for setup's files.

use std::path::{Path, PathBuf};

/// The files, relative to the home, in which a login sets variables for the user.
const USER_FILES: [&str; 6] =
    [".profile", ".bash_profile", ".bash_login", ".zshenv", ".zprofile", ".pam_environment"];

/// The files in which a login sets variables for every user.
const SYSTEM_FILES: [&str; 3] = ["/etc/environment", "/etc/security/pam_env.conf", "/etc/profile"];

/// The directories whose files a login reads to set variables, with the extension of those
/// files.
const SYSTEM_DIRS: [Dir; 2] = [
    Dir {
        path: "/etc/environment.d",
        extension: "conf",
    },
    Dir {
        path: "/etc/profile.d",
        extension: "sh",
    },
];

/// The directory of `environment.d` in the home.
const USER_ENVIRONMENT_DIR: Dir = Dir {
    path: ".config/environment.d",
    extension: "conf",
};

/// A directory of files that set variables.
struct Dir {
    path: &'static str,
    extension: &'static str,
}

impl Dir {
    /// The files of the directory, which is under `base` when its path is relative.
    fn files(&self, base: &Path) -> impl Iterator<Item = PathBuf> {
        let entries = std::fs::read_dir(base.join(self.path)).into_iter().flatten();
        let extension = self.extension;
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(move |path| path.extension().is_some_and(|found| found == extension))
    }
}

/// Whether a file that a login reads for the user whose home is `home` sets `XDG_CONFIG_HOME`.
pub fn set_at_login(home: &Path) -> bool {
    let mut dirs = SYSTEM_DIRS
        .iter()
        .flat_map(|dir| dir.files(Path::new("/")))
        .chain(USER_ENVIRONMENT_DIR.files(home));
    USER_FILES.iter().any(|name| sets_config_home(&home.join(name)))
        || SYSTEM_FILES.iter().any(|path| sets_config_home(Path::new(path)))
        || dirs.any(|path| sets_config_home(&path))
}

/// Whether the file at `path` assigns `XDG_CONFIG_HOME`, in the syntax of `sh`, of
/// `environment.d`, or of `pam_env`.
fn sets_config_home(path: &Path) -> bool {
    let Ok(text) = std::fs::read(path) else {
        return false;
    };
    text.split(|&byte| byte == b'\n').any(|line| {
        let line = line.trim_ascii_start();
        !line.starts_with(b"#")
            && [&b"XDG_CONFIG_HOME="[..], b"XDG_CONFIG_HOME DEFAULT", b"XDG_CONFIG_HOME OVERRIDE"]
                .iter()
                .any(|assignment| line.windows(assignment.len()).any(|bytes| bytes == *assignment))
    })
}

#[cfg(test)]
mod tests {
    use atuin_common::path::PathBufExt as _;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::sh("export XDG_CONFIG_HOME=\"$HOME/cfg\"\n", true)]
    #[case::environment_d("XDG_CONFIG_HOME=${HOME}/cfg\n", true)]
    #[case::pam_env("XDG_CONFIG_HOME DEFAULT=@{HOME}/cfg\n", true)]
    #[case::commented_out("# export XDG_CONFIG_HOME=\"$HOME/cfg\"\n", false)]
    #[case::only_read("echo \"${XDG_CONFIG_HOME:-$HOME/.config}\"\n", false)]
    fn reads_assignments(#[case] text: &str, #[case] expected: bool) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf().joined("profile");
        std::fs::write(&path, text).unwrap();

        assert_eq!(sets_config_home(&path), expected);
    }

    /// A file of `environment.d` in the home counts, and one with another extension doesn't.
    #[rstest]
    #[case::conf("10-xdg.conf", true)]
    #[case::other_extension("10-xdg.conf.bak", false)]
    fn reads_the_environment_d_of_the_home(#[case] name: &str, #[case] expected: bool) {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(USER_ENVIRONMENT_DIR.path);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.joined(name), "XDG_CONFIG_HOME=${HOME}/cfg\n").unwrap();

        let found = USER_ENVIRONMENT_DIR.files(home.path()).any(|path| sets_config_home(&path));

        assert_eq!(found, expected);
    }
}
