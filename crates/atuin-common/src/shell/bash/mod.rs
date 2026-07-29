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
    AliasValue, AliasesError, IsShell, RunError,
    posix::{self, Aliases},
};

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

/// The `bash` executable itself, and the ability to invoke it.
#[derive(Debug)]
struct BashExe {
    path: PathBuf,
}

impl BashExe {
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
#[display("bash")]
pub struct Bash {
    exe: Arc<BashExe>,
    inner: Arc<Inner>,
}

impl Bash {
    /// Create a new Bash shell object.
    ///
    /// This will kick off background tokio tasks to eagerly probe information. Resolving the
    /// asynchronous methods will block on said probe tasks.
    pub fn new(path: &Path) -> Self {
        let config_path = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".bashrc"))
            .unwrap_or_else(|| PathBuf::from(".bashrc"));

        let exe = Arc::new(BashExe::new(path));

        // Probe lazily: the shared future is not polled until `aliases()` is
        // first awaited, so merely constructing a shell (e.g. to render config)
        // spawns no subprocess and needs no runtime.
        let aliases = {
            let exe = exe.clone();
            async move {
                let output = exe.run("alias -p").await?;
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
impl IsShell for Bash {
    fn canonical_name(&self) -> &'static str {
        "bash"
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
}
