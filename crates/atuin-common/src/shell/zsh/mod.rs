use std::{
    borrow::Cow,
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

use bstr::{BStr, BString};

use super::{
    Alias, AliasValue, AliasesError, IsShell, Rendered, RunError, Var, VarName, VarParsingError,
    common,
};

mod alias;
mod exe;

use exe::ZshExe;

pub(super) type Aliases = HashMap<BString, AliasValue>;

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

#[derive(Debug)]
struct Inner {
    config_path: PathBuf,
    aliases: Probe<Aliases, AliasesError>,
}

#[derive(Debug, Clone, derive_more::Display)]
#[display("zsh")]
pub struct Zsh {
    exe: Arc<ZshExe>,
    inner: Arc<Inner>,
}

impl Zsh {
    const CANONICAL_NAME: &str = "zsh";

    /// Create a new Zsh shell object.
    ///
    /// This will kick off background tokio tasks to eagerly probe information. Resolving the
    /// asynchronous methods will block on said probe tasks.
    pub fn new(path: &Path) -> Self {
        let config_path = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".zshrc"))
            .unwrap_or_else(|| PathBuf::from(".zshrc"));

        let exe = Arc::new(ZshExe::new(path));

        // Probe lazily: the shared future is not polled until `aliases()` is
        // first awaited, so merely constructing a shell (e.g. to render config)
        // spawns no subprocess and needs no runtime.
        let aliases = {
            let exe = exe.clone();
            async move {
                // `unsetopt rcquotes` normalises the listing: with `rcquotes` set,
                // `alias -L` renders an embedded single quote as `''` inside the
                // single-quoted value, which the parser does not decode.
                let output = exe.run("unsetopt rcquotes; alias -L").await?;
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

impl IsShell for Zsh {
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

/// zsh's alias codec: it renders as plain POSIX but parses its own `$'…'`
/// (ANSI-C) alias listing.
#[derive(Clone, Copy)]
pub struct ZshAliases;

impl crate::shell::typed::AliasCodec for ZshAliases {
    fn render(&self, aliases: &[Alias]) -> Rendered {
        common::render_aliases(aliases)
    }

    fn parse(&self, listing: &[u8]) -> Result<crate::shell::typed::Aliases, AliasesError> {
        alias::parse_aliases(listing)
    }
}

// New capability SPI: `zsh::Zsh` is the *installed handle* for the `Zsh` marker.
impl crate::shell::typed::InstalledShell for Zsh {
    fn abspath(&self) -> &std::path::Path {
        self.exe.path()
    }

    async fn aliases(&self) -> Result<crate::shell::typed::Aliases, AliasesError> {
        self.inner.aliases.clone().await
    }

    async fn run(&self, command: &str) -> Result<std::process::Output, RunError> {
        self.exe.run(command).await
    }
}
