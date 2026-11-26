//! Running a harness's own CLI for what should go through its own writers.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;

use itertools::Itertools as _;

use crate::executable::resolve_in_path;

/// Errors returned by running a harness's CLI.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("`{0}` is not on your PATH; install it first")]
    NotFound(&'static str),

    #[error("could not run `{program}`: {cause}")]
    Io {
        program: &'static str,
        cause: std::io::Error,
    },

    /// The command exited non-zero. `said` is the first line it printed to stderr, if any.
    #[error("`{command}` failed{}", .said.as_ref().map(|said| format!(": {said}")).unwrap_or_default())]
    Failed {
        command: String,
        said: Option<String>,
    },
}

/// Runs `program` with `args`, keeping what it prints off the terminal: a screen may be on it. The
/// program runs from the path that [`resolve_in_path`] finds, so that what runs is what the `PATH`
/// check found. For example, on Windows, `claude` runs as `claude.cmd`.
pub(super) async fn run(program: &'static str, args: &[impl AsRef<OsStr>]) -> Result<(), CliError> {
    let found = resolve_in_path(Path::new(program)).ok_or(CliError::NotFound(program))?;
    let output = tokio::process::Command::new(&*found)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|cause| CliError::Io { program, cause })?;
    if output.status.success() {
        return Ok(());
    }
    let args = args.iter().map(|arg| arg.as_ref().to_string_lossy());
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(CliError::Failed {
        command: std::iter::once(program.into()).chain(args).join(" "),
        said: stderr.lines().map(str::trim).find(|line| !line.is_empty()).map(str::to_owned),
    })
}

/// [`run`], for a removal that may have nothing to remove.
pub(super) async fn run_ignoring_failure(program: &'static str, args: &[impl AsRef<OsStr>]) {
    let _ = run(program, args).await;
}
