use std::{collections::HashMap, process};

use winnow::{
    ModalResult, Parser,
    combinator::{alt, delimited, opt, preceded, repeat, terminated},
    token::{literal, take_while},
};

use bstr::BString;

use super::{AliasValue, AliasesError, RunError};

pub(super) type Aliases = HashMap<BString, AliasValue>;

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
            delimited(b'"', literal(b"'".as_slice()), b'"'),
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

    records
        .parse(input)
        .map_err(|error| parse_error(input, error.offset()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn map(pairs: &[(&[u8], &[u8])]) -> Aliases {
        pairs
            .iter()
            .map(|(k, v)| (BString::from(*k), AliasValue::Command(BString::from(*v))))
            .collect()
    }

    #[test]
    fn decodes_bash_escaped_quote() {
        assert_eq!(
            parse_aliases(br"alias whoops='echo it'\''s fine'").unwrap(),
            map(&[(b"whoops", b"echo it's fine")])
        );
    }

    #[test]
    fn decodes_dash_double_quoted_quote() {
        assert_eq!(
            parse_aliases(br#"whoops='echo it'"'"'s fine'"#).unwrap(),
            map(&[(b"whoops", b"echo it's fine")])
        );
    }

    #[test]
    fn accepts_dash_missing_alias_prefix() {
        assert_eq!(
            parse_aliases(b"ll='ls -l'\n").unwrap(),
            map(&[(b"ll", b"ls -l")])
        );
    }

    #[test]
    fn preserves_embedded_newline() {
        assert_eq!(
            parse_aliases(b"alias multi='line one\nline two'\nalias after='x'\n").unwrap(),
            map(&[(b"multi", b"line one\nline two"), (b"after", b"x")])
        );
    }

    #[test]
    fn preserves_non_utf8() {
        assert_eq!(
            parse_aliases(b"alias bin='\xff\xfe'").unwrap(),
            map(&[(b"bin", &[0xff, 0xfe])])
        );
    }

    #[test]
    fn name_does_not_run_across_a_newline() {
        // Regression: `take_while(|b| b != b'=')` without excluding `\n` made this return
        // Ok({"to use foo\nalias a": "b"}) -- a WRONG map, not an error.
        let result = parse_aliases(b"alias to use foo\nalias a='b'\n");
        assert!(result.is_err(), "expected Err, got {result:?}");
    }

    #[test]
    fn rejects_trailing_garbage_rather_than_truncating() {
        assert!(parse_aliases(b"alias ll='ls -l'\nnot an alias line\n").is_err());
    }

    #[test]
    fn parses_empty_input_and_empty_values() {
        assert_eq!(parse_aliases(b"").unwrap(), Aliases::new());
        assert_eq!(parse_aliases(b"alias e=''").unwrap(), map(&[(b"e", b"")]));
    }

    #[test]
    fn last_duplicate_definition_wins() {
        assert_eq!(
            parse_aliases(b"alias a='first'\nalias a='second'\n").unwrap(),
            map(&[(b"a", b"second")])
        );
    }
}
