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

use exe::PowershellExe;

pub(super) type Aliases = HashMap<BString, AliasValue>;

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

/// Lists the aliases defined in the loaded profile as JSON records with `Name`
/// and `Definition`. Read-only aliases (PowerShell's built-ins, like `ls` →
/// `Get-ChildItem`) are filtered out so only the user's own aliases are
/// imported. `-AsArray` forces an array even for zero or one alias.
const ALIAS_PROBE: &str = "Get-Alias | Where-Object { $_.Options -notmatch 'ReadOnly' } | Select-Object Name,Definition | ConvertTo-Json -AsArray";

/// Wrap `inner` so a syntax error in one rendered line does not abort the whole
/// sourced config. PowerShell aborts a script on a parse error (unlike POSIX
/// shells), so each line is run through `Invoke-Expression -ErrorAction
/// Continue`. `inner` is embedded as a single-quoted string, so its `'` are
/// doubled to `''`.
pub(super) fn secure_command(inner: &[u8]) -> BString {
    let mut out = BString::from(&b"Invoke-Expression -ErrorAction Continue -Command '"[..]);
    for &b in inner {
        if b == b'\'' {
            out.extend_from_slice(b"''");
        } else {
            out.push(b);
        }
    }
    out.extend_from_slice(b"'\n");
    out
}

#[derive(Debug)]
struct Inner {
    config_path: PathBuf,
    aliases: Probe<Aliases, AliasesError>,
}

#[derive(Debug, Clone, derive_more::Display)]
#[display("powershell")]
pub struct Powershell {
    exe: Arc<PowershellExe>,
    inner: Arc<Inner>,
}

impl Powershell {
    const CANONICAL_NAME: &str = "powershell";

    /// Create a new PowerShell shell object.
    ///
    /// This will kick off background tokio tasks to eagerly probe information. Resolving the
    /// asynchronous methods will block on said probe tasks.
    pub fn new(path: &Path) -> Self {
        let config_path = directories::BaseDirs::new()
            .map(|dirs| {
                dirs.config_dir()
                    .join("powershell")
                    .join("Microsoft.PowerShell_profile.ps1")
            })
            .unwrap_or_else(|| PathBuf::from("Microsoft.PowerShell_profile.ps1"));

        let exe = Arc::new(PowershellExe::new(path));

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

impl IsShell for Powershell {
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // The inner command is embedded as a single-quoted PowerShell string, so its
    // own `'` are doubled; the whole thing runs under `-ErrorAction Continue`.
    #[test]
    fn secure_command_wraps_and_doubles_quotes() {
        assert_eq!(
            secure_command(b"echo 'foo'"),
            BString::from("Invoke-Expression -ErrorAction Continue -Command 'echo ''foo'''\n")
        );
    }
}
