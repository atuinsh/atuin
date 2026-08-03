use std::{
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use tracing::instrument;

use crate::shell::{RunError, common};

/// The `sh` executable itself, and the ability to invoke it.
#[derive(Debug)]
pub(super) struct ShExe {
    path: PathBuf,
}

impl ShExe {
    pub(super) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    #[instrument(skip(command))]
    pub(super) async fn run(&self, command: &str) -> Result<process::Output, RunError> {
        let mut output = tokio::process::Command::new(&self.path)
            .args(["-ic", &common::frame(command)])
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
