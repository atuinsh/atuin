use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    process,
    sync::Arc,
};

use bstr::{BStr, BString};
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use tracing::instrument;

use super::{
    Alias, AliasValue, AliasesError, IsShell, Rendered, RunError, Var, VarName, VarParsingError,
    common::{self, Aliases},
};

mod exe;

use exe::ShExe;

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

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
    const CANONICAL_NAME: &str = "sh";

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
                common::parse_aliases(&output.stdout)
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

impl IsShell for Sh {
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
        common::render_aliases(aliases)
    }

    fn render_vars(&self, vars: &[Var]) -> BString {
        common::render_vars(vars)
    }

    fn quote_value<'a>(&self, value: &'a [u8]) -> Cow<'a, BStr> {
        common::quote_value(value)
    }

    fn validate_var_name(&self, name: BString) -> Result<VarName, VarParsingError> {
        common::validate_var_name(name, Self::CANONICAL_NAME)
    }
}
