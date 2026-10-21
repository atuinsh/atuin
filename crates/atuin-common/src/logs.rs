//! Logging policy shared by the commands that choose it.

/// Target for messages meant for the user, as opposed to diagnostics.
///
/// No module path can start with a hyphenated name, so target prefix matching never mixes it up
/// with a module's events.
pub const USER: &str = "atuin-user";

/// Where a process sends its logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogMode {
    /// Warnings on stderr and no log file, for hot paths that may still talk to the user.
    Stderr,
    /// The log file, and warnings on stderr.
    FileAndStderr,
    /// The log file only, for processes that own the terminal.
    File,
    /// The daemon: the log file, and with `show_logs` every event on stderr too.
    Daemon {
        show_logs: bool,
    },
}

impl LogMode {
    /// Whether this mode writes the log file.
    #[must_use]
    pub const fn has_file(self) -> bool {
        match self {
            Self::File | Self::FileAndStderr | Self::Daemon { .. } => true,
            Self::Stderr => false,
        }
    }

    /// Whether this mode prints warnings and errors on stderr.
    #[must_use]
    pub const fn has_stderr(self) -> bool {
        match self {
            Self::Stderr | Self::FileAndStderr => true,
            Self::File | Self::Daemon { .. } => false,
        }
    }
}
