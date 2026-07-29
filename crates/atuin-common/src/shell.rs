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

mod posix;

pub mod bash;
pub mod fish;
pub mod sh;
pub mod xonsh;
pub mod zsh;

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

#[derive(Debug, Clone, thiserror::Error)]
pub enum AliasesError {
    #[error("could not query the shell's aliases: {0}")]
    Run(#[from] RunError),
    #[error("could not parse the shell's alias list at byte {offset}, near {near:?}")]
    Parse { offset: usize, near: String },
    #[error("the alias probe did not run to completion")]
    Probe,
}

/// The body an alias expands to.
///
/// Shells disagree about what an alias body *is*. Most store a command string
/// the shell re-parses on use; xonsh stores an argv vector it execs directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasValue {
    /// A command string, re-parsed by the shell: bash, zsh, sh, fish.
    Command(BString),
    /// An argv vector, exec'd without a shell parsing pass: xonsh.
    Argv(Vec<BString>),
}

impl AliasValue {
    /// Render the alias body as a command string for a POSIX shell.
    ///
    /// `Command` already is one. `Argv` is single-quoted argument by argument so
    /// a POSIX re-parse reproduces the original argv: a plain space join would
    /// lose the boundaries of any argument containing a space and drop empty
    /// arguments entirely.
    pub fn shcmd(&self) -> BString {
        match self {
            AliasValue::Command(cmd) => cmd.clone(),
            AliasValue::Argv(argv) => {
                let mut out = BString::default();
                for (index, arg) in argv.iter().enumerate() {
                    if index > 0 {
                        out.push(b' ');
                    }
                    out.push(b'\'');
                    for &byte in arg.iter() {
                        if byte == b'\'' {
                            out.extend_from_slice(br"'\''");
                        } else {
                            out.push(byte);
                        }
                    }
                    out.push(b'\'');
                }
                out
            }
        }
    }
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

    pub fn config_file(&self) -> Option<std::path::PathBuf> {
        let mut path = directories::BaseDirs::new()?.home_dir().to_owned();

        // TODO: handle all shells
        match self {
            Shell::Bash => path.push(".bashrc"),
            Shell::Zsh => path.push(".zshrc"),
            Shell::Fish => path.push(".config/fish/config.fish"),

            _ => return None,
        };

        Some(path)
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

    /// Returns true if the shell is posix-like
    /// Note that while fish is not posix compliant, it behaves well enough for our current
    /// featureset that this does not matter.
    pub fn is_posixish(&self) -> bool {
        matches!(self, Shell::Bash | Shell::Fish | Shell::Zsh)
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

    pub fn run_interactive<I, S>(&self, args: I) -> Result<String, ShellError>
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

#[cfg(test)]
mod alias_value_tests {
    use super::AliasValue;
    use bstr::BString;

    #[test]
    fn command_shcmd_is_passthrough() {
        let v = AliasValue::Command(BString::from("ls -l"));
        assert_eq!(v.shcmd(), BString::from("ls -l"));
    }

    #[test]
    fn argv_shcmd_single_quotes_each_argument() {
        let v = AliasValue::Argv(vec![
            BString::from("git"),
            BString::from("commit"),
            BString::from("-m"),
            BString::from("hello world"),
        ]);
        assert_eq!(
            v.shcmd(),
            BString::from(r"'git' 'commit' '-m' 'hello world'")
        );
    }

    #[test]
    fn argv_shcmd_preserves_empty_and_escapes_quote() {
        assert_eq!(
            AliasValue::Argv(vec![
                BString::from("echo"),
                BString::from(""),
                BString::from("x")
            ])
            .shcmd(),
            BString::from(r"'echo' '' 'x'"),
        );
        assert_eq!(
            AliasValue::Argv(vec![BString::from("echo"), BString::from("it's")]).shcmd(),
            BString::from(r"'echo' 'it'\''s'"),
        );
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

#[cfg(test)]
mod factory_tests {
    use super::Shell;

    #[tokio::test]
    async fn interface_maps_supported_shells_to_the_right_impl() {
        assert_eq!(Shell::Bash.interface().unwrap().canonical_name(), "bash");
        assert_eq!(Shell::Sh.interface().unwrap().canonical_name(), "sh");
        assert_eq!(Shell::Zsh.interface().unwrap().canonical_name(), "zsh");
        assert_eq!(Shell::Fish.interface().unwrap().canonical_name(), "fish");
        assert_eq!(Shell::Xonsh.interface().unwrap().canonical_name(), "xonsh");
    }

    #[test]
    fn interface_is_none_for_unsupported_shells() {
        assert!(Shell::Nu.interface().is_none());
        assert!(Shell::Powershell.interface().is_none());
        assert!(Shell::Unknown.interface().is_none());
    }
}
