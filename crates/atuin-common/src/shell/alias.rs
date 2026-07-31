use bstr::BString;

use super::RunError;

#[derive(Debug, Clone, thiserror::Error)]
pub enum AliasesError {
    #[error("could not query the shell's aliases: {0}")]
    Run(#[from] RunError),
    #[error("could not parse the shell's alias list at byte {offset}, near {near:?}")]
    Parse { offset: usize, near: String },
    #[error("the alias probe did not run to completion")]
    Probe,
}

/// The body an alias expands to.
///
/// Shells disagree about what an alias body *is*. Most store a command string
/// the shell re-parses on use; xonsh stores an argv vector it execs directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasValue {
    /// A command string, re-parsed by the shell: bash, zsh, sh, fish.
    Command(BString),
    /// An argv vector, exec'd without a shell parsing pass: xonsh.
    Argv(Vec<BString>),
}

impl AliasValue {
    /// Render the alias body as a command string for a POSIX shell.
    ///
    /// `Command` already is one. `Argv` is single-quoted argument by argument so
    /// a POSIX re-parse reproduces the original argv: a plain space join would
    /// lose the boundaries of any argument containing a space and drop empty
    /// arguments entirely.
    pub fn shcmd(&self) -> BString {
        match self {
            AliasValue::Command(cmd) => cmd.clone(),
            AliasValue::Argv(argv) => {
                let mut out = BString::default();
                for (index, arg) in argv.iter().enumerate() {
                    if index > 0 {
                        out.push(b' ');
                    }
                    out.push(b'\'');
                    for &byte in arg.iter() {
                        if byte == b'\'' {
                            out.extend_from_slice(br"'\''");
                        } else {
                            out.push(byte);
                        }
                    }
                    out.push(b'\'');
                }
                out
            }
        }
    }
}

/// A shell alias in atuin's neutral model: a name bound to a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub name: BString,
    pub value: AliasValue,
}

#[cfg(test)]
mod alias_value_tests {
    use super::AliasValue;
    use bstr::BString;
    use rstest::rstest;

    #[rstest]
    #[case::command_shcmd_is_passthrough(AliasValue::Command(BString::from("ls -l")), "ls -l")]
    #[case::argv_shcmd_single_quotes_each_argument(
        AliasValue::Argv(vec![
            BString::from("git"),
            BString::from("commit"),
            BString::from("-m"),
            BString::from("hello world"),
        ]),
        r"'git' 'commit' '-m' 'hello world'"
    )]
    #[case::argv_shcmd_preserves_empty(
        AliasValue::Argv(vec![BString::from("echo"), BString::from(""), BString::from("x")]),
        r"'echo' '' 'x'"
    )]
    #[case::argv_shcmd_escapes_quote(
        AliasValue::Argv(vec![BString::from("echo"), BString::from("it's")]),
        r"'echo' 'it'\''s'"
    )]
    fn renders_shcmd(#[case] value: AliasValue, #[case] expected: &str) {
        assert_eq!(value.shcmd(), BString::from(expected));
    }
}
