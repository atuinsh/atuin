use std::borrow::Cow;

use bstr::{BStr, BString, ByteSlice};

use crate::shell::{Var, VarName, VarParsingError};

/// Validate `name` as a xonsh variable name.
///
/// xonsh's tokenizer yields a `$NAME` env var when `NAME` is a Python identifier
/// (`str.isidentifier()`, Unicode-aware): a letter or `_` first, then
/// alphanumerics or `_`, with non-ASCII letters allowed (`café`, `π`). We mirror
/// that with Rust's Unicode `char` classes (an invalid-UTF-8 name is rejected).
#[allow(unsafe_code)]
pub(super) fn validate_var_name(
    name: BString,
    shell: &'static str,
) -> Result<VarName, VarParsingError> {
    let mut chars = name.chars();
    let first_ok = matches!(chars.next(), Some(c) if c == '_' || c.is_alphabetic());
    let rest_ok = chars.all(|c| c == '_' || c.is_alphanumeric());
    if first_ok && rest_ok {
        // SAFETY: validated as a xonsh variable name just above.
        Ok(unsafe { VarName::new_unchecked(name) })
    } else {
        Err(VarParsingError::InvalidName { shell, name })
    }
}

/// Render vars as xonsh environment assignments: `$NAME=value`. xonsh has only
/// environment variables, so the `export` flag is not represented. The value is
/// always a Python double-quoted string.
pub(super) fn render_vars(vars: &[Var]) -> BString {
    let mut script = BString::default();
    for var in vars {
        script.push(b'$');
        script.extend_from_slice(&var.name);
        script.push(b'=');
        script.extend_from_slice(&quote_value(&var.value));
        script.push(b'\n');
    }
    script
}

/// The value as a xonsh literal, always a Python double-quoted string.
///
/// xonsh evaluates the right-hand side of `$NAME = <rhs>` as a Python
/// expression, so a bareword RHS is Python code — `bar` is a `NameError`,
/// `/usr/local` a `SyntaxError` — never the literal string. So, unlike the
/// POSIX/fish renderers, there is no bareword fast-path: every value is quoted.
pub(super) fn quote_value(value: &[u8]) -> Cow<'_, BStr> {
    let mut out = BString::default();
    py_str(value, &mut out);
    Cow::Owned(out)
}

/// Append `value` as a Python double-quoted string literal, escaping `\`, `"`
/// and control bytes. A raw newline/CR would terminate the literal (and a NUL
/// cannot appear in Python source at all), so those are emitted as `\n`, `\r`,
/// `\t` or `\xHH`, matching the `py_str` in this shell's alias renderer.
fn py_str(value: &[u8], out: &mut BString) {
    out.push(b'"');
    for &b in value {
        match b {
            b'\\' => out.extend_from_slice(br"\\"),
            b'"' => out.extend_from_slice(br#"\""#),
            b'\n' => out.extend_from_slice(br"\n"),
            b'\r' => out.extend_from_slice(br"\r"),
            b'\t' => out.extend_from_slice(br"\t"),
            0x00..=0x1f | 0x7f => out.extend_from_slice(format!("\\x{b:02x}").as_bytes()),
            _ => out.push(b),
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
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
    #[case::spaces_force_python_double_quotes("FOO", "bar baz", true, "$FOO=\"bar baz\"\n")]
    // A bareword RHS is Python code (`NameError`/`SyntaxError`), so even
    // "bareword-safe" values are quoted as a Python string literal.
    #[case::safe_chars_are_still_quoted("P", "a_b-c.d/e", true, "$P=\"a_b-c.d/e\"\n")]
    fn renders_var_and_skips_nothing(
        #[case] name: &str,
        #[case] value: &str,
        #[case] export: bool,
        #[case] expected: &str,
    ) {
        let script = render_vars(&[var(name, value, export)]);
        assert_eq!(script, BString::from(expected));
    }

    #[rstest]
    #[case::plain_value_is_quoted_and_export_is_ignored("FOO", "bar", false, "$FOO=\"bar\"\n")]
    #[case::escapes_backslash_and_double_quote(
        "V",
        r#"a"b\c"#,
        true,
        concat!(r#"$V="a\"b\\c""#, "\n")
    )]
    // Control bytes are escaped so a raw newline cannot terminate the literal.
    #[case::escapes_control_bytes(
        "V",
        "line1\nline2\ttab",
        true,
        concat!(r#"$V="line1\nline2\ttab""#, "\n")
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
    // xonsh accepts Python identifiers, so non-ASCII letters are valid.
    #[case::non_ascii("café")]
    fn accepts_valid_names(#[case] name: &str) {
        let valid = validate_var_name(BString::from(name), "xonsh").unwrap();
        assert_eq!(BString::from(valid), BString::from(name));
    }

    #[rstest]
    #[case::empty("")]
    // A Python identifier cannot start with a digit.
    #[case::leading_digit("9to5")]
    #[case::hyphen("a-b")]
    fn rejects_invalid_names(#[case] name: &str) {
        assert!(validate_var_name(BString::from(name), "xonsh").is_err());
    }
}
