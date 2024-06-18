use std::{ffi::OsStr, path::Path, process::Command};

use serde::Serialize;
use sysinfo::{get_current_pid, Process, System};
use thiserror::Error;

#[derive(PartialEq)]
pub enum Shell {
    Sh,
    Bash,
    Fish,
    Zsh,
    Xonsh,
    Nu,
    Powershell,

    Unknown,
}

impl std::fmt::Display for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shell = match self {
            Shell::Bash => "bash",
            Shell::Fish => "fish",
            Shell::Zsh => "zsh",
            Shell::Nu => "nu",
            Shell::Xonsh => "xonsh",
            Shell::Sh => "sh",
            Shell::Powershell => "powershell",

            Shell::Unknown => "unknown",
        };

        write!(f, "{}", shell)
    }
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

    pub fn config_file(&self) -> Option<std::path::PathBuf> {
        let mut path = if let Some(base) = directories::BaseDirs::new() {
            base.home_dir().to_owned()
        } else {
            return None;
        };

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
