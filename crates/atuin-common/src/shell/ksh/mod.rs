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

mod alias;
mod exe;

use exe::KshExe;

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

#[derive(Debug)]
struct Inner {
    config_path: PathBuf,
    aliases: Probe<Aliases, AliasesError>,
}

#[derive(Debug, Clone, derive_more::Display)]
#[display("ksh")]
pub struct Ksh {
    exe: Arc<KshExe>,
    inner: Arc<Inner>,
}

impl Ksh {
    const CANONICAL_NAME: &str = "ksh";

    /// Create a new Ksh shell object.
    ///
    /// This will kick off background tokio tasks to eagerly probe information. Resolving the
    /// asynchronous methods will block on said probe tasks.
    pub fn new(path: &Path) -> Self {
        let config_path = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".kshrc"))
            .unwrap_or_else(|| PathBuf::from(".kshrc"));

        let exe = Arc::new(KshExe::new(path));

        // Probe lazily: the shared future is not polled until `aliases()` is
        // first awaited, so merely constructing a shell (e.g. to render config)
        // spawns no subprocess and needs no runtime.
        let aliases = {
            let exe = exe.clone();
            async move {
                let output = exe.run("alias -p").await?;
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

impl IsShell for Ksh {
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
