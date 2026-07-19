use std::{collections::HashMap, ffi::{OsStr, OsString}, io, path::Path, process::{self, Command, ExitCode, Output}};
use tokio;

#[derive(Debug, Clone, thiserror::Error)]
pub enum RunError {
    #[error("'{0}' encountered an IO error: {1}")]
    Io(String, io::Error),
    #[error("'{0}' failed with error code {1.status}")]
    Exec(Vec<u8>, Vec<u8>, Output),
    #[error("could not parse the output of '{0}'")]
    Parse(String),
}

pub trait IsShell {
    /// Get the name of this shell that we use internal to atuin.
    fn canonical_name(&self) -> &'static str;

    /// Return whether the shell is POSIX-compliant.
    fn is_posix(&self) -> bool;

    /// Query the aliases defined in the current shell.
    async fn aliases(&self) -> Result<HashMap<Vec<u8>, Vec<u8>>, RunError>;

    /// Invoke the given shell command, interactively, in this shell.
    ///
    /// Returns `Ok` if `exit_code == 0`, otherwise `Err`.
    async fn run_interactive(&self, s: impl AsRef<str>) -> Result<process::Output, RunError>;

    /// Get the full path to this shell, if it is installed.
    fn installed_path(&self) -> Option<&Path>;

    /// Return the path to the user configuration path of this shell.
    fn user_config_path(&self) -> &Path;
}

#[derive(Debug, Clone, Copy, derive_more::Display("bash"))]
pub struct Bash;

impl IsShell for Bash {
    fn canonical_name(&self) -> &'static str { "bash" }

    fn is_posix(&self) -> bool { true }

    #[instrument]
    async fn aliases(&self) -> Result<HashMap<Vec<u8>, Vec<u8>>, RunError> {
        self.run_interactive("alias -p")
            .await
            .map(|o| {
                // Unfortunately, naively parsing aliases won't work. Bash can potentially return
                // `\n` characters as part of the output of `alias`, so we need a complex
                // combinatorial parser.
            })
    }

    #[instrument]
    async fn run_interactive(&self, s: impl AsRef<str>) -> Result<process::Output, RunError> {
        tokio::process::Command::new(self.path())
            .args(["-ic", s.as_ref()])
            .output()
            .await?
            .map_or_else(RunError::into, |o| {
                if o.status == ExitCode::SUCCESS {
                    Ok(o)
                } else {
                    Err(o.into())
                }
            })
    }

    fn installed_path(&self) -> Option<&Path> {
    }

    fn user_config_path(&self) -> &Path {
    }
}
