use std::collections::HashMap;

use bstr::BString;
use winnow::{
    ModalResult, Parser,
    combinator::{alt, delimited, opt, preceded, repeat, terminated},
    token::{literal, take_while},
};

use crate::shell::{AliasValue, AliasesError};

use super::Aliases;

/// Parse the output of `alias` into a name → value map.
pub(super) fn parse_aliases(input: &[u8]) -> Result<Aliases, AliasesError> {
    fn piece<'a>(input: &mut &'a [u8]) -> ModalResult<&'a [u8]> {
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

    fn quoted_value(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
        delimited(
            b'\'',
            repeat(0.., piece).fold(Vec::new, |mut acc: Vec<u8>, seg: &[u8]| {
                acc.extend_from_slice(seg);
                acc
            }),
            b'\'',
        )
        .parse_next(input)
    }

    fn bare_value(input: &mut &[u8]) -> ModalResult<Vec<u8>> {
        take_while(1.., |b: u8| b != b' ' && b != b'\n')
            .map(<[u8]>::to_vec)
            .parse_next(input)
    }

    fn alias_line(input: &mut &[u8]) -> ModalResult<(Vec<u8>, Vec<u8>)> {
        (
            literal(b"alias ".as_slice()),
            take_while(1.., |b: u8| b != b' ' && b != b'\n'),
            literal(b" ".as_slice()),
            alt((quoted_value, bare_value)),
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

    #[test]
    fn parses_bare_and_quoted() {
        assert_eq!(
            parse(b"alias plain man\n")[&BString::from(&b"plain"[..])],
            BString::from(&b"man"[..])
        );
        assert_eq!(
            parse(b"alias ll 'ls -l'\n")[&BString::from(&b"ll"[..])],
            BString::from(&b"ls -l"[..])
        );
    }

    #[test]
    fn decodes_backslash_escapes() {
        assert_eq!(
            parse(br"alias q 'it\'s'")[&BString::from(&b"q"[..])],
            BString::from(&b"it's"[..])
        );
        assert_eq!(
            parse(br"alias b 'a\\b'")[&BString::from(&b"b"[..])],
            BString::from(&br"a\b"[..])
        );
    }

    #[test]
    fn does_not_use_equals_as_separator() {
        // fish records are `alias NAME VALUE`; an `=` is just an ordinary byte.
        assert_eq!(
            parse(b"alias k 'a=b'\n")[&BString::from(&b"k"[..])],
            BString::from(&b"a=b"[..])
        );
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse_aliases(b"alias ll 'ls -l'\nnonsense\n").is_err());
    }
}
