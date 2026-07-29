use std::collections::HashMap;

use bstr::BString;
use winnow::{
    ModalResult, Parser,
    combinator::{alt, delimited, opt, preceded, repeat, terminated},
    token::{any, literal, take_while},
};

use crate::shell::{AliasValue, AliasesError};

use super::Aliases;

/// Parse the output of `alias -L` into a name → value map.
pub(super) fn parse_aliases(input: &[u8]) -> Result<Aliases, AliasesError> {
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

    fn parse(input: &[u8]) -> HashMap<BString, BString> {
        parse_aliases(input)
            .unwrap()
            .into_iter()
            .map(|(k, v)| match v {
                AliasValue::Command(cmd) => (k, cmd),
                AliasValue::Argv(_) => unreachable!("zsh yields command strings"),
            })
            .collect()
    }

    #[test]
    fn parses_bare_value() {
        assert_eq!(
            parse(b"alias plain=man\n")[&BString::from(&b"plain"[..])],
            BString::from(&b"man"[..])
        );
    }

    #[test]
    fn parses_single_quoted_with_escaped_quote() {
        assert_eq!(
            parse(br"alias whoops='echo it'\''s fine'")[&BString::from(&b"whoops"[..])],
            BString::from(&b"echo it's fine"[..])
        );
    }

    #[test]
    fn decodes_ansi_c_newline() {
        assert_eq!(
            parse(b"alias multi=$'line one\\nline two'\n")[&BString::from(&b"multi"[..])],
            BString::from(&b"line one\nline two"[..])
        );
    }

    #[test]
    fn decodes_ansi_c_octal_and_hex_and_backslash() {
        assert_eq!(
            parse(b"alias a=$'\\101'\n")[&BString::from(&b"a"[..])],
            BString::from(&b"A"[..])
        );
        assert_eq!(
            parse(b"alias b=$'\\x41'\n")[&BString::from(&b"b"[..])],
            BString::from(&b"A"[..])
        );
        assert_eq!(
            parse(b"alias c=$'\\\\'\n")[&BString::from(&b"c"[..])],
            BString::from(&b"\\"[..])
        );
    }

    #[test]
    fn name_does_not_run_across_a_newline() {
        assert!(parse_aliases(b"alias to use foo\nalias a=b\n").is_err());
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse_aliases(b"alias ll='ls -l'\nnonsense\n").is_err());
    }
}
