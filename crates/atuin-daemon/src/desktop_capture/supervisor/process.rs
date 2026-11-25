//! Starts the helper's process.

use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use futures::future::{self, BoxFuture};
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, BufReader};
use tokio::process::ChildStderr;
use tracing::{debug, info};

use crate::desktop_capture::signing::{Signing, SigningError};

/// Starts the helper's process.
pub trait Spawn: Send + Sync {
    /// Returns the program to start for the helper at `program`.
    fn prepare(&self, program: PathBuf) -> BoxFuture<'_, Result<PathBuf, SigningError>> {
        Box::pin(future::ready(Ok(program)))
    }

    /// Starts `program` as the helper.
    fn spawn(&self, program: &Path) -> std::io::Result<HelperProcess>;
}

/// A running helper.
pub struct HelperProcess {
    pub stdin: Box<dyn AsyncWrite + Send + Unpin>,
    pub stdout: Box<dyn AsyncRead + Send + Unpin>,
    /// Resolves when the helper exits. Dropping the future kills the helper.
    pub exit: BoxFuture<'static, std::io::Result<ExitStatus>>,
}

/// Starts the helper as a child process, and logs what the helper writes to stderr.
pub struct HelperCommand {
    signing: Option<Signing>,
}

impl HelperCommand {
    /// Starts the helper through the signed copy that `signing` keeps, when `signing` is given.
    #[must_use]
    pub const fn new(signing: Option<Signing>) -> Self {
        Self { signing }
    }
}

impl Spawn for HelperCommand {
    fn prepare(&self, program: PathBuf) -> BoxFuture<'_, Result<PathBuf, SigningError>> {
        match &self.signing {
            Some(signing) => Box::pin(signing.helper(program)),
            None => Box::pin(future::ready(Ok(program))),
        }
    }

    fn spawn(&self, program: &Path) -> std::io::Result<HelperProcess> {
        let mut child = tokio::process::Command::new(program)
            .arg("capture")
            // The helper's log lines go into the daemon's log, which has colors of its own.
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let missing = || std::io::Error::other("the helper's pipes are missing");
        let stdin = child.stdin.take().ok_or_else(missing)?;
        let stdout = child.stdout.take().ok_or_else(missing)?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(forward_stderr(stderr));
        }
        Ok(HelperProcess {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            exit: Box::pin(async move { child.wait().await }),
        })
    }
}

/// Logs each line that the helper writes to stderr, through one buffer for every line.
async fn forward_stderr(stderr: ChildStderr) {
    let mut stderr = BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        match stderr.read_until(b'\n', &mut line).await {
            Ok(0) => return,
            Ok(_) => {
                let text = line.strip_suffix(b"\n").unwrap_or(&line);
                let text = String::from_utf8_lossy(text);
                info!(target: "desktop_capture::helper", "{text}");
            }
            Err(err) => {
                debug!(?err, "stopped reading the helper's stderr");
                return;
            }
        }
    }
}
