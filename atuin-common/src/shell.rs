use sysinfo::{get_current_pid, Process, System};
use thiserror::Error;

pub enum Shell {
    Sh,
    Bash,
    Fish,
    Zsh,
    Xonsh,
    Nu,

    Unknown,
}

#[derive(Debug, Error)]
pub enum ShellError {
    #[error("shell not supported")]
    NotSupported,

    #[error("failed to execute shell command: {0}")]
    ExecError(String),
}

pub fn shell() -> Shell {
    let name = shell_name(None);

    match name.as_str() {
        "bash" => Shell::Bash,
        "fish" => Shell::Fish,
        "zsh" => Shell::Zsh,
        "xonsh" => Shell::Xonsh,
        "nu" => Shell::Nu,
        "sh" => Shell::Sh,

        _ => Shell::Unknown,
    }
}

impl Shell {
    /// Returns true if the shell is posix-like
    /// Note that while fish is not posix compliant, it behaves well enough for our current
    /// featureset that this does not matter.
    pub fn is_posixish(&self) -> bool {
        matches!(self, Shell::Bash | Shell::Fish | Shell::Zsh)
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
