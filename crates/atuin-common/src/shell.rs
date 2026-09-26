#[cfg(unix)]
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::ExitStatus;

use sysinfo::{Process, System, get_current_pid};
use thiserror::Error;

#[derive(PartialEq, Eq, derive_more::Debug, derive_more::Display)]
pub enum Shell {
    #[display("sh")]
    Sh,
    #[display("bash")]
    Bash,
    #[display("fish")]
    Fish,
    #[display("zsh")]
    Zsh,
    #[display("xonsh")]
    Xonsh,
    #[display("nu")]
    Nu,
    #[display("powershell")]
    Powershell,
    #[display("unknown")]
    Unknown,
}

/// An error encountered while trying to query the user's default shell.
#[derive(Debug, Error)]
pub enum ShellLookupError {
    #[error("missing environment variable ${0}")]
    MissingEnv(&'static str),
    #[error("failed to run {command:?}: {error}")]
    RunCommand {
        command: String,
        error: std::io::Error,
    },
    #[error("command exited with status {status}: {command:?}")]
    CommandFailed {
        command: String,
        status: ExitStatus,
    },
    #[error("failed to parse output from command: {command:?}")]
    ParseOutput {
        command: String,
    },
}

impl Shell {
    #[must_use]
    pub fn current() -> Self {
        let sys = System::new_all();

        let process = sys
            .process(get_current_pid().expect("Failed to get current PID"))
            .expect("Process with current pid does not exist");

        let parent = sys
            .process(process.parent().expect("Atuin running with no parent!"))
            .expect("Process with parent pid does not exist");

        let shell = parent.name().to_string_lossy().trim().to_lowercase();
        let shell = shell.strip_prefix('-').unwrap_or(&shell);

        Self::from_string(shell)
    }

    #[must_use]
    pub fn from_env() -> Self {
        std::env::var("ATUIN_SHELL")
            .map_or(Self::Unknown, |shell| Self::from_string(&shell.trim().to_lowercase()))
    }

    #[must_use]
    pub fn config_file(&self) -> Option<PathBuf> {
        let mut path = directories::BaseDirs::new()?.home_dir().to_owned();

        // TODO: handle all shells
        match self {
            Self::Bash => path.push(".bashrc"),
            Self::Zsh => path.push(".zshrc"),
            Self::Fish => path.push(".config/fish/config.fish"),

            _ => return None,
        };

        Some(path)
    }

    /// Best-effort attempt to determine the default shell.
    ///
    /// This implementation will be different across different platforms. Caller should ensure
    /// [`Shell::Unknown`] is handled correctly.
    #[cfg(any(feature = "os", not(unix)))]
    pub fn default_shell() -> Result<Self, ShellLookupError> {
        if cfg!(windows) {
            return Ok(Self::Powershell);
        }

        #[cfg(unix)]
        if cfg!(target_os = "macos") {
            let user = std::env::var_os("USER").ok_or(ShellLookupError::MissingEnv("USER"))?;
            let path = PathBuf::from("/Local/Default/Users").join(user);
            return shell_from_command(
                "dscl".as_ref(),
                &["localhost".as_ref(), "-read".as_ref(), path.as_ref(), "shell".as_ref()],
                parse_dscl,
            );
        } else {
            let uid = crate::os::unix::uid().to_string();
            return shell_from_command(
                "getent".as_ref(),
                &["passwd".as_ref(), uid.as_ref()],
                parse_getent,
            );
        }

        #[allow(unreachable_code, reason = "unused only on unix")]
        Ok(Self::Unknown)
    }

    #[must_use]
    pub fn from_string(name: &str) -> Self {
        match name {
            "bash" => Self::Bash,
            "fish" => Self::Fish,
            "zsh" => Self::Zsh,
            "xonsh" => Self::Xonsh,
            "nu" => Self::Nu,
            "sh" => Self::Sh,
            "powershell" => Self::Powershell,

            _ => Self::Unknown,
        }
    }

    /// Returns true if the shell is posix-like
    /// Note that while fish is not posix compliant, it behaves well enough for our current
    /// featureset that this does not matter.
    #[must_use]
    pub fn is_posixish(&self) -> bool {
        matches!(self, Self::Bash | Self::Fish | Self::Zsh)
    }
}

#[must_use]
pub fn shell_name(parent: Option<&Process>) -> String {
    let sys = System::new_all();

    let parent = if let Some(parent) = parent {
        parent
    } else {
        let process = sys
            .process(get_current_pid().expect("Failed to get current PID"))
            .expect("Process with current pid does not exist");

        sys.process(process.parent().expect("Atuin running with no parent!"))
            .expect("Process with parent pid does not exist")
    };

    let shell = parent.name().to_string_lossy().trim().to_lowercase();
    let shell = shell.strip_prefix('-').unwrap_or(&shell);

    shell.to_string()
}

/// Get the default shell by running a command.
///
/// `parse` is fed the full output of the command and should return just the portion that
/// represents the path to the shell.
#[cfg(unix)]
fn shell_from_command<F>(
    command: &OsStr,
    args: &[&OsStr],
    parse: F,
) -> Result<Shell, ShellLookupError>
where
    F: FnOnce(&[u8]) -> Option<&[u8]>,
{
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::process::Command;

    use itertools::Itertools;

    // The full command as a string, for error messages.
    let command_string =
        || std::iter::once(command).chain(args.iter().copied()).map(OsStr::display).join(" ");

    let output =
        Command::new(command).args(args).output().map_err(|e| ShellLookupError::RunCommand {
            command: command_string(),
            error: e,
        })?;

    if !output.status.success() {
        return Err(ShellLookupError::CommandFailed {
            command: command_string(),
            status: output.status,
        });
    }

    let path = parse(&output.stdout).ok_or_else(|| ShellLookupError::ParseOutput {
        command: command_string(),
    })?;
    let path = Path::new(OsStr::from_bytes(path));
    let shell = path.file_name().ok_or_else(|| ShellLookupError::ParseOutput {
        command: command_string(),
    })?;

    Ok(shell.to_str().map_or(Shell::Unknown, Shell::from_string))
}

/// Extract the shell from the output of `dscl localhost -read <user> shell`.
#[cfg(unix)]
fn parse_dscl(output: &[u8]) -> Option<&[u8]> {
    // Extract the second field.
    output
        .split(|b| *b == b'\n')
        .next()?
        .split(u8::is_ascii_whitespace)
        .filter(|s| !s.is_empty())
        .nth(1)
}

/// Extract the login shell from the output of `getent passwd <uid>`.
#[cfg(unix)]
fn parse_getent(output: &[u8]) -> Option<&[u8]> {
    // Extract the 7th colon-separated field.
    output
        .split(|b| *b == b'\n')
        .next()?
        .split(|b| *b == b':')
        .nth(6)
        .map(<[u8]>::trim_ascii)
        .filter(|s| !s.is_empty())
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    use proptest::prelude::*;
    use rstest::rstest;

    use super::{Shell, ShellLookupError, parse_dscl, parse_getent, shell_from_command};

    #[rstest]
    #[case::simple(b"you:x:1000:1000:You Nix:/home/you:/bin/zsh\n", Some(&b"/bin/zsh"[..]))]
    #[case::no_trailing_newline(b"you:x:1000:1000::/home/you:/usr/bin/fish", Some(&b"/usr/bin/fish"[..]))]
    #[case::crlf(b"you:x:1000:1000::/home/you:/bin/bash\r\n", Some(&b"/bin/bash"[..]))]
    #[case::first_line_only(b"a:x:1:1::/a:/bin/zsh\nb:x:2:2::/b:/bin/fish\n", Some(&b"/bin/zsh"[..]))]
    #[case::non_utf8(b"you:x:1000:1000::/home/you:/opt/\xff/nu\n", Some(&b"/opt/\xff/nu"[..]))]
    #[case::empty_shell(b"you:x:1000:1000::/home/you:\n", None)]
    #[case::blank_shell(b"you:x:1000:1000::/home/you:  \n", None)]
    #[case::too_few_fields(b"you:x:1000:1000::/home/you\n", None)]
    #[case::empty(b"", None)]
    fn getent_passwd(#[case] output: &[u8], #[case] expected: Option<&[u8]>) {
        assert_eq!(parse_getent(output), expected);
    }

    #[rstest]
    #[case::simple(b"UserShell: /bin/zsh\n", Some(&b"/bin/zsh"[..]))]
    #[case::extra_spaces(b"UserShell:   /bin/zsh  \n", Some(&b"/bin/zsh"[..]))]
    #[case::no_trailing_newline(b"UserShell: /opt/homebrew/bin/fish", Some(&b"/opt/homebrew/bin/fish"[..]))]
    #[case::non_utf8(b"UserShell: /opt/\xff/nu\n", Some(&b"/opt/\xff/nu"[..]))]
    #[case::missing_value(b"UserShell:\n", None)]
    #[case::empty(b"", None)]
    fn dscl_shell(#[case] output: &[u8], #[case] expected: Option<&[u8]>) {
        assert_eq!(parse_dscl(output), expected);
    }

    proptest! {
        #[test]
        fn getent_passwd_returns_seventh_field(
            fields in proptest::array::uniform6("[^:\n]*"),
            shell in "/[^:\n\\s]+",
            rest in "(\n[^\n]*)?",
        ) {
            let line = format!("{}:{shell}{rest}", fields.join(":"));
            prop_assert_eq!(parse_getent(line.as_bytes()), Some(shell.as_bytes()));
        }
    }

    /// Run `printf '%s' <output>` through [`shell_from_command`] with the getent parser.
    fn from_printf(output: &[u8]) -> Result<Shell, ShellLookupError> {
        let output = OsStr::from_bytes(output);
        shell_from_command("printf".as_ref(), &["%s".as_ref(), output], parse_getent)
    }

    #[rstest]
    #[case::zsh(b"u:x:1:1::/h:/bin/zsh\n", Shell::Zsh)]
    #[case::bash(b"u:x:1:1::/h:/usr/local/bin/bash\n", Shell::Bash)]
    #[case::bare_name(b"u:x:1:1::/h:fish\n", Shell::Fish)]
    #[case::unsupported(b"u:x:1:1::/h:/bin/tcsh\n", Shell::Unknown)]
    #[case::non_utf8_dir(b"u:x:1:1::/h:/opt/\xff/nu\n", Shell::Nu)]
    #[case::non_utf8_name(b"u:x:1:1::/h:/bin/\xffsh\n", Shell::Unknown)]
    fn command_output_to_shell(#[case] output: &[u8], #[case] expected: Shell) {
        assert_eq!(from_printf(output).unwrap(), expected);
    }

    #[rstest]
    #[case::no_shell_field(b"u:x:1:1::/h\n")]
    #[case::root_path(b"u:x:1:1::/h:/\n")]
    #[case::parent_dir(b"u:x:1:1::/h:/bin/..\n")]
    fn command_output_unparseable(#[case] output: &[u8]) {
        let err = from_printf(output).unwrap_err();
        assert!(matches!(err, ShellLookupError::ParseOutput { .. }), "{err:?}");
        assert!(err.to_string().contains("printf %s"), "{err}");
    }

    #[rstest]
    fn command_failed() {
        let err = shell_from_command("false".as_ref(), &[], parse_getent).unwrap_err();
        assert!(matches!(err, ShellLookupError::CommandFailed { .. }), "{err:?}");
    }

    #[rstest]
    fn command_missing() {
        let err = shell_from_command(
            "atuin-definitely-not-a-real-command".as_ref(),
            &["arg".as_ref()],
            parse_getent,
        )
        .unwrap_err();
        assert!(matches!(err, ShellLookupError::RunCommand { .. }), "{err:?}");
        assert!(err.to_string().contains("\"atuin-definitely-not-a-real-command arg\""), "{err}");
    }
}
