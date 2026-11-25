//! What setup writes into each kind of file that a session reads at login: a call of
//! `atuin desktop-capture login` in the syntax of the file, or the session variables themselves.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use atuin_common::quote::{push_desktop_exec_arg, push_desktop_string, push_nu_raw, sh};

/// The comment above each call that setup writes, in every syntax that has `#` comments.
pub const MARKER: &str = "# Added by atuin setup: starts desktop capture at login.";

/// The start of the first line of the environment file.
pub const ENVIRONMENT_HEADER: &str = "# Written by atuin setup.";

/// Whether `first_line` starts a file that belongs to setup alone, as the first line of every
/// such file that setup writes does.
pub fn is_first_line_of_setups_file(first_line: &[u8]) -> bool {
    [MARKER, ENVIRONMENT_HEADER].into_iter().any(|prefix| first_line.starts_with(prefix.as_bytes()))
}

/// The arguments of `atuin` in every call that setup writes, which also identify setup's calls.
pub const LOGIN_SUBCOMMAND: &str = "desktop-capture login";

/// The flag with which the autostart entry calls `login`. It says that `login` runs inside a
/// desktop session that exists now, so that a running daemon tries a helper at once.
pub const IN_DESKTOP_SESSION_FLAG: &str = "--in-desktop-session";

/// The file in `environment.d` that sets the session variables.
///
/// The format of `environment.d` allows only assignments, so the file holds the variables
/// themselves, which are [`variables`](super::variables).
pub const ENVIRONMENT_CONTENT: &str = "# Written by atuin setup. `atuin desktop-capture login` \
                                       keeps this file up to \
                                       date.\nACCESSIBILITY_ENABLED=1\nNO_AT_BRIDGE=0\n";

/// The keys with which a desktop turns an autostart entry off, at the values that turn it off.
const DISABLING_KEYS: [DesktopKey; 2] = [
    DesktopKey {
        key: "Hidden",
        value: "true",
    },
    DesktopKey {
        key: "X-GNOME-Autostart-enabled",
        value: "false",
    },
];

/// A key of a desktop entry, at one value.
struct DesktopKey {
    key: &'static str,
    value: &'static str,
}

impl DesktopKey {
    /// Whether the desktop entry `entry` sets the key to the value.
    fn is_set_in(&self, entry: &[u8]) -> bool {
        entry.split(|&byte| byte == b'\n').any(|line| {
            line.iter().position(|&byte| byte == b'=').is_some_and(|equals| {
                line[..equals].trim_ascii() == self.key.as_bytes()
                    && line[equals + 1..].trim_ascii() == self.value.as_bytes()
            })
        })
    }
}

/// How a login runs `atuin`.
pub struct Launch<'a> {
    /// The path of Atuin's executable.
    pub atuin: &'a str,
    /// The variables that tell Atuin where its configuration and data are.
    pub env: &'a BTreeMap<String, String>,
}

impl Launch<'_> {
    /// The line of `sh` that evaluates what `login` prints, when Atuin's executable exists.
    ///
    /// The `if` keeps a shell that runs with `set -e`, such as Debian's `Xsession`, going when
    /// the executable is gone.
    pub fn sh_line(&self) -> String {
        let atuin = sh(self.atuin);
        let mut line = format!("if [ -x {atuin} ]; then eval \"$(");
        self.push_sh_env(&mut line);
        let _ = write!(line, "{atuin} {LOGIN_SUBCOMMAND} --shell sh 2>/dev/null)\"; fi");
        line
    }

    /// The file in fish's `conf.d` that evaluates what `login` prints, in login shells only.
    pub fn fish_file(&self) -> String {
        let atuin = sh(self.atuin);
        let mut file = format!("{MARKER}\nif status is-login; and test -x {atuin}\n    ");
        self.push_sh_env(&mut file);
        let _ = write!(file, "{atuin} {LOGIN_SUBCOMMAND} --shell fish 2>/dev/null | source\nend\n");
        file
    }

    /// The line of Nushell that loads what `login` prints.
    ///
    /// The `try` keeps the login going when the executable is gone or doesn't print a record.
    pub fn nu_line(&self) -> String {
        let mut line = "try { ".to_owned();
        if !self.env.is_empty() {
            line.push_str("with-env {");
            for (index, (name, value)) in self.env.iter().enumerate() {
                if index > 0 {
                    line.push_str(", ");
                }
                let _ = write!(line, "{name}: ");
                push_nu_raw(&mut line, value);
            }
            line.push_str("} { ");
        }
        line.push('^');
        push_nu_raw(&mut line, self.atuin);
        let _ = write!(line, " {LOGIN_SUBCOMMAND} --shell nu e> /dev/null");
        if !self.env.is_empty() {
            line.push_str(" }");
        }
        line.push_str(" | from json | load-env }");
        line
    }

    /// The XDG autostart entry that runs `login` with [`IN_DESKTOP_SESSION_FLAG`] and ignores what
    /// it prints.
    ///
    /// The entry keeps the keys with which the desktop turned off `before`, the entry that it
    /// replaces.
    pub fn autostart_entry(&self, before: Option<&[u8]>) -> String {
        let mut exec = String::new();
        if !self.env.is_empty() {
            exec.push_str("env ");
            for (name, value) in self.env {
                push_desktop_exec_arg(&mut exec, &[name, "=", value]);
                exec.push(' ');
            }
        }
        push_desktop_exec_arg(&mut exec, &[self.atuin]);
        let _ = write!(exec, " {LOGIN_SUBCOMMAND} {IN_DESKTOP_SESSION_FLAG}");
        let mut entry = format!(
            "{MARKER}\n[Desktop Entry]\nType=Application\nName=Atuin desktop capture\nExec="
        );
        push_desktop_string(&mut entry, &exec);
        entry.push_str("\nTryExec=");
        push_desktop_string(&mut entry, self.atuin);
        entry.push_str("\nNoDisplay=true\nTerminal=false\n");
        for disabling in DISABLING_KEYS {
            if before.is_some_and(|before| disabling.is_set_in(before)) {
                let _ = writeln!(entry, "{}={}", disabling.key, disabling.value);
            }
        }
        entry
    }

    /// Pushes `NAME=value ` to `line` for each location variable, quoted for `sh` and fish.
    fn push_sh_env(&self, line: &mut String) {
        for (name, value) in self.env {
            let _ = write!(line, "{name}={} ", sh(value));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use rstest::{fixture, rstest};

    use super::*;

    #[fixture]
    fn no_env() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    fn located() -> BTreeMap<String, String> {
        BTreeMap::from([("ATUIN_CONFIG_DIR".to_owned(), "/home/o'neil/at uin".to_owned())])
    }

    #[rstest]
    fn the_sh_line(no_env: BTreeMap<String, String>) {
        let launch = Launch {
            atuin: "/usr/bin/atuin",
            env: &no_env,
        };
        assert_eq!(
            launch.sh_line(),
            "if [ -x /usr/bin/atuin ]; then eval \"$(/usr/bin/atuin desktop-capture login --shell \
             sh 2>/dev/null)\"; fi"
        );
    }

    #[rstest]
    fn the_nu_line(no_env: BTreeMap<String, String>) {
        let launch = Launch {
            atuin: "/usr/bin/atuin",
            env: &no_env,
        };
        assert_eq!(
            launch.nu_line(),
            "try { ^r#'/usr/bin/atuin'# desktop-capture login --shell nu e> /dev/null | from json \
             | load-env }"
        );
    }

    #[rstest]
    fn the_autostart_entry_calls_login_in_the_desktop_session(no_env: BTreeMap<String, String>) {
        let launch = Launch {
            atuin: "/usr/bin/atuin",
            env: &no_env,
        };

        let entry = launch.autostart_entry(None);

        assert!(
            entry.contains("\nExec=/usr/bin/atuin desktop-capture login --in-desktop-session\n"),
            "{entry}"
        );
    }

    #[rstest]
    fn the_autostart_entry_escapes_the_quoted_backslash() {
        let env = located();
        let launch = Launch {
            atuin: "/a\\b/atuin",
            env: &env,
        };
        let entry = launch.autostart_entry(None);
        assert!(
            entry.contains(
                "Exec=env \"ATUIN_CONFIG_DIR=/home/o'neil/at uin\" \"/a\\\\\\\\b/atuin\" \
                 desktop-capture login --in-desktop-session\n"
            ),
            "{entry}"
        );
        assert!(entry.contains("TryExec=/a\\\\b/atuin\n"), "{entry}");
    }

    /// The entry stays off when the desktop turned it off, in either way.
    #[rstest]
    #[case::enabled("[Desktop Entry]\nExec=atuin\n", "")]
    #[case::hidden("[Desktop Entry]\nHidden=true\n", "Hidden=true\n")]
    #[case::gnome(
        "[Desktop Entry]\nX-GNOME-Autostart-enabled = false\n",
        "X-GNOME-Autostart-enabled=false\n"
    )]
    #[case::gnome_enabled("[Desktop Entry]\nX-GNOME-Autostart-enabled=true\n", "")]
    #[case::both(
        "Hidden=true\nX-GNOME-Autostart-enabled=false\n",
        "Hidden=true\nX-GNOME-Autostart-enabled=false\n"
    )]
    fn the_autostart_entry_stays_off(
        no_env: BTreeMap<String, String>,
        #[case] before: &str,
        #[case] kept: &str,
    ) {
        let launch = Launch {
            atuin: "/usr/bin/atuin",
            env: &no_env,
        };

        let entry = launch.autostart_entry(Some(before.as_bytes()));

        assert_eq!(entry, format!("{}{kept}", launch.autostart_entry(None)));
    }

    /// The file holds its header and then each variable, in the order of the list.
    #[rstest]
    #[cfg_attr(not(target_os = "linux"), ignore = "only the Linux backend names session variables")]
    fn the_environment_file_sets_every_variable() {
        let (header, variables) = ENVIRONMENT_CONTENT.split_once('\n').unwrap();
        assert!(header.starts_with(ENVIRONMENT_HEADER), "{header}");
        let mut expected = String::new();
        for super::super::Variable { name, value } in super::super::variables() {
            let _ = writeln!(expected, "{name}={value}");
        }
        assert_eq!(variables, expected);
    }

    /// A stand-in for `atuin` that prints the variable that it was given, as `login` would print
    /// a session variable.
    fn fake_atuin(dir: &std::path::Path, output: &str) -> String {
        let path = dir.join("at'uin $x");
        std::fs::write(&path, format!("#!/bin/sh\nprintf '%s' \"{output}\"\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path.into_os_string().into_string().unwrap()
    }

    /// The shells read the lines as setup means them: they run `atuin` from a path that needs
    /// quotes, pass the location variables, and take in what `atuin` prints.
    #[cfg(unix)]
    #[rstest]
    fn sh_shells_run_the_line(#[values("sh", "bash", "zsh")] shell: &str) {
        let dir = tempfile::tempdir().unwrap();
        let atuin = fake_atuin(dir.path(), "export SEEN=\\\"$ATUIN_CONFIG_DIR\\\"");
        let env = located();
        let line = Launch {
            atuin: &atuin,
            env: &env,
        }
        .sh_line();

        let Ok(result) =
            Command::new(shell).arg("-c").arg(format!("{line}\nprintf %s \"$SEEN\"")).output()
        else {
            return;
        };

        assert_eq!(String::from_utf8(result.stdout).unwrap(), "/home/o'neil/at uin");
    }

    #[cfg(unix)]
    #[rstest]
    fn sh_skips_a_missing_executable_under_set_e(no_env: BTreeMap<String, String>) {
        let line = Launch {
            atuin: "/nonexistent/atuin",
            env: &no_env,
        }
        .sh_line();

        let result =
            Command::new("sh").arg("-c").arg(format!("set -e\n{line}\necho on")).output().unwrap();

        assert_eq!(String::from_utf8(result.stdout).unwrap(), "on\n");
    }

    #[cfg(unix)]
    #[rstest]
    fn fish_runs_the_file_in_a_login_shell() {
        let dir = tempfile::tempdir().unwrap();
        let atuin = fake_atuin(dir.path(), "set -gx SEEN \\\"$ATUIN_CONFIG_DIR\\\"");
        let env = located();
        let file = dir.path().join("atuin.fish");
        std::fs::write(
            &file,
            Launch {
                atuin: &atuin,
                env: &env,
            }
            .fish_file(),
        )
        .unwrap();
        let script = format!("source '{}'; printf %s \"$SEEN\"", file.display());

        let Ok(login) = Command::new("fish").args(["--no-config", "-l", "-c", &script]).output()
        else {
            return;
        };
        let other = Command::new("fish").args(["--no-config", "-c", &script]).output().unwrap();

        assert_eq!(String::from_utf8(login.stdout).unwrap(), "/home/o'neil/at uin");
        assert_eq!(String::from_utf8(other.stdout).unwrap(), "");
    }

    #[cfg(unix)]
    #[rstest]
    fn nu_loads_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let atuin = fake_atuin(dir.path(), "{\\\"SEEN\\\":\\\"$ATUIN_CONFIG_DIR\\\"}");
        let env = located();
        let line = Launch {
            atuin: &atuin,
            env: &env,
        }
        .nu_line();

        let Ok(result) =
            Command::new("nu").args(["-n", "-c", &format!("{line}; print -n $env.SEEN")]).output()
        else {
            return;
        };

        assert_eq!(String::from_utf8(result.stdout).unwrap(), "/home/o'neil/at uin");
    }

    #[cfg(unix)]
    #[rstest]
    fn nu_survives_a_missing_executable(no_env: BTreeMap<String, String>) {
        let line = Launch {
            atuin: "/nonexistent/atuin",
            env: &no_env,
        }
        .nu_line();

        let Ok(result) =
            Command::new("nu").args(["-n", "-c", &format!("{line}; print on")]).output()
        else {
            return;
        };

        assert!(result.status.success(), "{result:?}");
        assert_eq!(String::from_utf8(result.stdout).unwrap(), "on\n");
    }
}
