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
};

mod alias;
mod exe;
mod var;

use exe::NuExe;

pub(super) type Aliases = HashMap<BString, AliasValue>;

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

/// Lists the aliases defined in the loaded config as JSON records with `name`
/// and `expansion` fields (among others). A default `nu -c` loads the user's
/// config, so their interactive aliases are in scope.
const ALIAS_PROBE: &str = "scope aliases | to json";

#[derive(Debug)]
struct Inner {
    config_path: PathBuf,
    aliases: Probe<Aliases, AliasesError>,
}

#[derive(Debug, Clone, derive_more::Display)]
#[display("nu")]
pub struct Nu {
    exe: Arc<NuExe>,
    inner: Arc<Inner>,
}

impl Nu {
    const CANONICAL_NAME: &str = "nu";

    /// Create a new Nu shell object.
    ///
    /// This will kick off background tokio tasks to eagerly probe information. Resolving the
    /// asynchronous methods will block on said probe tasks.
    pub fn new(path: &Path) -> Self {
        let config_path = directories::BaseDirs::new()
            .map(|dirs| dirs.config_dir().join("nushell").join("config.nu"))
            .unwrap_or_else(|| PathBuf::from("config.nu"));

        let exe = Arc::new(NuExe::new(path));

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

impl IsShell for Nu {
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

    fn quote_value<'a>(&self, value: &'a [u8]) -> Cow<'a, BStr> {
        var::quote_value(value)
    }

    fn validate_var_name(&self, name: BString) -> Result<VarName, VarParsingError> {
        var::validate_var_name(name, Self::CANONICAL_NAME)
    }

    fn render_vars(&self, vars: &[Var]) -> BString {
        var::render_vars(vars)
    }
}

/// The `nu` alias codec.
#[derive(Clone, Copy)]
pub struct NuAliases;

impl crate::shell::typed::AliasCodec for NuAliases {
    fn render(&self, aliases: &[Alias]) -> Rendered {
        alias::render_aliases(aliases)
    }

    fn parse(&self, listing: &[u8]) -> Result<crate::shell::typed::Aliases, AliasesError> {
        alias::parse_aliases(listing)
    }
}

/// The `nu` variable codec.
#[derive(Clone, Copy)]
pub struct NuVars;

impl crate::shell::typed::VarCodec for NuVars {
    fn validate_name(&self, name: impl AsRef<BStr>) -> Result<VarName, VarParsingError> {
        var::validate_var_name(name.as_ref().to_owned(), "nu")
    }

    #[allow(unsafe_code)]
    fn validate_value(&self, value: impl AsRef<BStr>) -> Result<super::VarValue, VarParsingError> {
        // SAFETY: any bytes are representable once quoted.
        Ok(unsafe { super::VarValue::new_unchecked(value.as_ref().to_owned()) })
    }

    fn quote<'a>(&self, value: &'a [u8]) -> std::borrow::Cow<'a, BStr> {
        var::quote_value(value)
    }

    fn render(&self, vars: &[Var]) -> BString {
        var::render_vars(vars)
    }
}

// New capability SPI: `nu::Nu` is the installed handle for the `Nu` marker.
impl crate::shell::typed::InstalledShell for Nu {
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
