use std::{
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use tracing::instrument;

use crate::shell::RunError;

/// The `xonsh` executable itself, and the ability to invoke it.
#[derive(Debug)]
pub(super) struct XonshExe {
    path: PathBuf,
}

impl XonshExe {
    /// `xonsh -i` sources the user's rc files before running our command, and anything they print
    /// lands on the same stdout. Bracket the real output with these NUL-delimited markers so it
    /// can be sliced back out; NUL cannot occur in a command's arguments or output.
    const OUTPUT_BEGIN: &[u8] = b"\0atuin\0";
    const OUTPUT_END: &[u8] = b"\0nituA\0";

    pub(super) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Wrap `command` so its output is delimited by [`Self::OUTPUT_BEGIN`] and
    /// [`Self::OUTPUT_END`]. The exit status of the last subprocess is captured and re-raised so
    /// that framing does not mask the command's own exit status.
    ///
    /// `print` is used rather than `sys.stdout.write` because `-i -c` echoes the value of any
    /// expression statement, colourised, into the framed region; `print` evaluates to `None` and
    /// is not echoed. The interactive shell also writes a terminal-title escape to file descriptor
    /// 1 before each subprocess, which would land inside the frame, so `settitle` is disabled.
    /// Both of xonsh's subprocess-error settings are turned off so that a failing command raises
    /// no exception and the closing marker is still emitted.
    fn frame(command: &str) -> String {
        format!(
            "if getattr(__xonsh__.shell, 'shell', None) is not None: __xonsh__.shell.shell.settitle = lambda: None\n\
             $XONSH_SUBPROC_RAISE_ERROR = False\n\
             $XONSH_SUBPROC_CMD_RAISE_ERROR = False\n\
             __xonsh__.lastcmd = None\n\
             print('\\000atuin\\000', end='', flush=True)\n\
             {command}\n\
             __atuin_status = getattr(__xonsh__.lastcmd, 'rtn', 0) or 0\n\
             print('\\000nituA\\000', end='', flush=True)\n\
             import sys as __atuin_sys; __atuin_sys.exit(__atuin_status)\n"
        )
    }

    #[instrument(skip(command))]
    pub(super) async fn run(&self, command: &str) -> Result<process::Output, RunError> {
        let mut output = tokio::process::Command::new(&self.path)
            .args(["-i", "-c", &Self::frame(command)])
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
