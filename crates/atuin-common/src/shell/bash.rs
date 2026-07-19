use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    io,
    path::Path,
    process::{self, Command, ExitCode, Output},
    sync::Arc,
};
use tokio::{self, task::JoinHandle};
use winnow::{
    ModalResult, Parser,
    combinator::{alt, delimited, opt, preceded, repeat, terminated},
    token::{literal, take_while},
};

#[derive(Debug, Clone, thiserror::Error)]
pub enum RunError {
    #[error("'{0}' encountered an IO error: {1}")]
    Io(String, io::Error),
    #[error("'{0}' failed with error code {1.status}")]
    Exec(Vec<u8>, Vec<u8>, Output),
    #[error("could not parse the output of '{0}'")]
    Parse(String),
}

pub trait IsShell {
    /// Get the name of this shell that we use internal to atuin.
    fn canonical_name(&self) -> &'static str;

    /// Return whether the shell is POSIX-compliant.
    fn is_posix(&self) -> bool;

    /// Query the aliases defined in the current shell.
    async fn aliases(&self) -> Result<HashMap<Vec<u8>, Vec<u8>>, RunError>;

    /// Invoke the given shell command, interactively, in this shell.
    ///
    /// Returns `Ok` if `exit_code == 0`, otherwise `Err`.
    async fn run_interactive(&self, s: impl AsRef<str>) -> Result<process::Output, RunError>;

    /// Get the full path to this shell, if it is installed.
    fn installed_path(&self) -> Option<&Path>;

    /// Return the path to the user configuration path of this shell.
    fn user_config_path(&self) -> &Path;
}

struct Inner {
    aliases: JoinHandle<Result<HashMap<Vec<u8>, Vec<u8>>, RunError>>,
}

#[derive(Debug, Clone, derive_more::Display("bash"))]
pub struct Bash {
    inner: Arc<Inner>,
}

impl Bash {
    /// Create a new Bash shell object.
    ///
    /// This will kick off background tokio tasks to eagerly probe information. Resolving the
    /// asynchronous methods will block on said probe tasks.
    pub fn new() -> Self {
        let inner = Inner {
            aliases: tokio::spawn(async || {
                self.run_interactive("alias -p")
                    .await
                    .map(|o| Self::parse_aliases(o.stdout))
            }),
        };

        Self {
            inner: Arc::new(inner),
        }
    }

    /// Parse the output of `alias -p` into a name → value map.
    fn parse_aliases(input: &[u8]) -> Result<HashMap<Vec<u8>, Vec<u8>>, RunError> {
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
                take_while(1.., |b: u8| b != b'='),
                literal(b"=".as_slice()),
                alias_value,
            )
                .map(|(_, name, _, value): (_, &[u8], _, Vec<u8>)| (name.to_vec(), value))
                .parse_next(input)
        }

        let mut records = repeat(0.., terminated(alias_line, opt(literal(b"\n".as_slice())))).fold(
            HashMap::new,
            |mut acc: HashMap<Vec<u8>, Vec<u8>>, (name, value)| {
                acc.insert(name, value);
                acc
            },
        );

        records
            .parse(input)
            .map_err(|_| RunError::Parse("alias -p".to_owned()))
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
    async fn aliases(&self) -> Result<HashMap<Vec<u8>, Vec<u8>>, RunError> {}

    #[instrument]
    async fn run_interactive(&self, s: impl AsRef<str>) -> Result<process::Output, RunError> {
        tokio::process::Command::new(self.path())
            .args(["-ic", s.as_ref()])
            .output()
            .await?
            .map_or_else(RunError::into, |o| {
                if o.status == ExitCode::SUCCESS {
                    Ok(o)
                } else {
                    Err(o.into())
                }
            })
    }

    fn installed_path(&self) -> Option<&Path> {}

    fn user_config_path(&self) -> &Path {}
}
