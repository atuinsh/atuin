use std::{
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use tracing::instrument;

use crate::shell::{RunError, common};

/// The `nu` executable itself, and the ability to invoke it.
#[derive(Debug)]
pub(super) struct NuExe {
    path: PathBuf,
}

impl NuExe {
    pub(super) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Wrap `command` so its output is bracketed by the same NUL markers as
    /// `common::frame` (so `common::unframe` reads it back), written with
    /// nushell's `\u{0}` escape.
    ///
    /// A non-interactive `nu -c` prints only the value of the *final* pipeline,
    /// so the command is piped into `print` to force its output into the framed
    /// region. The last external command's status is captured from
    /// `$env.LAST_EXIT_CODE` and re-raised so framing does not mask it.
    fn frame(command: &str) -> String {
        format!(
            "print -rn \"\\u{{0}}atuin\\u{{0}}\"\n\
             {command} | print -rn $in\n\
             print -rn \"\\u{{0}}nituA\\u{{0}}\"\n\
             exit ($env.LAST_EXIT_CODE? | default 0)\n"
        )
    }

    #[instrument(skip(command))]
    pub(super) async fn run(&self, command: &str) -> Result<process::Output, RunError> {
        let mut output = tokio::process::Command::new(&self.path)
            .args(["-c", &Self::frame(command)])
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

    // The command must sit between the two NUL markers and be piped into `print`
    // so `nu -c` emits its output into the framed region.
    #[test]
    fn embeds_command_between_markers() {
        let frame = NuExe::frame("scope aliases | to json");

        let begin = frame
            .find(r#"print -rn "\u{0}atuin\u{0}""#)
            .expect("begin marker");
        let command = frame
            .find("scope aliases | to json | print -rn $in")
            .expect("command piped into print");
        let end = frame
            .find(r#"print -rn "\u{0}nituA\u{0}""#)
            .expect("end marker");

        assert!(
            begin < command && command < end,
            "command must sit between the frame markers"
        );
    }
}
