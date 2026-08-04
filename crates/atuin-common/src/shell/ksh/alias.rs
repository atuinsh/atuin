use std::collections::HashMap;

use bstr::BString;
use winnow::{
    ModalResult, Parser,
    combinator::{alt, delimited, opt, preceded, repeat, terminated},
    token::{any, literal, take_while},
};

use crate::shell::{AliasValue, AliasesError};

use super::Aliases;

/// Parse the output of `alias -p` into a name → value map.
///
/// `ksh` prints one `alias name=value` line per alias. A plain value is single-quoted; a value
/// with a single quote or a control byte is rendered ANSI-C (`$'...'`), escaping `'` as `\'`,
/// `\` as `\\`, the usual `\n`/`\t`/`\r`/`\a`/`\b`/`\f`/`\v`, escape as `\E`, and any other byte
/// as `\xNN`. Bare (unquoted) values also occur, e.g. `alias fc=hist`.
pub(super) fn parse_aliases(input: &[u8]) -> Result<Aliases, AliasesError> {
    /// Decode one escape sequence, having already consumed its leading backslash.
    fn escape(input: &mut &[u8]) -> ModalResult<u8> {
        alt((
            preceded(b'x', take_while(1..=2, |b: u8| b.is_ascii_hexdigit()))
                .map(|digits: &[u8]| radix(digits, 16)),
            any.map(|byte: u8| match byte {
                b'n' => b'\n',
                b't' => b'\t',
                b'r' => b'\r',
                b'a' => 0x07,
                b'b' => 0x08,
                b'f' => 0x0c,
                b'v' => 0x0b,
                b'E' | b'e' => 0x1b,
                other => other,
            }),
        ))
        .parse_next(input)
    }

    fn radix(digits: &[u8], base: u32) -> u8 {
        digits.iter().fold(0u8, |acc, digit| {
            let value = (*digit as char).to_digit(base).unwrap_or(0) as u8;
            acc.wrapping_mul(base as u8).wrapping_add(value)
        })
    }

    /// Parse a `$'...'` ANSI-C string, decoding `ksh`'s escapes.
    fn ansi_c(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
        delimited(
            literal(b"$'".as_slice()),
            repeat(
                0..,
                alt((
                    preceded(b'\\', escape).map(|byte| vec![byte]),
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

    /// Parse a piece of a value: a `$'...'` run, a plain single-quoted run, or bytes `ksh`
    /// left bare.
    fn value_piece(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
        alt((
            ansi_c,
            delimited(b'\'', take_while(0.., |b: u8| b != b'\''), b'\'').map(<[u8]>::to_vec),
            take_while(1.., |b: u8| b != b'\'' && b != b'$' && b != b'\n').map(<[u8]>::to_vec),
            b'$'.value(vec![b'$']),
        ))
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
            opt(literal(b"alias ".as_slice())),
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
            acc.insert(
                BString::from(name),
                AliasValue::Command(BString::from(value)),
            );
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    fn parse(input: &[u8]) -> HashMap<BString, BString> {
        parse_aliases(input)
            .unwrap()
            .into_iter()
            .map(|(k, v)| match v {
                AliasValue::Command(cmd) => (k, cmd),
                AliasValue::Argv(_) => unreachable!("ksh yields command strings"),
            })
            .collect()
    }

    #[rstest]
    #[case::plain_single_quoted(b"alias ll='ls -l'\n", b"ll", b"ls -l")]
    #[case::bare_value(b"alias fc=hist\n", b"fc", b"hist")]
    #[case::no_alias_prefix(b"ll='ls -l'\n", b"ll", b"ls -l")]
    #[case::ansi_c_embedded_quote(b"alias q=$'echo it\\'s'\n", b"q", b"echo it's")]
    #[case::ansi_c_newline(b"alias n=$'a\\nb'\n", b"n", b"a\nb")]
    #[case::ansi_c_tab(b"alias t=$'a\\tb'\n", b"t", b"a\tb")]
    #[case::ansi_c_escape_uppercase(b"alias e=$'\\E'\n", b"e", b"\x1b")]
    #[case::ansi_c_backslash(b"alias c=$'a\\\\b'\n", b"c", b"a\\b")]
    #[case::ansi_c_hex_control(b"alias d=$'\\x01'\n", b"d", b"\x01")]
    #[case::ansi_c_hex_high_byte(b"alias h=$'\\xff'\n", b"h", b"\xff")]
    #[case::trailing_space_in_value(b"alias command='command '\n", b"command", b"command ")]
    fn parses_value(#[case] input: &[u8], #[case] key: &[u8], #[case] expected: &[u8]) {
        assert_eq!(parse(input)[&BString::from(key)], BString::from(expected));
    }

    #[test]
    fn empty_input_yields_no_aliases() {
        assert!(parse_aliases(b"").unwrap().is_empty());
    }

    #[test]
    fn last_duplicate_wins() {
        assert_eq!(
            parse(b"alias a='first'\nalias a='second'\n")[&BString::from("a")],
            BString::from("second")
        );
    }

    #[rstest]
    #[case::name_does_not_run_across_a_newline(b"alias to use foo\nalias a='b'\n")]
    #[case::trailing_garbage(b"alias ll='ls -l'\nnonsense\n")]
    fn rejects(#[case] input: &[u8]) {
        let result = parse_aliases(input);
        assert!(result.is_err(), "expected Err, got {result:?}");
    }

    // Exact bytes captured from `ksh -ic "...; alias -p"`, including the builtin
    // aliases ksh always emits and a value quoted `$'...'` for its embedded quote.
    #[test]
    fn recovers_real_ksh_listing() {
        let bytes = concat!(
            "alias a='x y'\n",
            "alias command='command '\n",
            "alias fc=hist\n",
            "alias q=$'it\\'s'\n",
            "alias suspend='kill -s STOP $$'\n",
            "alias times='{ { time;} 2>&1;}'\n",
        )
        .as_bytes();
        let m = parse(bytes);
        assert_eq!(m[&BString::from("a")], BString::from("x y"));
        assert_eq!(m[&BString::from("q")], BString::from("it's"));
        assert_eq!(m[&BString::from("command")], BString::from("command "));
        assert_eq!(m[&BString::from("fc")], BString::from("hist"));
        assert_eq!(
            m[&BString::from("times")],
            BString::from("{ { time;} 2>&1;}")
        );
    }
}
