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

#[cfg(feature = "shell-syntax")]
use crate::shell::parse::{Token, ShellParser, classify_with};

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
impl Shell for Fish {
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

    fn render_vars(&self, vars: &[Var]) -> Rendered {
        var::render_vars(vars)
    }
}

#[cfg(all(test, feature = "shell-syntax"))]
mod fish_parse_tests {
    use crate::shell::commands;
    use super::FishParser;
    use rstest::rstest;
    use pretty_assertions::assert_eq;

    #[rstest]
    #[case::simple("ls -la /tmp", &["ls"])]
    #[case::conditional("git add .; and git commit -m hi", &["git", "git"])]
    #[case::substitution("echo (date)", &["echo", "date"])]
    fn extracts_names(#[case] code: &str, #[case] want: &[&str]) {
        let names: Vec<&str> = commands(&FishParser, code).iter().map(|c| c.name).collect();
        assert_eq!(names, want);
    }

    // Carry-forward from the Task 2 review: `walk_tokens` synthesizes a
    // `Command` token for a fish `command` node's `name` field *before*
    // recursing into the node's other children (redirects, arguments). That
    // is only safe if `name` is always the leftmost child of `command` --
    // otherwise the synthetic token could land out of byte-start order,
    // which `commands()` assumes never happens.
    //
    // Verified against the fish grammar (tree-sitter-fish 3.6.0):
    // `command: seq(field('name', expr), repeat(choice(field('redirect', ..), field('argument', ..))))`
    // -- `name` is grammatically required to precede any redirect/argument,
    // so no in-grammar parse can produce a `command` node whose child starts
    // before `name`. A dumped parse tree for `ls > out.txt` confirms this:
    // `command` node's first child is the `word` "ls" (0..2), then
    // `file_redirect` (3..12) strictly after it. These tests assert
    // `commands()` does not panic and returns names/fulls in source order.
    #[rstest]
    #[case::redirect_after_name("ls > out.txt", &[("ls", "ls > out.txt")])]
    #[case::substitution_then_redirect("echo (date) > out.txt", &[("echo", "echo"), ("date", "date")])]
    fn ordering_survives_redirects(#[case] code: &str, #[case] want: &[(&str, &str)]) {
        let got: Vec<(&str, &str)> =
            commands(&FishParser, code).iter().map(|c| (c.name, c.full)).collect();
        assert_eq!(got, want);
    }
}
