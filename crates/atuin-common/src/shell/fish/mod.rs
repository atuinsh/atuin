use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use tracing::instrument;

use bstr::BString;

use super::{Alias, AliasValue, AliasesError, IsShell, Rendered, RunError, posix};

mod alias;

pub(super) type Aliases = HashMap<BString, AliasValue>;

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

/// The `fish` executable itself, and the ability to invoke it.
#[derive(Debug)]
struct FishExe {
    path: PathBuf,
}

impl FishExe {
    /// `fish -i` sources the user's config files before running our command, and anything they
    /// print lands on the same stdout. Bracket the real output with these NUL-delimited markers so
    /// it can be sliced back out; NUL cannot occur in a command's arguments or output.
    const OUTPUT_BEGIN: &[u8] = b"\0atuin\0";
    const OUTPUT_END: &[u8] = b"\0nituA\0";

    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn path(&self) -> &Path {
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
    async fn run(&self, command: &str) -> Result<process::Output, RunError> {
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

#[derive(Debug)]
struct Inner {
    config_path: PathBuf,
    aliases: Probe<Aliases, AliasesError>,
}

#[derive(Debug, Clone, derive_more::Display)]
#[display("fish")]
pub struct Fish {
    exe: Arc<FishExe>,
    inner: Arc<Inner>,
}

impl Fish {
    /// Create a new Fish shell object.
    ///
    /// This will kick off background tokio tasks to eagerly probe information. Resolving the
    /// asynchronous methods will block on said probe tasks.
    pub fn new(path: &Path) -> Self {
        let config_path = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".config/fish/config.fish"))
            .unwrap_or_else(|| PathBuf::from(".config/fish/config.fish"));

        let exe = Arc::new(FishExe::new(path));

        // Probe lazily: the shared future is not polled until `aliases()` is
        // first awaited, so merely constructing a shell (e.g. to render config)
        // spawns no subprocess and needs no runtime.
        let aliases = {
            let exe = exe.clone();
            async move {
                let output = exe.run("alias").await?;
                alias::parse_aliases(&output.stdout)
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
impl IsShell for Fish {
    fn canonical_name(&self) -> &'static str {
        "fish"
    }

    fn is_posix(&self) -> bool {
        false
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
}
