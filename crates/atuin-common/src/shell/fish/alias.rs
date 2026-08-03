use std::collections::HashMap;

use bstr::BString;
use winnow::{
    ModalResult, Parser,
    combinator::{alt, delimited, opt, preceded, repeat, terminated},
    token::{any, literal, take_while},
};

use crate::shell::{Alias, AliasValue, AliasesError, Rendered, Skipped};

use super::Aliases;

/// Parse the output of `alias` into a name → value map.
///
/// The value is whatever `string escape` produced, in one of three forms
/// depending on the fish version and the body's contents:
///   - single-quoted (`'...'`), honouring fish's `\'` and `\\` escapes;
///   - double-quoted (`"..."`), used by fish 4.x when the body has an apostrophe
///     but no offsetting `"`/`$`, honouring `\\`, `\"`, `\$`;
///   - unquoted, script-escaped (fish 3.x, and any version for control chars),
///     honouring `\'` `\\` `\<space>` `\"` `\$`, the named escapes `\t \n \r \b
///     \e \f \v \a`, `\cX`, and the hex byte escapes `\xHH`/`\XHH`.
pub(super) fn parse_aliases(input: &[u8]) -> Result<Aliases, AliasesError> {
    /// Decode the control byte fish writes as `\cX`.
    fn control(byte: u8) -> u8 {
        if byte == b'?' {
            0x7f
        } else {
            byte.to_ascii_uppercase() & 0x1f
        }
    }

    fn radix(digits: &[u8], base: u32) -> u8 {
        digits.iter().fold(0u8, |acc, digit| {
            let value = (*digit as char).to_digit(base).unwrap_or(0) as u8;
            acc.wrapping_mul(base as u8).wrapping_add(value)
        })
    }

    /// Decode one fish script-style escape, the leading `\` already consumed.
    fn escape(input: &mut &[u8]) -> ModalResult<u8> {
        alt((
            preceded(b'c', any).map(control),
            preceded(
                alt((b'x', b'X')),
                take_while(1..=2, |b: u8| b.is_ascii_hexdigit()),
            )
            .map(|digits: &[u8]| radix(digits, 16)),
            any.map(|byte: u8| match byte {
                b'n' => b'\n',
                b't' => b'\t',
                b'r' => b'\r',
                b'b' => 0x08,
                b'e' => 0x1b,
                b'f' => 0x0c,
                b'v' => 0x0b,
                b'a' => 0x07,
                // `\'`, `\\`, `\<space>`, `\"`, `\$`, `\;`, ... decode to the byte.
                other => other,
            }),
        ))
        .parse_next(input)
    }

    /// A piece of a single-quoted body: fish honours only `\'` and `\\` here.
    fn single_piece<'a>(input: &mut &'a [u8]) -> ModalResult<&'a [u8]> {
        alt((
            preceded(
                b'\\',
                alt((literal(br"\".as_slice()), literal(b"'".as_slice()))),
            ),
            take_while(1.., |b: u8| b != b'\'' && b != b'\\'),
            literal(br"\".as_slice()),
        ))
        .parse_next(input)
    }

    fn single_quoted(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
        delimited(
            b'\'',
            repeat(0.., single_piece).fold(Vec::new, |mut acc: Vec<u8>, seg: &[u8]| {
                acc.extend_from_slice(seg);
                acc
            }),
            b'\'',
        )
        .parse_next(input)
    }

    /// A piece of a double-quoted body: fish honours only `\\`, `\"`, `\$` and a
    /// line-continuation `\<newline>`; any other `\x` keeps the backslash.
    fn double_piece(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
        alt((
            preceded(b'\\', any).map(|byte: u8| match byte {
                b'\\' => vec![b'\\'],
                b'"' => vec![b'"'],
                b'$' => vec![b'$'],
                b'\n' => vec![],
                other => vec![b'\\', other],
            }),
            take_while(1.., |b: u8| b != b'"' && b != b'\\').map(<[u8]>::to_vec),
        ))
        .parse_next(input)
    }

    fn double_quoted(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
        delimited(
            b'"',
            repeat(0.., double_piece).fold(Vec::new, |mut acc: Vec<u8>, seg: Vec<u8>| {
                acc.extend_from_slice(&seg);
                acc
            }),
            b'"',
        )
        .parse_next(input)
    }

    /// A piece of an unquoted, script-escaped body: an escape, or a run of bytes
    /// that are neither a delimiter (space/newline) nor the start of an escape.
    fn bare_piece(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
        alt((
            preceded(b'\\', escape).map(|byte| vec![byte]),
            take_while(1.., |b: u8| b != b' ' && b != b'\n' && b != b'\\').map(<[u8]>::to_vec),
        ))
        .parse_next(input)
    }

    fn bare_value(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
        repeat(1.., bare_piece)
            .fold(Vec::new, |mut acc: Vec<u8>, seg: Vec<u8>| {
                acc.extend_from_slice(&seg);
                acc
            })
            .parse_next(input)
    }

    fn alias_line(input: &mut &[u8]) -> ModalResult<(Vec<u8>, Vec<u8>)> {
        (
            literal(b"alias ".as_slice()),
            take_while(1.., |b: u8| b != b' ' && b != b'\n'),
            literal(b" ".as_slice()),
            alt((single_quoted, double_quoted, bare_value)),
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

/// Render aliases as fish `alias name 'body'` definitions.
///
/// fish accepts the POSIX `alias name=value` form too, but its single-quote
/// rules differ: inside `'...'`, fish honours `\'` and `\\` as escapes (POSIX
/// treats them literally). So a body rendered with POSIX quoting can break in
/// fish — e.g. a body ending in a backslash renders to `'a\'`, whose `\'` fish
/// reads as an escaped quote, leaving the string unterminated. This escapes the
/// fish way, mirroring what [`parse_aliases`] decodes.
pub(super) fn render_aliases(aliases: &[Alias]) -> Rendered {
    let mut script = BString::default();
    let mut skipped = Vec::new();

    for alias in aliases {
        if !is_valid_name(&alias.name) {
            skipped.push(Skipped {
                name: alias.name.clone(),
                reason: "not a valid alias name for fish".to_owned(),
            });
            continue;
        }

        script.extend_from_slice(b"alias ");
        script.extend_from_slice(&alias.name);
        script.push(b' ');
        super::fish_single_quote(&alias.value.shcmd(), &mut script);
        script.push(b'\n');
    }

    Rendered { script, skipped }
}

/// A fish alias name must be non-empty and free of whitespace, `=`, single
/// quotes and control characters.
fn is_valid_name(name: &[u8]) -> bool {
    !name.is_empty()
        && !name
            .iter()
            .any(|&b| b == b'=' || b == b'\'' || b.is_ascii_whitespace() || b.is_ascii_control())
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

    #[test]
    fn renders_a_plain_command() {
        let r = render_aliases(&[alias("ll", AliasValue::Command(BString::from("ls -l")))]);
        assert_eq!(r.script, BString::from("alias ll 'ls -l'\n"));
        assert!(r.skipped.is_empty());
    }

    #[rstest]
    #[case::escapes_single_quote_the_fish_way(
        "x",
        AliasValue::Command(BString::from("echo it's")),
        "alias x 'echo it\\'s'\n"
    )]
    // The case POSIX quoting gets wrong in fish: a trailing backslash.
    #[case::escapes_backslash_so_it_cannot_eat_the_quote(
        "b",
        AliasValue::Command(BString::from(r"a\")),
        "alias b 'a\\\\'\n"
    )]
    fn escapes_body(#[case] name: &str, #[case] value: AliasValue, #[case] expected: &str) {
        let r = render_aliases(&[alias(name, value)]);
        assert_eq!(r.script, BString::from(expected));
    }

    #[test]
    fn skips_invalid_names() {
        let r = render_aliases(&[alias("has space", AliasValue::Command(BString::from("x")))]);
        assert!(r.script.is_empty());
        assert_eq!(r.skipped.len(), 1);
    }
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
                AliasValue::Argv(_) => unreachable!("fish yields command strings"),
            })
            .collect()
    }

    #[rstest]
    #[case::parses_bare(b"alias plain man\n".as_slice(), b"plain".as_slice(), b"man".as_slice())]
    #[case::parses_quoted(
        b"alias ll 'ls -l'\n".as_slice(),
        b"ll".as_slice(),
        b"ls -l".as_slice()
    )]
    #[case::decodes_escaped_single_quote(
        br"alias q 'it\'s'".as_slice(),
        b"q".as_slice(),
        b"it's".as_slice()
    )]
    #[case::decodes_double_backslash(
        br"alias b 'a\\b'".as_slice(),
        b"b".as_slice(),
        br"a\b".as_slice()
    )]
    // fish records are `alias NAME VALUE`; an `=` is just an ordinary byte.
    #[case::does_not_use_equals_as_separator(
        b"alias k 'a=b'\n".as_slice(),
        b"k".as_slice(),
        b"a=b".as_slice()
    )]
    // fish 4.x wraps an apostrophe body in double quotes; decode `"..."`.
    #[case::decodes_double_quoted_body(
        b"alias x \"echo it's\"\n".as_slice(),
        b"x".as_slice(),
        b"echo it's".as_slice()
    )]
    #[case::decodes_double_quoted_escapes(
        b"alias v \"a\\\\b\\\"c\\$d\"\n".as_slice(),
        b"v".as_slice(),
        br#"a\b"c$d"#.as_slice()
    )]
    // fish 3.x emits an unquoted, backslash-escaped body: escaped space + quote.
    #[case::decodes_unquoted_escaped_space_and_quote(
        br"alias z echo\ it\'s".as_slice(),
        b"z".as_slice(),
        b"echo it's".as_slice()
    )]
    // Control chars are always emitted unquoted with ANSI-C escapes.
    #[case::decodes_unquoted_tab(
        br"alias t echo\tx".as_slice(),
        b"t".as_slice(),
        b"echo\tx".as_slice()
    )]
    #[case::decodes_unquoted_control(br"alias g \cg".as_slice(), b"g".as_slice(), b"\x07".as_slice())]
    #[case::decodes_unquoted_hex(br"alias h \x41".as_slice(), b"h".as_slice(), b"A".as_slice())]
    fn parses_values(#[case] input: &[u8], #[case] name: &[u8], #[case] expected: &[u8]) {
        assert_eq!(parse(input)[&BString::from(name)], BString::from(expected));
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse_aliases(b"alias ll 'ls -l'\nnonsense\n").is_err());
    }
}
