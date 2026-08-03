use std::{
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use tracing::instrument;

use crate::shell::RunError;

/// The `fish` executable itself, and the ability to invoke it.
#[derive(Debug)]
pub(super) struct FishExe {
    path: PathBuf,
}

impl FishExe {
    /// `fish -i` sources the user's config files before running our command, and anything they
    /// print lands on the same stdout. Bracket the real output with these NUL-delimited markers so
    /// it can be sliced back out; NUL cannot occur in a command's arguments or output.
    const OUTPUT_BEGIN: &[u8] = b"\0atuin\0";
    const OUTPUT_END: &[u8] = b"\0nituA\0";

    pub(super) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Wrap `command` so its output is delimited by [`Self::OUTPUT_BEGIN`] and
    /// [`Self::OUTPUT_END`]. `$status` is captured and re-raised so that framing does not mask the
    /// command's own exit status.
    fn frame(command: &str) -> String {
        format!(
            r"printf '\000atuin\000'; {command}; set __atuin_status $status; printf '\000nituA\000'; exit $__atuin_status"
        )
    }

    #[instrument(skip(command))]
    pub(super) async fn run(&self, command: &str) -> Result<process::Output, RunError> {
        let mut output = tokio::process::Command::new(&self.path)
            .args(["-ic", &Self::frame(command)])
            .output()
            .await
            .map_err(|error| RunError::Io {
                command: command.to_owned(),
                error: Arc::new(error),
            })?;

        let delimiter = || RunError::Delimiter {
            command: command.to_owned(),
        };
        let start = output
            .stdout
            .windows(Self::OUTPUT_BEGIN.len())
            .position(|window| window == Self::OUTPUT_BEGIN)
            .map(|at| at + Self::OUTPUT_BEGIN.len())
            .ok_or_else(delimiter)?;
        let end = output.stdout[start..]
            .windows(Self::OUTPUT_END.len())
            .position(|window| window == Self::OUTPUT_END)
            .map(|at| at + start)
            .ok_or_else(delimiter)?;

        output.stdout = output.stdout[start..end].to_vec();

        if output.status.success() {
            Ok(output)
        } else {
            Err(RunError::Exec {
                command: command.to_owned(),
                status: output.status,
                stdout: output.stdout.into(),
                stderr: output.stderr.into(),
            })
        }
    }
}
