//! Windows command lines: encoding an argv so `CommandLineToArgvW` and Rust's std read it back.
//!
//! The rules are Windows', but every byte they care about (`"`, `\`, space, tab) is ASCII, and
//! ASCII never occurs inside a multi-byte character, so escaping an `OsStr`'s encoded bytes gives
//! the same result as escaping its UTF-16. That keeps this module free of Windows APIs, outside
//! `os::windows`, and tested on every platform.
#![cfg_attr(not(windows), allow(dead_code, reason = "only os::windows::command spawns with it"))]

use std::ffi::{OsStr, OsString};
use std::{io, iter};

use crate::string::NonNulStr;

/// Quote `program` and escape `args` into a command line, as std does.
pub(super) fn argv_to_command_line(
    program: NonNulStr<&OsStr>,
    args: &[NonNulStr<&OsStr>],
) -> io::Result<OsString> {
    // A Windows path cannot contain `"`, so argv[0] is quoted without escaping.
    if program.as_os_str().as_encoded_bytes().contains(&b'"') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "program path contains a quote"));
    }
    let mut line = OsString::from("\"");
    line.push(program.as_os_str());
    line.push("\"");
    for &arg in args {
        line.push(" ");
        line.push(escape_arg(arg));
    }
    Ok(line)
}

/// Escape `arg` so `CommandLineToArgvW` reads it back, quoting it only if it is empty or
/// contains whitespace.
#[allow(unsafe_code, reason = "joins the escaped bytes back into an OsString")]
fn escape_arg(arg: NonNulStr<&OsStr>) -> OsString {
    let bytes = arg.as_os_str().as_encoded_bytes();
    let quote = bytes.is_empty() || bytes.iter().any(|&b| b == b' ' || b == b'\t');
    let mut escaped = Vec::with_capacity(bytes.len() + 2);
    if quote {
        escaped.push(b'"');
    }

    let mut backslashes = 0;
    for &byte in bytes {
        match byte {
            b'\\' => backslashes += 1,
            // 2n+1 backslashes before an embedded quote: n literal ones and the escape.
            b'"' => {
                escaped.extend(iter::repeat_n(b'\\', backslashes + 1));
                backslashes = 0;
            }
            _ => backslashes = 0,
        }
        escaped.push(byte);
    }

    if quote {
        // Double any trailing backslashes so they don't escape the closing quote.
        escaped.extend(iter::repeat_n(b'\\', backslashes));
        escaped.push(b'"');
    }
    // SAFETY: `escaped` is `arg`'s encoded bytes with ASCII inserted only before a `"` and at the
    // ends, which are boundaries where `from_encoded_bytes_unchecked` allows joining UTF-8.
    unsafe { OsString::from_encoded_bytes_unchecked(escaped) }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    const PROGRAM: &str = r"C:\Program Files\atuin\atuin.exe";

    fn arg(s: &str) -> NonNulStr<&OsStr> {
        NonNulStr::new(OsStr::new(s)).unwrap()
    }

    /// Split `line` by the rules of std's Windows `parse_lp_cmd_line`, which is how a Rust child
    /// such as the daemon reads its arguments, dropping argv[0].
    fn parse_args(line: &OsStr) -> Vec<String> {
        let is_blank = |&b: &u8| b == b' ' || b == b'\t';
        let mut bytes = line.as_encoded_bytes().iter().copied().peekable();

        // argv[0] has no escapes: quotes only toggle, and unquoted whitespace ends it.
        let mut in_quotes = false;
        for byte in bytes.by_ref() {
            match byte {
                b'"' => in_quotes = !in_quotes,
                b' ' | b'\t' if !in_quotes => break,
                _ => {}
            }
        }
        while bytes.next_if(is_blank).is_some() {}

        let mut args = Vec::new();
        let mut cur = Vec::new();
        let mut in_quotes = false;
        while let Some(byte) = bytes.next() {
            match byte {
                b' ' | b'\t' if !in_quotes => {
                    args.push(std::mem::take(&mut cur));
                    while bytes.next_if(is_blank).is_some() {}
                }
                b'\\' => {
                    let mut count = 1;
                    while bytes.next_if_eq(&b'\\').is_some() {
                        count += 1;
                    }
                    if bytes.peek() == Some(&b'"') {
                        cur.extend(iter::repeat_n(b'\\', count / 2));
                        if count % 2 == 1 {
                            bytes.next();
                            cur.push(b'"');
                        }
                    } else {
                        cur.extend(iter::repeat_n(b'\\', count));
                    }
                }
                b'"' if in_quotes => match bytes.peek() {
                    Some(&b'"') => {
                        bytes.next();
                        cur.push(b'"');
                    }
                    Some(_) => in_quotes = false,
                    None => break,
                },
                b'"' => in_quotes = true,
                _ => cur.push(byte),
            }
        }
        if !cur.is_empty() || in_quotes {
            args.push(cur);
        }
        args.into_iter().map(|arg| String::from_utf8(arg).unwrap()).collect()
    }

    #[rstest]
    #[case::plain("plain", "plain")]
    #[case::empty("", r#""""#)]
    #[case::space("a b", r#""a b""#)]
    #[case::quote(r#"a"b"#, r#"a\"b"#)]
    #[case::lone_backslash(r"a\b", r"a\b")]
    #[case::backslash_before_a_quote(r#"a\"b"#, r#"a\\\"b"#)]
    #[case::trailing_backslash_in_quotes(r"a b\", r#""a b\\""#)]
    fn escapes_exactly_as_std_does(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(escape_arg(arg(input)), expected);
    }

    #[rstest]
    fn refuses_a_quote_in_the_program_path() {
        let err = argv_to_command_line(arg(r#"C:\a"b.exe"#), &[]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    proptest! {
        #[test]
        fn a_rust_child_reads_back_any_arguments(
            args in prop::collection::vec(
                prop_oneof![r#"[a é\t\\"]{0,8}"#, any::<String>()]
                    .prop_filter("NUL cannot be passed", |arg| !arg.contains('\0')),
                0..6,
            ),
        ) {
            let argv: Vec<_> = args.iter().map(String::as_str).map(arg).collect();
            let line = argv_to_command_line(arg(PROGRAM), &argv).unwrap();
            prop_assert_eq!(parse_args(&line), args);
        }
    }
}
