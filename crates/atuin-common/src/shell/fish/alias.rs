use std::collections::HashMap;

use bstr::BString;
use winnow::{
    ModalResult, Parser,
    combinator::{alt, delimited, opt, preceded, repeat, terminated},
    token::{literal, take_while},
};

use crate::shell::{Alias, AliasValue, AliasesError, Rendered, Skipped};

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
        fish_single_quote(&alias.value.shcmd(), &mut script);
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

/// Append `bytes` to `out`, single-quoted with fish's escaping (`\` → `\\`,
/// `'` → `\'`).
fn fish_single_quote(bytes: &[u8], out: &mut BString) {
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

#[cfg(test)]
mod render_tests {
    use super::*;
    use pretty_assertions::assert_eq;

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

    #[test]
    fn escapes_single_quote_the_fish_way() {
        let r = render_aliases(&[alias("x", AliasValue::Command(BString::from("echo it's")))]);
        assert_eq!(r.script, BString::from("alias x 'echo it\\'s'\n"));
    }

    #[test]
    fn escapes_backslash_so_it_cannot_eat_the_quote() {
        // The case POSIX quoting gets wrong in fish: a trailing backslash.
        let r = render_aliases(&[alias("b", AliasValue::Command(BString::from(r"a\")))]);
        assert_eq!(r.script, BString::from("alias b 'a\\\\'\n"));
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
