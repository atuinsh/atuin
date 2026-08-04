use std::{
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use tracing::instrument;

use crate::shell::{RunError, common};

/// The `xonsh` executable itself, and the ability to invoke it.
#[derive(Debug)]
pub(super) struct XonshExe {
    path: PathBuf,
}

impl XonshExe {
    pub(super) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// xonsh's frame writes the same NUL markers as `common::frame` (so
    /// `common::unframe` reads it back), but the body is Python, not POSIX, and
    /// the exit status of the last subprocess is captured and re-raised so that
    /// framing does not mask the command's own exit status.
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
