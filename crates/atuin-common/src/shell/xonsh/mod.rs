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

use exe::XonshExe;

pub(super) type Aliases = HashMap<BString, AliasValue>;

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

// str/list aliases map directly onto our command/argv model. A string alias
// xonsh stored as an `ExecAlias` (it had exec markers, or was not a Python
// expression) keeps its original source on `.src`, so capture that too. A
// `FuncAlias`/callable has no source string and cannot be represented, so it is
// omitted.
const ALIAS_PROBE: &str = "import json; print(json.dumps({k: (v if isinstance(v, (str, list)) else v.src) for k, v in aliases.items() if isinstance(v, (str, list)) or isinstance(getattr(v, 'src', None), str)}))";

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
    const CANONICAL_NAME: &str = "xonsh";

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

impl IsShell for Xonsh {
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

/// The `xonsh` alias codec.
#[derive(Clone, Copy)]
pub struct XonshAliases;

impl crate::shell::typed::AliasCodec for XonshAliases {
    fn render(&self, aliases: &[Alias]) -> Rendered {
        alias::render_aliases(aliases)
    }

    fn parse(&self, listing: &[u8]) -> Result<crate::shell::typed::Aliases, AliasesError> {
        alias::parse_aliases(listing)
    }
}

/// The `xonsh` variable codec.
#[derive(Clone, Copy)]
pub struct XonshVars;

impl crate::shell::typed::VarCodec for XonshVars {
    fn validate_name(&self, name: impl AsRef<BStr>) -> Result<VarName, VarParsingError> {
        var::validate_var_name(name.as_ref().to_owned(), "xonsh")
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

// New capability SPI: `xonsh::Xonsh` is the installed handle for the `Xonsh` marker.
impl crate::shell::typed::InstalledShell for Xonsh {
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
