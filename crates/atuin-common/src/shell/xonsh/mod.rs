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

use super::{Alias, AliasValue, AliasesError, Rendered, RunError, Shell, Var};

mod alias;
mod var;

pub(super) type Aliases = HashMap<BString, AliasValue>;

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

const ALIAS_PROBE: &str = "import json; print(json.dumps({k: v for k, v in aliases.items() if isinstance(v, (str, list))}))";

/// The `xonsh` executable itself, and the ability to invoke it.
#[derive(Debug)]
struct XonshExe {
    path: PathBuf,
}

impl XonshExe {
    /// `xonsh -i` sources the user's rc files before running our command, and anything they print
    /// lands on the same stdout. Bracket the real output with these NUL-delimited markers so it
    /// can be sliced back out; NUL cannot occur in a command's arguments or output.
    const OUTPUT_BEGIN: &[u8] = b"\0atuin\0";
    const OUTPUT_END: &[u8] = b"\0nituA\0";

    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn path(&self) -> &Path {
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
    async fn run(&self, command: &str) -> Result<process::Output, RunError> {
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

#[derive(Debug)]
struct Inner {
    config_path: PathBuf,
    aliases: Probe<Aliases, AliasesError>,
}

#[derive(Debug, Clone, derive_more::Display)]
#[display("xonsh")]
pub struct Xonsh {
    exe: Arc<XonshExe>,
    inner: Arc<Inner>,
}

impl Xonsh {
    /// Create a new Xonsh shell object.
    ///
    /// This will kick off background tokio tasks to eagerly probe information. Resolving the
    /// asynchronous methods will block on said probe tasks.
    pub fn new(path: &Path) -> Self {
        let config_path = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".xonshrc"))
            .unwrap_or_else(|| PathBuf::from(".xonshrc"));

        let exe = Arc::new(XonshExe::new(path));

        // Probe lazily: the shared future is not polled until `aliases()` is
        // first awaited, so merely constructing a shell (e.g. to render config)
        // spawns no subprocess and needs no runtime.
        let aliases = {
            let exe = exe.clone();
            async move {
                let output = exe.run(ALIAS_PROBE).await?;
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
impl Shell for Xonsh {
    fn canonical_name(&self) -> &'static str {
        "xonsh"
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
        alias::render_aliases(aliases)
    }

    fn render_vars(&self, vars: &[Var]) -> Rendered {
        var::render_vars(vars)
    }
}
