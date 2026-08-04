use std::{
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use tracing::instrument;

use crate::shell::{RunError, common};

/// The `pwsh` executable itself, and the ability to invoke it.
#[derive(Debug)]
pub(super) struct PowershellExe {
    path: PathBuf,
}

impl PowershellExe {
    pub(super) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Wrap `command` so its output is bracketed by the same NUL markers as
    /// `common::frame` (so `common::unframe` reads it back), written as
    /// PowerShell's `` `0 `` escape.
    ///
    /// The command's output is captured to a string first (`Out-String`, with an
    /// effectively unbounded width so lines are not wrapped) and then written
    /// together with the markers in one `[Console]::Out.Write` — this keeps the
    /// markers and the output on one stream and in order (a bare `[Console]`
    /// write and PowerShell's own output stream could otherwise interleave). The
    /// last external command's status is captured from `$LASTEXITCODE` and
    /// re-raised so framing does not mask it.
    fn frame(command: &str) -> String {
        format!(
            "$atuin_out = {command} | Out-String -Width 2147483647\n\
             [Console]::Out.Write(\"`0atuin`0\" + $atuin_out + \"`0nituA`0\")\n\
             exit ($LASTEXITCODE ?? 0)\n"
        )
    }

    #[instrument(skip(command))]
    pub(super) async fn run(&self, command: &str) -> Result<process::Output, RunError> {
        let mut output = tokio::process::Command::new(&self.path)
            .args(["-NoLogo", "-Command", &Self::frame(command)])
            .output()
            .await
            .map_err(|error| RunError::Io {
                command: command.to_owned(),
                error: Arc::new(error),
            })?;

        common::unframe(&mut output, command)?;

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

#[cfg(test)]
mod frame_tests {
    use super::*;

    // The command is captured via `Out-String`, then written between the two NUL
    // markers in a single `[Console]::Out.Write`.
    #[test]
    fn captures_command_and_writes_between_markers() {
        let frame = PowershellExe::frame("Get-Alias | ConvertTo-Json");

        assert!(
            frame.contains("$atuin_out = Get-Alias | ConvertTo-Json | Out-String"),
            "command captured into $atuin_out"
        );
        assert!(
            frame.contains(r#"[Console]::Out.Write("`0atuin`0" + $atuin_out + "`0nituA`0")"#),
            "captured output written between the markers"
        );

        let begin = frame.find(r#""`0atuin`0""#).expect("begin marker");
        let end = frame.find(r#""`0nituA`0""#).expect("end marker");
        assert!(begin < end, "begin marker precedes end marker");
    }
}
