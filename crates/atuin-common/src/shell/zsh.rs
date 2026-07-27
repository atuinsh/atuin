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
    token::{any, literal, take_while},
};

use super::{AliasesError, CmdAliasValue, IsShell, RunError};

type Aliases = HashMap<Vec<u8>, CmdAliasValue>;

type Probe<T, E> = Shared<BoxFuture<'static, Result<T, E>>>;

/// The `zsh` executable itself, and the ability to invoke it.
#[derive(Debug)]
struct ZshExe {
    path: PathBuf,
}

impl ZshExe {
    /// `zsh -i` sources the user's rc files before running our command, and anything they print
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
#[display("zsh")]
pub struct Zsh {
    exe: Arc<ZshExe>,
    inner: Arc<Inner>,
}

impl Zsh {
    /// Create a new Zsh shell object.
    ///
    /// This will kick off background tokio tasks to eagerly probe information. Resolving the
    /// asynchronous methods will block on said probe tasks.
    pub fn new(path: &Path) -> Self {
        let config_path = directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".zshrc"))
            .unwrap_or_else(|| PathBuf::from(".zshrc"));

        let exe = Arc::new(ZshExe::new(path));

        let probe_exe = exe.clone();
        let probe = tokio::spawn(async move {
            let output = probe_exe.run("alias -L").await?;
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

    /// Parse the output of `alias -L` into a name → value map.
    fn parse_aliases(input: &[u8]) -> Result<Aliases, AliasesError> {
        /// Decode the character `zsh` names with `\C-x`, its rendering of a control byte.
        fn control(byte: u8) -> u8 {
            if byte == b'?' {
                0x7f
            } else {
                byte.to_ascii_uppercase() & 0x1f
            }
        }

        /// Decode one escape sequence, having already consumed its leading backslash.
        fn escape(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
            alt((
                preceded(literal(b"C-".as_slice()), any).map(|byte: u8| vec![control(byte)]),
                preceded(literal(b"M-".as_slice()), meta).map(|byte| vec![byte]),
                any.verify_map(|byte: u8| {
                    let decoded = match byte {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        b'a' => 0x07,
                        b'b' => 0x08,
                        b'e' => 0x1b,
                        b'f' => 0x0c,
                        b'v' => 0x0b,
                        _ => return None,
                    };
                    Some(vec![decoded])
                }),
                preceded(b'x', take_while(1..=2, |b: u8| b.is_ascii_hexdigit()))
                    .map(|digits: &[u8]| vec![radix(digits, 16)]),
                take_while(1..=3, |b: u8| (b'0'..=b'7').contains(&b))
                    .map(|digits: &[u8]| vec![radix(digits, 8)]),
                any.map(|byte: u8| vec![byte]),
            ))
            .parse_next(input)
        }

        /// Decode the target of a `\M-` prefix, which is itself either an escape or a raw byte.
        fn meta(input: &mut &[u8]) -> ModalResult<u8> {
            alt((
                preceded(b'\\', escape).map(|bytes: Vec<u8>| bytes.first().copied().unwrap_or(0)),
                any,
            ))
            .map(|byte: u8| byte | 0x80)
            .parse_next(input)
        }

        fn radix(digits: &[u8], base: u32) -> u8 {
            digits.iter().fold(0u8, |acc, digit| {
                let value = (*digit as char).to_digit(base).unwrap_or(0) as u8;
                acc.wrapping_mul(base as u8).wrapping_add(value)
            })
        }

        /// Parse a `$'...'` string, decoding the escapes `zsh` uses for bytes it cannot emit
        /// literally. A newline in a value is written `\n`, never as a raw byte.
        fn ansi_c(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
            delimited(
                literal(b"$'".as_slice()),
                repeat(
                    0..,
                    alt((
                        preceded(b'\\', escape),
                        take_while(1.., |b: u8| b != b'\\' && b != b'\'').map(<[u8]>::to_vec),
                    )),
                )
                .fold(Vec::new, |mut acc: Vec<u8>, seg: Vec<u8>| {
                    acc.extend_from_slice(&seg);
                    acc
                }),
                b'\'',
            )
            .parse_next(input)
        }

        /// Parse a quoted or escaped run. `zsh` splices the three forms together, so an embedded
        /// quote arrives as `'...'\''...'` exactly as it does in `bash`.
        fn quoted(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
            alt((
                ansi_c,
                delimited(b'\'', take_while(0.., |b: u8| b != b'\''), b'\'').map(<[u8]>::to_vec),
                preceded(b'\\', any).map(|byte: u8| vec![byte]),
            ))
            .parse_next(input)
        }

        /// Parse a piece of a value: a quoted run, or bytes `zsh` judged safe to leave bare.
        fn value_piece(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
            alt((
                quoted,
                take_while(1.., |b: u8| {
                    b != b'\'' && b != b'\\' && b != b'$' && b != b'\n'
                })
                .map(<[u8]>::to_vec),
                b'$'.value(vec![b'$']),
            ))
            .parse_next(input)
        }

        /// Parse a piece of a name. A name never contains `=`, since `zsh` splits an alias
        /// definition at its first one, but it may contain a newline and so be `$'...'` quoted.
        fn name_piece(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
            alt((
                quoted,
                take_while(1.., |b: u8| {
                    b != b'\'' && b != b'\\' && b != b'$' && b != b'=' && b != b'\n'
                })
                .map(<[u8]>::to_vec),
                b'$'.value(vec![b'$']),
            ))
            .parse_next(input)
        }

        fn alias_name(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
            repeat(1.., name_piece)
                .fold(Vec::new, |mut acc: Vec<u8>, seg: Vec<u8>| {
                    acc.extend_from_slice(&seg);
                    acc
                })
                .parse_next(input)
        }

        fn alias_value(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
            repeat(0.., value_piece)
                .fold(Vec::new, |mut acc: Vec<u8>, seg: Vec<u8>| {
                    acc.extend_from_slice(&seg);
                    acc
                })
                .parse_next(input)
        }

        fn alias_line(input: &mut &[u8]) -> ModalResult<(Vec<u8>, Vec<u8>)> {
            (
                literal(b"alias ".as_slice()),
                alias_name,
                literal(b"=".as_slice()),
                alias_value,
            )
                .map(|(_, name, _, value): (_, Vec<u8>, _, Vec<u8>)| (name, value))
                .parse_next(input)
        }

        let mut records = repeat(0.., terminated(alias_line, opt(literal(b"\n".as_slice())))).fold(
            HashMap::new,
            |mut acc: Aliases, (name, value)| {
                acc.insert(name, CmdAliasValue(value));
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

impl IsShell for Zsh {
    type AliasKey = Vec<u8>;
    type AliasValue = CmdAliasValue;

    fn canonical_name(&self) -> &'static str {
        "zsh"
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn parse(input: &[u8]) -> HashMap<Vec<u8>, Vec<u8>> {
        Zsh::parse_aliases(input)
            .unwrap()
            .into_iter()
            .map(|(k, v)| (k, v.0))
            .collect()
    }

    #[test]
    fn parses_bare_value() {
        assert_eq!(
            parse(b"alias plain=man\n")[b"plain".as_slice()],
            b"man".to_vec()
        );
    }

    #[test]
    fn parses_single_quoted_with_escaped_quote() {
        assert_eq!(
            parse(br"alias whoops='echo it'\''s fine'")[b"whoops".as_slice()],
            b"echo it's fine".to_vec()
        );
    }

    #[test]
    fn decodes_ansi_c_newline() {
        assert_eq!(
            parse(b"alias multi=$'line one\\nline two'\n")[b"multi".as_slice()],
            b"line one\nline two".to_vec()
        );
    }

    #[test]
    fn decodes_ansi_c_octal_and_hex_and_backslash() {
        assert_eq!(parse(b"alias a=$'\\101'\n")[b"a".as_slice()], b"A".to_vec());
        assert_eq!(parse(b"alias b=$'\\x41'\n")[b"b".as_slice()], b"A".to_vec());
        assert_eq!(parse(b"alias c=$'\\\\'\n")[b"c".as_slice()], b"\\".to_vec());
    }

    #[test]
    fn name_does_not_run_across_a_newline() {
        assert!(Zsh::parse_aliases(b"alias to use foo\nalias a=b\n").is_err());
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(Zsh::parse_aliases(b"alias ll='ls -l'\nnonsense\n").is_err());
    }
}
