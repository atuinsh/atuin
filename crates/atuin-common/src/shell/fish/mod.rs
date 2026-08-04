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

use exe::FishExe;

#[cfg(feature = "shell-syntax")]
use crate::shell::parse::{ShellParser, Token, classify_with};

/// Classifies fish via the fish grammar.
#[cfg(feature = "shell-syntax")]
pub struct FishParser;

#[cfg(feature = "shell-syntax")]
impl ShellParser for FishParser {
    fn classify(&self, code: &str) -> Vec<Token> {
        classify_with(tree_sitter_fish::language(), code)
    }
}

pub(super) type Aliases = HashMap<BString, AliasValue>;

/// Append `bytes` to `out`, single-quoted with fish's escaping (`\` → `\\`,
/// `'` → `\'`). Shared by fish alias and var rendering.
pub(super) fn fish_single_quote(bytes: &[u8], out: &mut BString) {
    out.push(b'\'');
    for &b in bytes {
        match b {
            b'\\' => out.extend_from_slice(br"\\"),
            b'\'' => out.extend_from_slice(br"\'"),
            _ => out.push(b),
        }
    }
    out.push(b'\'');
}

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

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

#[cfg(all(test, feature = "shell-syntax"))]
mod fish_parse_tests {
    use super::FishParser;
    use crate::shell::commands;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    #[rstest]
    #[case::simple("ls -la /tmp", &["ls"])]
    #[case::conditional("git add .; and git commit -m hi", &["git", "git"])]
    #[case::substitution("echo (date)", &["echo", "date"])]
    fn extracts_names(#[case] code: &str, #[case] want: &[&str]) {
        let names: Vec<&str> = commands(&FishParser, code).iter().map(|c| c.name).collect();
        assert_eq!(names, want);
    }

    #[rstest]
    #[case::redirect_after_name("ls > out.txt", &[("ls", "ls > out.txt")])]
    #[case::substitution_then_redirect("echo (date) > out.txt", &[("echo", "echo"), ("date", "date")])]
    fn ordering_survives_redirects(#[case] code: &str, #[case] want: &[(&str, &str)]) {
        let got: Vec<(&str, &str)> = commands(&FishParser, code)
            .iter()
            .map(|c| (c.name, c.full))
            .collect();
        assert_eq!(got, want);
    }
}
