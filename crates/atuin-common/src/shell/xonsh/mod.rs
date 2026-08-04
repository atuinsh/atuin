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
impl IsShell for Xonsh {
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

    fn quote_value<'a>(&self, value: &'a [u8]) -> Cow<'a, BStr> {
        var::quote_value(value)
    }

    fn validate_var_name(&self, name: BString) -> Result<VarName, VarParsingError> {
        var::validate_var_name(name, self.canonical_name())
    }

    fn render_vars(&self, vars: &[Var]) -> BString {
        var::render_vars(vars)
    }
}
