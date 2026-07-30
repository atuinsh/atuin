use std::{
    collections::HashMap,
    ffi::OsStr,
    io,
    path::Path,
    process::{self, Command, ExitStatus},
    sync::Arc,
};

use bstr::BString;
use serde::Serialize;
use sysinfo::{Process, System, get_current_pid};
use thiserror::Error;

mod alias;
mod posix;
mod render;
mod var;

pub mod bash;
pub mod fish;
pub mod sh;
pub mod xonsh;
pub mod zsh;

pub use alias::{Alias, AliasValue, AliasesError};
pub use render::{Rendered, Skipped};
pub use var::Var;

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

#[async_trait::async_trait]
pub trait IsShell: Send + Sync {
    /// Get the name of this shell that we use internal to atuin.
    fn canonical_name(&self) -> &'static str;

    /// Return whether the shell is POSIX-compliant.
    fn is_posix(&self) -> bool;

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

    /// Render the given variables into this shell's config syntax.
    ///
    /// Best-effort: variables this shell cannot represent are reported in
    /// [`Rendered::skipped`] rather than failing the whole render.
    fn render_vars(&self, vars: &[Var]) -> Rendered;
}

/// Compile-time proof that `IsShell` is object-safe. If a method signature ever
/// reintroduces a generic, associated type, or `impl Trait` argument, this fails
/// to compile.
const _: fn(&dyn IsShell) = |_shell| {};

#[derive(PartialEq, derive_more::Display)]
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

#[derive(Debug, Error, Serialize)]
pub enum ShellError {
    #[error("shell not supported")]
    NotSupported,

    #[error("failed to execute shell command: {0}")]
    ExecError(String),
}

impl Shell {
    pub fn current() -> Shell {
        let sys = System::new_all();

        let process = sys
            .process(get_current_pid().expect("Failed to get current PID"))
            .expect("Process with current pid does not exist");

        let parent = sys
            .process(process.parent().expect("Atuin running with no parent!"))
            .expect("Process with parent pid does not exist");

        let shell = parent.name().trim().to_lowercase();
        let shell = shell.strip_prefix('-').unwrap_or(&shell);

        Shell::from_string(shell.to_string())
    }

    pub fn from_env() -> Shell {
        std::env::var("ATUIN_SHELL").map_or(Shell::Unknown, |shell| {
            Shell::from_string(shell.trim().to_lowercase())
        })
    }

    /// Best-effort attempt to determine the default shell
    /// This implementation will be different across different platforms
    /// Caller should ensure to handle Shell::Unknown correctly
    pub fn default_shell() -> Result<Shell, ShellError> {
        let sys = System::name().unwrap_or("".to_string()).to_lowercase();

        // TODO: Support Linux
        // I'm pretty sure we can use /etc/passwd there, though there will probably be some issues
        let path = if sys.contains("darwin") {
            // This works in my testing so far
            Shell::Sh.run_interactive([
                "dscl localhost -read \"/Local/Default/Users/$USER\" shell | awk '{print $2}'",
            ])?
        } else if cfg!(windows) {
            return Ok(Shell::Powershell);
        } else {
            Shell::Sh.run_interactive(["getent passwd $LOGNAME | cut -d: -f7"])?
        };

        let path = Path::new(path.trim());
        let shell = path.file_name();

        if shell.is_none() {
            return Err(ShellError::NotSupported);
        }

        Ok(Shell::from_string(
            shell.unwrap().to_string_lossy().to_string(),
        ))
    }

    pub fn from_string(name: String) -> Shell {
        match name.as_str() {
            "bash" => Shell::Bash,
            "fish" => Shell::Fish,
            "zsh" => Shell::Zsh,
            "xonsh" => Shell::Xonsh,
            "nu" => Shell::Nu,
            "sh" => Shell::Sh,
            "powershell" => Shell::Powershell,

            _ => Shell::Unknown,
        }
    }

    /// Construct the object-safe [`IsShell`] interface for this shell, if atuin
    /// has an implementation for it (`None` for nu, powershell and unknown).
    ///
    /// The executable is resolved from `$PATH` by its canonical name when a
    /// command is run.
    pub fn interface(&self) -> Option<Box<dyn IsShell>> {
        let shell: Box<dyn IsShell> = match self {
            Shell::Bash => Box::new(bash::Bash::new(Path::new("bash"))),
            Shell::Sh => Box::new(sh::Sh::new(Path::new("sh"))),
            Shell::Zsh => Box::new(zsh::Zsh::new(Path::new("zsh"))),
            Shell::Fish => Box::new(fish::Fish::new(Path::new("fish"))),
            Shell::Xonsh => Box::new(xonsh::Xonsh::new(Path::new("xonsh"))),
            Shell::Nu | Shell::Powershell | Shell::Unknown => return None,
        };
        Some(shell)
    }

    fn run_interactive<I, S>(&self, args: I) -> Result<String, ShellError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let shell = self.to_string();
        let output = if self == &Self::Powershell {
            Command::new(shell)
                .args(args)
                .output()
                .map_err(|e| ShellError::ExecError(e.to_string()))?
        } else {
            Command::new(shell)
                .arg("-ic")
                .args(args)
                .output()
                .map_err(|e| ShellError::ExecError(e.to_string()))?
        };

        Ok(String::from_utf8(output.stdout).unwrap())
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
