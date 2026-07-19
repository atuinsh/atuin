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
use winnow::{
    ModalResult, Parser,
    combinator::{alt, delimited, opt, preceded, repeat, terminated},
    token::{literal, take_while},
};

use super::{AliasesError, IsShell, RunError};

type Aliases = HashMap<Vec<u8>, Vec<u8>>;

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

/// The `bash` executable itself, and the ability to invoke it.
#[derive(Debug)]
struct BashExe {
    path: PathBuf,
}

impl BashExe {
    /// `bash -i` sources the user's rc files before running our command, and anything they print
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
    /// [`Self::OUTPUT_END`]. `$?` is captured and re-raised so that framing does not mask the
    /// command's own exit status.
    fn frame(command: &str) -> String {
        format!(
            r"printf '\000atuin\000'; {command}; __atuin_status=$?; printf '\000nituA\000'; exit $__atuin_status"
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
                stdout: output.stdout,
                stderr: output.stderr,
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
#[display("bash")]
pub struct Bash {
    exe: Arc<BashExe>,
    inner: Arc<Inner>,
}

impl Bash {
    /// Create a new Bash shell object.
    ///
    /// This will kick off background tokio tasks to eagerly probe information. Resolving the
    /// asynchronous methods will block on said probe tasks.
    pub fn new(path: &Path) -> Self {
        let config_path = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".bashrc"))
            .unwrap_or_else(|| PathBuf::from(".bashrc"));

        let exe = Arc::new(BashExe::new(path));

        let probe_exe = exe.clone();
        let probe = tokio::spawn(async move {
            let output = probe_exe.run("alias -p").await?;
            Self::parse_aliases(&output.stdout)
        });

        let aliases = async move { probe.await.unwrap_or(Err(AliasesError::Probe)) }
            .boxed()
            .shared();

        Self {
            exe,
            inner: Arc::new(Inner {
                config_path,
                aliases,
            }),
        }
    }

    /// Parse the output of `alias -p` into a name → value map.
    fn parse_aliases(input: &[u8]) -> Result<Aliases, AliasesError> {
        fn piece<'a>(input: &mut &'a [u8]) -> ModalResult<&'a [u8]> {
            alt((
                preceded(b'\\', literal(b"'".as_slice())),
                delimited(b'\'', take_while(0.., |b: u8| b != b'\''), b'\''),
            ))
            .parse_next(input)
        }

        fn alias_value(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
            repeat(0.., piece)
                .fold(Vec::new, |mut acc: Vec<u8>, seg: &[u8]| {
                    acc.extend_from_slice(seg);
                    acc
                })
                .parse_next(input)
        }

        fn alias_line(input: &mut &[u8]) -> ModalResult<(Vec<u8>, Vec<u8>)> {
            (
                literal(b"alias ".as_slice()),
                take_while(1.., |b: u8| b != b'=' && b != b'\n'),
                literal(b"=".as_slice()),
                alias_value,
            )
                .map(|(_, name, _, value): (_, &[u8], _, Vec<u8>)| (name.to_vec(), value))
                .parse_next(input)
        }

        let mut records = repeat(0.., terminated(alias_line, opt(literal(b"\n".as_slice())))).fold(
            HashMap::new,
            |mut acc: Aliases, (name, value)| {
                acc.insert(name, value);
                acc
            },
        );

        records.parse(input).map_err(|error| {
            let offset = error.offset();
            let near = input[offset..].iter().take(48).copied().collect::<Vec<_>>();

            AliasesError::Parse {
                offset,
                near: String::from_utf8_lossy(&near).into_owned(),
            }
        })
    }
}

impl IsShell for Bash {
    fn canonical_name(&self) -> &'static str {
        "bash"
    }

    fn is_posix(&self) -> bool {
        true
    }

    #[instrument]
    async fn aliases(&self) -> Result<Aliases, AliasesError> {
        self.inner.aliases.clone().await
    }

    #[instrument(skip(s))]
    async fn run_interactive(&self, s: impl AsRef<str>) -> Result<process::Output, RunError> {
        self.exe.run(s.as_ref()).await
    }

    fn installed_path(&self) -> Option<&Path> {
        Some(self.exe.path())
    }

    fn user_config_path(&self) -> &Path {
        &self.inner.config_path
    }
}
