use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use bstr::BString;
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use tracing::instrument;

use super::{
    Alias, AliasValue, AliasesError, IsShell, Rendered, RunError, Var,
    posix::{self, Aliases},
};

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

/// The `sh` executable itself, and the ability to invoke it.
#[derive(Debug)]
struct ShExe {
    path: PathBuf,
}

impl ShExe {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    #[instrument(skip(command))]
    async fn run(&self, command: &str) -> Result<process::Output, RunError> {
        let mut output = tokio::process::Command::new(&self.path)
            .args(["-ic", &posix::frame(command)])
            .output()
            .await
            .map_err(|error| RunError::Io {
                command: command.to_owned(),
                error: Arc::new(error),
            })?;

        posix::unframe(&mut output, command)?;

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

#[derive(Debug)]
struct Inner {
    config_path: PathBuf,
    aliases: Probe<Aliases, AliasesError>,
}

#[derive(Debug, Clone, derive_more::Display)]
#[display("sh")]
pub struct Sh {
    exe: Arc<ShExe>,
    inner: Arc<Inner>,
}

impl Sh {
    /// Create a new Sh shell object.
    ///
    /// This will kick off background tokio tasks to eagerly probe information. Resolving the
    /// asynchronous methods will block on said probe tasks.
    ///
    /// `/bin/sh` is bash on macOS, dash on Debian and busybox ash elsewhere; their alias listings
    /// differ, so the shared POSIX parser is deliberately lenient about which dialect it is given.
    pub fn new(path: &Path) -> Self {
        let config_path = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".profile"))
            .unwrap_or_else(|| PathBuf::from(".profile"));

        let exe = Arc::new(ShExe::new(path));

        // Probe lazily: the shared future is not polled until `aliases()` is
        // first awaited, so merely constructing a shell (e.g. to render config)
        // spawns no subprocess and needs no runtime.
        let aliases = {
            let exe = exe.clone();
            async move {
                let output = exe.run("alias").await?;
                posix::parse_aliases(&output.stdout)
            }
            .boxed()
            .shared()
        };

        Self {
            exe,
            inner: Arc::new(Inner {
                config_path,
                aliases,
            }),
        }
    }
}

#[async_trait::async_trait]
impl IsShell for Sh {
    fn canonical_name(&self) -> &'static str {
        "sh"
    }

    fn is_posix(&self) -> bool {
        true
    }

    #[instrument]
    async fn aliases(&self) -> Result<HashMap<BString, AliasValue>, AliasesError> {
        self.inner.aliases.clone().await
    }

    #[instrument(skip(command))]
    async fn run_interactive(&self, command: &str) -> Result<process::Output, RunError> {
        self.exe.run(command).await
    }

    fn installed_path(&self) -> Option<&Path> {
        Some(self.exe.path())
    }

    fn user_config_path(&self) -> &Path {
        &self.inner.config_path
    }

    fn render_aliases(&self, aliases: &[Alias]) -> Rendered {
        posix::render_aliases(aliases)
    }

    fn render_vars(&self, vars: &[Var]) -> Rendered {
        posix::render_vars(vars)
    }
}
