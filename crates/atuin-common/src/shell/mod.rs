use std::{
    borrow::Cow,
    collections::HashMap,
    convert::Infallible,
    ffi::OsStr,
    io,
    path::Path,
    process::{self, ExitStatus},
    str::FromStr,
    sync::Arc,
};

use crate::sysinfo::SystemExt;
use bstr::{BStr, BString};
use enum_dispatch::enum_dispatch;
use serde::Serialize;
use sysinfo::{Process, RefreshKind, System, get_current_pid};
use thiserror::Error;

mod alias;
mod common;
#[cfg(feature = "shell-syntax")]
mod parse;
mod render;
mod var;
#[cfg(feature = "shell-syntax")]
use parse::Fallback;
#[cfg(feature = "shell-syntax")]
pub use parse::{Command, ShellParser, Token, TokenKind, commands};

pub mod bash;
pub mod dash;
pub mod fish;
pub mod ksh;
pub mod nu;
pub mod powershell;
pub mod sh;
pub mod xonsh;
pub mod zsh;

pub use alias::{Alias, AliasValue, AliasesError};
pub use render::{Rendered, Skipped};
use tracing::instrument;
pub use var::{Var, VarName, VarParsingError, VarValue};

#[derive(Debug, Clone, thiserror::Error)]
pub enum RunError {
    #[error("'{command}' encountered an IO error: {error}")]
    Io {
        command: String,
        error: Arc<io::Error>,
    },
    #[error("'{command}' failed with {status}")]
    Exec {
        command: String,
        status: ExitStatus,
        stdout: BString,
        stderr: BString,
    },
    #[error("the output of '{command}' was not delimited as expected")]
    Delimiter { command: String },
}

#[enum_dispatch]
#[allow(
    async_fn_in_trait,
    reason = "only dispatched over [`Shell`] within our code; never needs to be Send"
)]
pub trait IsShell: Send + Sync {
    /// Query the aliases defined in the current shell.
    async fn aliases(&self) -> Result<HashMap<BString, AliasValue>, AliasesError>;

    /// Invoke the given shell command, interactively, in this shell.
    ///
    /// Returns `Ok` if `exit_code == 0`, otherwise `Err`.
    async fn run_interactive(&self, command: &str) -> Result<process::Output, RunError>;

    /// Get the full path to this shell, if it is installed.
    fn installed_path(&self) -> Option<&Path>;

    /// Return the path to the user configuration path of this shell.
    fn user_config_path(&self) -> &Path;

    /// Render the given aliases into this shell's config syntax.
    ///
    /// Best-effort: aliases this shell cannot represent are reported in
    /// [`Rendered::skipped`] rather than failing the whole render.
    fn render_aliases(&self, aliases: &[Alias]) -> Rendered;

    /// Render the given (already-validated) variables into this shell's config
    /// syntax. Total: names and values are validated when the [`Var`] is built,
    /// so nothing can be skipped here.
    fn render_vars(&self, vars: &[Var]) -> BString;

    /// Quote `value` as a literal in this shell's syntax: borrowed when it needs
    /// no quoting, owned (escaped) otherwise.
    fn quote_value<'a>(&self, value: &'a [u8]) -> Cow<'a, BStr>;

    /// Validate `name` as a variable name in this shell, producing a [`VarName`]
    /// or explaining why it was rejected. Each shell defines validity for
    /// itself; this is the only way to build a [`VarName`].
    fn validate_var_name(&self, name: BString) -> Result<VarName, VarParsingError>;

    /// Wrap `value` as a [`VarValue`] for this shell. Infallible today — any
    /// bytes can be quoted — but a shell may add constraints. The only safe way
    /// to build a [`VarValue`].
    #[allow(unsafe_code)]
    fn validate_var_value(&self, value: BString) -> Result<VarValue, Infallible> {
        // SAFETY: no value is rejected; any bytes are representable once quoted.
        Ok(unsafe { VarValue::new_unchecked(value) })
    }
}

/// Static-dispatch enum over every shell atuin can drive. `enum_dispatch`
/// generates the [`IsShell`] impl and the `From<Sh>`/… conversions, so this is
/// what [`ShellKind::interface`] hands back in place of a `Box<dyn IsShell>` —
/// which is why `IsShell` no longer needs to be object-safe.
#[enum_dispatch(IsShell)]
pub enum Shell {
    Sh(sh::Sh),
    Bash(bash::Bash),
    Fish(fish::Fish),
    Zsh(zsh::Zsh),
    Dash(dash::Dash),
    Ksh(ksh::Ksh),
    Xonsh(xonsh::Xonsh),
    Nu(nu::Nu),
    Powershell(powershell::Powershell),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum ShellKind {
    #[display("sh")]
    Sh,
    #[display("bash")]
    Bash,
    #[display("fish")]
    Fish,
    #[display("zsh")]
    Zsh,
    #[display("dash")]
    Dash,
    #[display("ksh")]
    Ksh,
    #[display("xonsh")]
    Xonsh,
    #[display("nu")]
    Nu,
    #[display("powershell")]
    Powershell,
    #[display("unknown")]
    Unknown,
}

/// The string did not name a shell atuin knows.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unrecognised shell: {0:?}")]
pub struct UnknownShell(pub String);

impl FromStr for ShellKind {
    type Err = UnknownShell;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Ok(match name {
            "bash" => Self::Bash,
            "fish" => Self::Fish,
            "zsh" => Self::Zsh,
            "dash" => Self::Dash,
            "ksh" => Self::Ksh,
            "xonsh" => Self::Xonsh,
            "nu" => Self::Nu,
            "sh" => Self::Sh,
            "powershell" => Self::Powershell,
            other => return Err(UnknownShell(other.to_owned())),
        })
    }
}

impl TryFrom<&str> for ShellKind {
    type Error = UnknownShell;

    fn try_from(name: &str) -> Result<Self, Self::Error> {
        name.parse()
    }
}

#[derive(Debug, Error, Serialize)]
pub enum ShellError {
    #[error("shell not supported")]
    NotSupported,

    #[error("failed to execute shell command: {0}")]
    ExecError(String),
}

impl ShellKind {
    pub fn current() -> ShellKind {
        let sys = System::new_all();

        let process = sys
            .process(get_current_pid().expect("Failed to get current PID"))
            .expect("Process with current pid does not exist");

        let parent = sys
            .process(process.parent().expect("Atuin running with no parent!"))
            .expect("Process with parent pid does not exist");

        let shell = parent.name().trim().to_lowercase();
        let shell = shell.strip_prefix('-').unwrap_or(&shell);

        shell.parse().unwrap_or(ShellKind::Unknown)
    }

    pub fn from_env() -> ShellKind {
        std::env::var("ATUIN_SHELL").map_or(ShellKind::Unknown, |shell| {
            shell
                .trim()
                .to_lowercase()
                .parse()
                .unwrap_or(ShellKind::Unknown)
        })
    }

    /// Best-effort attempt to determine the default shell
    /// This implementation will be different across different platforms
    /// Caller should ensure to handle ShellKind::Unknown correctly
    pub fn default_shell() -> Result<ShellKind, ShellError> {
        let sys = System::name().unwrap_or("".to_string()).to_lowercase();

        // TODO: Support Linux
        // I'm pretty sure we can use /etc/passwd there, though there will probably be some issues
        let path = if sys.contains("darwin") {
            // This works in my testing so far
            ShellKind::Sh.run_interactive([
                "dscl localhost -read \"/Local/Default/Users/$USER\" shell | awk '{print $2}'",
            ])?
        } else if cfg!(windows) {
            return Ok(ShellKind::Powershell);
        } else {
            ShellKind::Sh.run_interactive(["getent passwd $LOGNAME | cut -d: -f7"])?
        };

        let path = Path::new(path.trim());
        let shell = path.file_name();

        if shell.is_none() {
            return Err(ShellError::NotSupported);
        }

        Ok(shell
            .unwrap()
            .to_string_lossy()
            .parse()
            .unwrap_or(ShellKind::Unknown))
    }

    /// This shell's canonical name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sh => "sh",
            Self::Bash => "bash",
            Self::Fish => "fish",
            Self::Zsh => "zsh",
            Self::Dash => "dash",
            Self::Ksh => "ksh",
            Self::Xonsh => "xonsh",
            Self::Nu => "nu",
            Self::Powershell => "powershell",
            Self::Unknown => "unknown",
        }
    }

    /// Construct the [`Shell`] interface for this shell, if atuin has an
    /// implementation for it (`None` for powershell and unknown).
    ///
    /// The executable is resolved from `$PATH` by its canonical name when a
    /// command is run.
    pub fn interface(&self) -> Option<Shell> {
        let shell: Shell = match self {
            ShellKind::Bash => bash::Bash::new(Path::new("bash")).into(),
            ShellKind::Sh => sh::Sh::new(Path::new("sh")).into(),
            ShellKind::Zsh => zsh::Zsh::new(Path::new("zsh")).into(),
            ShellKind::Fish => fish::Fish::new(Path::new("fish")).into(),
            ShellKind::Xonsh => xonsh::Xonsh::new(Path::new("xonsh")).into(),
            ShellKind::Nu => nu::Nu::new(Path::new("nu")).into(),
            ShellKind::Dash => dash::Dash::new(Path::new("dash")).into(),
            ShellKind::Ksh => ksh::Ksh::new(Path::new("ksh")).into(),
            ShellKind::Powershell => powershell::Powershell::new(Path::new("pwsh")).into(),
            ShellKind::Unknown => return None,
        };
        Some(shell)
    }

    /// This dialect's syntax parser. Total — always returns one (the word-level
    /// fallback for shells without a grammar).
    #[cfg(feature = "shell-syntax")]
    pub fn parser(&self) -> &'static dyn ShellParser {
        match self {
            Self::Bash | Self::Sh | Self::Zsh | Self::Dash | Self::Ksh => &common::PosixParser,
            Self::Fish => &fish::FishParser,
            _ => &Fallback,
        }
    }

    /// Every command that will run in `code`, per this dialect. Convenience for
    /// `shell::commands(self.parser(), code)`.
    #[cfg(feature = "shell-syntax")]
    pub fn commands<'a>(&self, code: &'a str) -> Vec<Command<'a>> {
        commands(self.parser(), code)
    }

    fn run_interactive<I, S>(&self, args: I) -> Result<String, ShellError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let shell = self.to_string();
        let output = if self == &Self::Powershell {
            process::Command::new(shell)
                .args(args)
                .output()
                .map_err(|e| ShellError::ExecError(e.to_string()))?
        } else {
            process::Command::new(shell)
                .arg("-ic")
                .args(args)
                .output()
                .map_err(|e| ShellError::ExecError(e.to_string()))?
        };

        Ok(String::from_utf8(output.stdout).unwrap())
    }
}

#[cfg(test)]
mod interface_dispatch {
    use super::*;

    // `enum_dispatch` must route each `Shell` variant's methods to the wrapped
    // shell; shells with no implementation resolve to `None`.
    #[test]
    fn interface_dispatches_to_the_wrapped_shell() {
        let bash = ShellKind::Bash.interface().expect("bash has an interface");
        assert!(bash.user_config_path().ends_with(".bashrc"));

        let zsh = ShellKind::Zsh.interface().expect("zsh has an interface");
        assert!(zsh.user_config_path().ends_with(".zshrc"));

        assert!(ShellKind::Powershell.interface().is_some());
        assert!(ShellKind::Unknown.interface().is_none());
    }
}

#[cfg(all(test, feature = "shell-syntax"))]
mod parser_selection {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    // dash/ksh route to the posix grammar (not the word fallback): only the
    // grammar sees the command inside `$(...)`.
    #[rstest]
    #[case::bash(ShellKind::Bash)]
    #[case::dash(ShellKind::Dash)]
    #[case::ksh(ShellKind::Ksh)]
    fn posix_family_sees_into_substitution(#[case] kind: ShellKind) {
        let names: Vec<&str> = kind
            .commands("echo $(git rev-parse HEAD)")
            .iter()
            .map(|c| c.name)
            .collect();
        assert!(
            names.contains(&"git"),
            "{kind} did not descend into $(...): {names:?}"
        );
    }

    #[test]
    fn unknown_shell_uses_word_fallback() {
        let got: Vec<(&str, &str)> = ShellKind::Nu
            .commands("ls && cat foo")
            .iter()
            .map(|c| (c.name, c.full))
            .collect();
        assert_eq!(got, vec![("ls", "ls"), ("cat", "cat foo")]);
    }
}

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

    let shell = parent.name().trim().to_lowercase();
    let shell = shell.strip_prefix('-').unwrap_or(&shell);

    shell.to_string()
}
