use std::{borrow::Cow, collections::HashMap, process};

use winnow::{
    ModalResult, Parser,
    combinator::{alt, delimited, opt, preceded, repeat, terminated},
    token::{literal, take_while},
};

use bstr::{BStr, BString, ByteSlice};

use super::{
    Alias, AliasValue, AliasesError, Rendered, RunError, Skipped, Var, VarName, VarParsingError,
    VarValue,
    typed::{AliasCodec, VarCodec},
};

pub(super) type Aliases = HashMap<BString, AliasValue>;

/// The POSIX-family alias codec — shared by sh, bash, dash, ksh (and zsh's
/// renderer). A ZST; all state lives in the free functions it delegates to.
#[derive(Clone, Copy)]
pub struct PosixAliases;

impl AliasCodec for PosixAliases {
    fn render(&self, aliases: &[Alias]) -> Rendered {
        render_aliases(aliases)
    }

    fn parse(&self, listing: &[u8]) -> Result<Aliases, AliasesError> {
        parse_aliases(listing)
    }
}

/// The POSIX-family variable codec — shared by every POSIX shell. Carries only
/// the shell's name, so a rejected variable name can say which shell rejected it.
#[derive(Clone, Copy)]
pub struct PosixVars {
    pub(super) shell: &'static str,
}

impl VarCodec for PosixVars {
    fn validate_name(&self, name: impl AsRef<BStr>) -> Result<VarName, VarParsingError> {
        validate_var_name(name.as_ref().to_owned(), self.shell)
    }

    #[allow(unsafe_code)]
    fn validate_value(&self, value: impl AsRef<BStr>) -> Result<VarValue, VarParsingError> {
        // SAFETY: no value is rejected; any bytes are representable once quoted.
        Ok(unsafe { VarValue::new_unchecked(value.as_ref().to_owned()) })
    }

    fn quote<'a>(&self, value: &'a [u8]) -> Cow<'a, BStr> {
        quote_value(value)
    }

    fn render(&self, vars: &[Var]) -> BString {
        render_vars(vars)
    }
}

#[cfg(feature = "shell-syntax")]
use super::parse::{ShellParser, Token, classify_with};

/// Classifies POSIX-family shells (bash/sh/zsh/dash/ksh) via the bash grammar.
#[cfg(feature = "shell-syntax")]
pub struct PosixParser;

#[cfg(feature = "shell-syntax")]
impl ShellParser for PosixParser {
    fn classify(&self, code: &str) -> Vec<Token> {
        classify_with(tree_sitter_bash::LANGUAGE.into(), code)
    }
}

/// A POSIX-ish shell sources the user's rc files before running our command, and anything they
/// print lands on the same stdout. Bracket the real output with these NUL-delimited markers so it
/// can be sliced back out; NUL cannot occur in a command's arguments or output.
pub(super) const OUTPUT_BEGIN: &[u8] = b"\0atuin\0";
pub(super) const OUTPUT_END: &[u8] = b"\0nituA\0";

/// Wrap `command` so its output is delimited by [`OUTPUT_BEGIN`] and [`OUTPUT_END`]. `$?` is
/// captured and re-raised so that framing does not mask the command's own exit status.
pub(super) fn frame(command: &str) -> String {
    format!(
        r"printf '\000atuin\000'; {command}; __atuin_status=$?; printf '\000nituA\000'; exit $__atuin_status"
    )
}

/// Trim `output.stdout` down to the bytes between the framing markers.
pub(super) fn unframe(output: &mut process::Output, command: &str) -> Result<(), RunError> {
    let delimiter = || RunError::Delimiter {
        command: command.to_owned(),
    };

    let start = output
        .stdout
        .windows(OUTPUT_BEGIN.len())
        .position(|window| window == OUTPUT_BEGIN)
        .map(|at| at + OUTPUT_BEGIN.len())
        .ok_or_else(delimiter)?;
    let end = output.stdout[start..]
        .windows(OUTPUT_END.len())
        .position(|window| window == OUTPUT_END)
        .map(|at| at + start)
        .ok_or_else(delimiter)?;

    output.stdout = output.stdout[start..end].to_vec();

    Ok(())
}

/// Build an [`AliasesError::Parse`] describing where parsing gave up.
pub(super) fn parse_error(input: &[u8], offset: usize) -> AliasesError {
    let near = input[offset..].iter().take(48).copied().collect::<Vec<_>>();

    AliasesError::Parse {
        offset,
        near: String::from_utf8_lossy(&near).into_owned(),
    }
}

/// Parse the alias listing of a POSIX-ish shell into a name → value map.
///
/// Written to span the dialects: the `alias ` prefix is optional because dash omits it, and an
/// embedded `'` arrives either backslashed (`'\''`, bash and its posix mode) or double-quoted
/// (`'"'"'`, dash). Values are always single-quoted, and may contain literal newlines, so records
/// cannot be found by splitting on `\n`. All bytes are preserved verbatim; alias bodies are not
/// required to be UTF-8. A repeated name keeps its last definition.
pub(super) fn parse_aliases(input: &[u8]) -> Result<Aliases, AliasesError> {
    fn piece<'a>(input: &mut &'a [u8]) -> ModalResult<&'a [u8]> {
        alt((
            preceded(b'\\', literal(b"'".as_slice())),
            // dash's `single_quote()` wraps a maximal *run* of consecutive single
            // quotes in one double-quoted segment (`"''"`, `"'''"`), so accept
            // one-or-more quotes here, not just one. bash emits the single-quote
            // form, for which this matches exactly one quote — identical to before.
            delimited(b'"', take_while(1.., |b: u8| b == b'\''), b'"'),
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
            opt(literal(b"alias ".as_slice())),
            // bash's `alias -p` prints a `-- ` guard before any name that starts
            // with `-` (a legal alias name), so strip it. A real alias name never
            // contains a space, so `-- ` here can only be that guard; dash never
            // emits it, so the `opt` is a harmless no-op there.
            opt(literal(b"-- ".as_slice())),
            take_while(1.., |b: u8| b != b'=' && b != b'\n'),
            literal(b"=".as_slice()),
            alias_value,
        )
            .map(|(_, _, name, _, value): (_, _, &[u8], _, Vec<u8>)| (name.to_vec(), value))
            .parse_next(input)
    }

    let mut records = repeat(0.., terminated(alias_line, opt(literal(b"\n".as_slice())))).fold(
        HashMap::new,
        |mut acc: Aliases, (name, value)| {
            acc.insert(
                BString::from(name),
                AliasValue::Command(BString::from(value)),
            );
            acc
        },
    );

    records
        .parse(input)
        .map_err(|error| parse_error(input, error.offset()))
}

/// Render aliases as POSIX `alias name='value'` lines.
///
/// Shared by bash, sh and zsh, and reused for fish, which accepts this form. An
/// alias whose name is not a valid POSIX alias name is skipped rather than
/// emitted, since it would otherwise break the sourced file.
pub(super) fn render_aliases(aliases: &[Alias]) -> Rendered {
    let mut script = BString::default();
    let mut skipped = Vec::new();

    for alias in aliases {
        if !is_valid_alias_name(&alias.name) {
            skipped.push(Skipped {
                name: alias.name.clone(),
                reason: "not a valid alias name for a POSIX shell".to_owned(),
            });
            continue;
        }

        script.extend_from_slice(b"alias ");
        // A name starting with `-` would be read as an option by the `alias`
        // builtin (`alias: -x: invalid option`); guard it with `-- ` exactly as
        // bash's and zsh's own reusable `alias -p`/`alias -L` output does.
        if alias.name.first() == Some(&b'-') {
            script.extend_from_slice(b"-- ");
        }
        script.extend_from_slice(&alias.name);
        script.push(b'=');
        single_quote(&alias.value.shcmd(), &mut script);
        script.push(b'\n');
    }

    Rendered { script, skipped }
}

/// A POSIX alias name must be non-empty and free of the characters bash's
/// `valid_alias_name` rejects — shell metacharacters (`( ) < > ; & |`), the
/// expansion/quoting characters (`$ " ' \` and backtick) and `/` — plus `=`
/// (the name/value separator) and whitespace/control bytes. Globs (`* ?`) are
/// *not* rejected: bash's `valid_alias_name` accepts them.
///
/// A leading `-` is *allowed* (it is a legal alias name); [`render_aliases`]
/// guards it with the `-- ` prefix so the sourced line is not read as an option.
fn is_valid_alias_name(name: &[u8]) -> bool {
    const META: &[u8] = b"=\"'\\$()<>;&|/`";
    !name.is_empty()
        && !name
            .iter()
            .any(|&b| META.contains(&b) || b.is_ascii_whitespace() || b.is_ascii_control())
}

/// Append `bytes` to `out`, single-quoted, each embedded `'` written `'\''`.
fn single_quote(bytes: &[u8], out: &mut BString) {
    out.push(b'\'');
    for &b in bytes {
        if b == b'\'' {
            out.extend_from_slice(br"'\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
}

/// Validate `name` as a POSIX-family variable name: non-empty, an ASCII letter
/// or `_` first, then ASCII alphanumerics or `_`.
#[allow(unsafe_code)]
pub(super) fn validate_var_name(
    name: BString,
    shell: &'static str,
) -> Result<VarName, VarParsingError> {
    let first_ok = matches!(name.first(), Some(&b) if b == b'_' || b.is_ascii_alphabetic());
    let rest_ok = name.iter().all(|&b| b == b'_' || b.is_ascii_alphanumeric());
    if first_ok && rest_ok {
        // SAFETY: validated as a POSIX variable name just above.
        Ok(unsafe { VarName::new_unchecked(name) })
    } else {
        Err(VarParsingError::InvalidName { shell, name })
    }
}

/// Render vars as POSIX assignments: `export NAME=value` for exported vars,
/// `NAME=value` for shell vars. Non-bareword values are double-quoted.
pub(super) fn render_vars(vars: &[Var]) -> BString {
    let mut script = BString::default();
    for var in vars {
        if var.export {
            script.extend_from_slice(b"export ");
        }
        script.extend_from_slice(&var.name);
        script.push(b'=');
        script.extend_from_slice(&quote_value(&var.value));
        script.push(b'\n');
    }
    script
}

/// The value as a POSIX literal: borrowed when bareword-safe (only alphanumerics
/// and `_ - / .`), else an owned double-quoted string with `\`, `"`, `$` and
/// backtick escaped so the shell does not expand or re-interpret it.
pub(super) fn quote_value(value: &[u8]) -> Cow<'_, BStr> {
    if value
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '.'))
    {
        return Cow::Borrowed(value.as_bstr());
    }
    let mut out = BString::default();
    out.push(b'"');
    for &b in value {
        match b {
            b'\\' => out.extend_from_slice(br"\\"),
            b'"' => out.extend_from_slice(br#"\""#),
            b'$' => out.extend_from_slice(br"\$"),
            b'`' => out.extend_from_slice(br"\`"),
            _ => out.push(b),
        }
    }
    out.push(b'"');
    Cow::Owned(out)
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    fn alias(name: &str, value: AliasValue) -> Alias {
        Alias {
            name: BString::from(name),
            value,
        }
    }

    #[rstest]
    #[case::plain_command(
        "ll",
        AliasValue::Command(BString::from("ls -l")),
        "alias ll='ls -l'\n"
    )]
    #[case::escapes_embedded_single_quote(
        "x",
        AliasValue::Command(BString::from("echo it's")),
        "alias x='echo it'\\''s'\n"
    )]
    #[case::empty_body("e", AliasValue::Command(BString::default()), "alias e=''\n")]
    #[case::argv_round_trips(
        "g",
        AliasValue::Argv(vec![BString::from("git"), BString::from("st")]),
        concat!(r"alias g=''\''git'\'' '\''st'\'''", "\n")
    )]
    // A leading `-` is a legal alias name; it must be emitted with the `-- ` guard
    // so the sourced line is not parsed as an option (`alias: -x: invalid option`).
    #[case::guards_leading_hyphen_name(
        "-x",
        AliasValue::Command(BString::from("y")),
        "alias -- -x='y'\n"
    )]
    fn renders_alias(#[case] name: &str, #[case] value: AliasValue, #[case] expected: &str) {
        let r = render_aliases(&[alias(name, value)]);
        assert_eq!(r.script, BString::from(expected));
        assert!(r.skipped.is_empty());
    }

    #[rstest]
    #[case::slash("a/b")]
    #[case::semicolon("g;rm")]
    #[case::pipe("a|b")]
    #[case::ampersand("a&b")]
    #[case::backtick("a`b`")]
    #[case::dollar("a$b")]
    #[case::double_quote("a\"b")]
    fn skips_names_with_shell_metacharacters(#[case] name: &str) {
        let r = render_aliases(&[alias(name, AliasValue::Command(BString::from("x")))]);
        assert!(r.script.is_empty(), "expected {name:?} to be skipped");
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].name, BString::from(name));
    }

    #[test]
    fn skips_invalid_names_but_keeps_the_rest() {
        let r = render_aliases(&[
            alias("ok", AliasValue::Command(BString::from("cmd"))),
            alias("has space", AliasValue::Command(BString::from("cmd"))),
            alias("has=eq", AliasValue::Command(BString::from("cmd"))),
        ]);
        assert_eq!(r.script, BString::from("alias ok='cmd'\n"));
        assert_eq!(r.skipped.len(), 2);
        assert_eq!(r.skipped[0].name, BString::from("has space"));
        assert_eq!(r.skipped[1].name, BString::from("has=eq"));
    }

    #[test]
    fn empty_input_renders_nothing() {
        let r = render_aliases(&[]);
        assert!(r.script.is_empty());
        assert!(r.skipped.is_empty());
    }
}

#[cfg(test)]
mod var_render_tests {
    use super::*;
    use crate::shell::{VarName, VarValue};
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    #[allow(unsafe_code)]
    fn var(name: &str, value: &str, export: bool) -> Var {
        // SAFETY: test fixtures use valid names/values.
        unsafe {
            Var {
                name: VarName::new_unchecked(name),
                value: VarValue::new_unchecked(value),
                export,
            }
        }
    }

    #[rstest]
    #[case::bareword_is_unquoted("FOO", "bar", true, "export FOO=bar\n")]
    #[case::all_safe_chars_is_unquoted("P", "a_b-c.d/e", true, "export P=a_b-c.d/e\n")]
    #[case::spaces_force_double_quotes("FOO", "bar baz", true, "export FOO=\"bar baz\"\n")]
    #[case::shell_var_has_no_export_prefix("FOO", "bar baz", false, "FOO=\"bar baz\"\n")]
    #[case::escapes_dollar_backtick_quote_and_backslash(
        "V",
        r#"a$b`c"d\e"#,
        true,
        concat!(r#"export V="a\$b\`c\"d\\e""#, "\n")
    )]
    fn renders_var(
        #[case] name: &str,
        #[case] value: &str,
        #[case] export: bool,
        #[case] expected: &str,
    ) {
        let script = render_vars(&[var(name, value, export)]);
        assert_eq!(script, BString::from(expected));
    }

    #[rstest]
    #[case::letter("FOO")]
    #[case::leading_underscore("_x")]
    #[case::digit_after_letter("A1")]
    fn accepts_valid_names(#[case] name: &str) {
        let valid = validate_var_name(BString::from(name), "sh").unwrap();
        assert_eq!(BString::from(valid), BString::from(name));
    }

    #[rstest]
    #[case::leading_digit("1BAD")]
    #[case::empty("")]
    #[case::hyphen("a-b")]
    fn rejects_invalid_names(#[case] name: &str) {
        let err = validate_var_name(BString::from(name), "sh").unwrap_err();
        assert_eq!(
            err,
            VarParsingError::InvalidName {
                shell: "sh",
                name: BString::from(name)
            }
        );
    }
}

#[cfg(all(test, feature = "shell-syntax"))]
mod posix_parse_tests {
    use super::PosixParser;
    use crate::shell::{ShellParser, TokenKind, commands};
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    // Exact extraction — mirrors the atuin-ai permission tests.
    #[rstest]
    #[case::simple("ls -la /tmp", &[("ls", "ls -la /tmp")])]
    #[case::chaining("git add . && git commit -m 'hi'", &[("git", "git add ."), ("git", "git commit -m 'hi'")])]
    #[case::pipeline("cat file.txt | grep foo | wc -l", &[("cat", "cat file.txt"), ("grep", "grep foo"), ("wc", "wc -l")])]
    #[case::assignment_stripped("FOO=bar ls -la /tmp", &[("ls", "ls -la /tmp")])]
    #[case::redirect_stripped("ls > out.txt", &[("ls", "ls")])]
    #[case::subshell("(cd /tmp && ls)", &[("cd", "cd /tmp"), ("ls", "ls")])]
    fn extracts(#[case] code: &str, #[case] want: &[(&str, &str)]) {
        let got: Vec<(&str, &str)> = commands(&PosixParser, code)
            .iter()
            .map(|c| (c.name, c.full))
            .collect();
        assert_eq!(got, want);
    }

    // Commands hidden in substitutions surface as their own entries.
    #[rstest]
    #[case::dollar_sub("echo $(git rev-parse HEAD)", &["echo", "git"])]
    #[case::backtick("echo `date`", &["echo", "date"])]
    #[case::nested("echo \"Result: $(git log | head -1)\"", &["echo", "git", "head"])]
    fn extracts_nested(#[case] code: &str, #[case] want_names: &[&str]) {
        let names: Vec<&str> = commands(&PosixParser, code)
            .iter()
            .map(|c| c.name)
            .collect();
        for n in want_names {
            assert!(names.contains(n), "expected {n:?} in {names:?}");
        }
    }

    #[test]
    fn classifies_flags_and_strings() {
        let kinds: Vec<TokenKind> = PosixParser
            .classify("git commit -m 'hi'")
            .iter()
            .map(|t| t.kind)
            .collect();
        assert!(kinds.contains(&TokenKind::Command));
        assert!(kinds.contains(&TokenKind::Flag)); // -m
        assert!(kinds.contains(&TokenKind::String)); // 'hi'
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    fn map(pairs: &[(&[u8], &[u8])]) -> Aliases {
        pairs
            .iter()
            .map(|(k, v)| (BString::from(*k), AliasValue::Command(BString::from(*v))))
            .collect()
    }

    #[rstest]
    #[case::bash_escaped_quote(
        br"alias whoops='echo it'\''s fine'",
        &[(b"whoops".as_slice(), b"echo it's fine".as_slice())]
    )]
    #[case::dash_double_quoted_quote(
        br#"whoops='echo it'"'"'s fine'"#,
        &[(b"whoops".as_slice(), b"echo it's fine".as_slice())]
    )]
    // dash groups a *run* of consecutive quotes into one double-quoted segment:
    // the empty-string idiom `echo ''` prints as `'echo '"''"`, two quotes as `"''"`.
    #[case::dash_grouped_two_quotes(
        br#"e='echo '"''""#,
        &[(b"e".as_slice(), b"echo ''".as_slice())]
    )]
    #[case::dash_grouped_bare_quote_run(
        br#"x=''"''""#,
        &[(b"x".as_slice(), b"''".as_slice())]
    )]
    // bash `alias -p` prints a `-- ` guard before a name starting with `-`.
    #[case::bash_reusable_dash_guard(
        b"alias -- -x='y'\n",
        &[(b"-x".as_slice(), b"y".as_slice())]
    )]
    // dash prints a leading-`-` name with no `alias ` prefix and no guard.
    #[case::dash_leading_hyphen_name(
        b"-x='y'\n",
        &[(b"-x".as_slice(), b"y".as_slice())]
    )]
    #[case::dash_missing_alias_prefix(b"ll='ls -l'\n", &[(b"ll".as_slice(), b"ls -l".as_slice())])]
    #[case::embedded_newline(
        b"alias multi='line one\nline two'\nalias after='x'\n",
        &[
            (b"multi".as_slice(), b"line one\nline two".as_slice()),
            (b"after".as_slice(), b"x".as_slice()),
        ]
    )]
    #[case::non_utf8(b"alias bin='\xff\xfe'", &[(b"bin".as_slice(), b"\xff\xfe".as_slice())])]
    #[case::empty_input(b"", &[])]
    #[case::empty_value(b"alias e=''", &[(b"e".as_slice(), b"".as_slice())])]
    #[case::last_duplicate_wins(
        b"alias a='first'\nalias a='second'\n",
        &[(b"a".as_slice(), b"second".as_slice())]
    )]
    fn parses_aliases(#[case] input: &[u8], #[case] expected: &[(&[u8], &[u8])]) {
        assert_eq!(parse_aliases(input).unwrap(), map(expected));
    }

    #[rstest]
    // `name_does_not_run_across_a_newline` regression: `take_while(|b| b != b'=')`
    // without excluding `\n` made this return Ok({"to use foo\nalias a": "b"}) -- a
    // WRONG map, not an error.
    #[case::name_does_not_run_across_a_newline(b"alias to use foo\nalias a='b'\n")]
    #[case::trailing_garbage_rather_than_truncating(b"alias ll='ls -l'\nnot an alias line\n")]
    fn rejects(#[case] input: &[u8]) {
        let result = parse_aliases(input);
        assert!(result.is_err(), "expected Err, got {result:?}");
    }
}
