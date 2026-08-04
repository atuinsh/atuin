use std::borrow::Cow;

use bstr::{BStr, BString};

use crate::shell::{Var, VarName, VarParsingError};

/// Validate `name` as a nushell environment variable name.
///
/// Env vars are read back as `$env.NAME`, so `NAME` must be a bare member
/// access: an ASCII letter or `_` first, then ASCII alphanumerics or `_`.
#[allow(unsafe_code)]
pub(super) fn validate_var_name(
    name: BString,
    shell: &'static str,
) -> Result<VarName, VarParsingError> {
    let first_ok = matches!(name.first(), Some(&b) if b == b'_' || b.is_ascii_alphabetic());
    let rest_ok = name.iter().all(|&b| b == b'_' || b.is_ascii_alphanumeric());
    if first_ok && rest_ok {
        // SAFETY: validated as a nushell variable name just above.
        Ok(unsafe { VarName::new_unchecked(name) })
    } else {
        Err(VarParsingError::InvalidName { shell, name })
    }
}

/// Render vars as nushell environment assignments: `$env.NAME = "value"`.
/// nushell has only environment variables, so the `export` flag is not
/// represented. The value is always a double-quoted string.
pub(super) fn render_vars(vars: &[Var]) -> BString {
    let mut script = BString::default();
    for var in vars {
        script.extend_from_slice(b"$env.");
        script.extend_from_slice(&var.name);
        script.extend_from_slice(b" = ");
        script.extend_from_slice(&quote_value(&var.value));
        script.push(b'\n');
    }
    script
}

/// The value as a nushell literal, always a double-quoted string.
///
/// nushell evaluates the right-hand side of `$env.NAME = <rhs>` as an
/// expression, so a bareword RHS is not the literal string. So, like the xonsh
/// renderer and unlike the POSIX/fish ones, there is no bareword fast-path:
/// every value is quoted.
pub(super) fn quote_value(value: &[u8]) -> Cow<'_, BStr> {
    let mut out = BString::default();
    nu_str(value, &mut out);
    Cow::Owned(out)
}

/// Append `value` as a nushell double-quoted string literal, escaping `\`, `"`
/// and control bytes. A raw newline/CR would run past the literal, so control
/// bytes are emitted as `\n`, `\r`, `\t` or nushell's `\u{XX}` escape.
fn nu_str(value: &[u8], out: &mut BString) {
    out.push(b'"');
    for &b in value {
        match b {
            b'\\' => out.extend_from_slice(br"\\"),
            b'"' => out.extend_from_slice(br#"\""#),
            b'\n' => out.extend_from_slice(br"\n"),
            b'\r' => out.extend_from_slice(br"\r"),
            b'\t' => out.extend_from_slice(br"\t"),
            0x00..=0x1f | 0x7f => out.extend_from_slice(format!("\\u{{{b:02x}}}").as_bytes()),
            _ => out.push(b),
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{VarName, VarValue};
    use bstr::BString;
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
    #[case::spaces_force_double_quotes("FOO", "bar baz", true, "$env.FOO = \"bar baz\"\n")]
    // nushell evaluates the RHS as an expression, so even bareword-safe values
    // are quoted as a string literal (mirrors xonsh — no bareword fast-path).
    #[case::safe_chars_are_still_quoted("P", "a_b-c.d/e", true, "$env.P = \"a_b-c.d/e\"\n")]
    // nushell has only `$env`, so the `export` flag has no representation.
    #[case::export_is_ignored("FOO", "bar", false, "$env.FOO = \"bar\"\n")]
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
    #[case::escapes_backslash_and_double_quote(
        "V",
        r#"a"b\c"#,
        concat!(r#"$env.V = "a\"b\\c""#, "\n")
    )]
    // Control bytes are escaped so a raw newline cannot terminate the literal.
    #[case::escapes_control_bytes(
        "V",
        "line1\nline2\ttab",
        concat!(r#"$env.V = "line1\nline2\ttab""#, "\n")
    )]
    // A byte with no named escape falls back to nushell's `\u{XX}` form.
    #[case::escapes_other_control_byte_as_unicode(
        "V",
        "a\u{1b}b",
        concat!(r#"$env.V = "a\u{1b}b""#, "\n")
    )]
    fn renders_var(#[case] name: &str, #[case] value: &str, #[case] expected: &str) {
        let script = render_vars(&[var(name, value, true)]);
        assert_eq!(script, BString::from(expected));
    }

    #[rstest]
    #[case::letter("FOO")]
    #[case::leading_underscore("_x")]
    #[case::digit_after_letter("A1")]
    fn accepts_valid_names(#[case] name: &str) {
        let valid = validate_var_name(BString::from(name), "nu").unwrap();
        assert_eq!(BString::from(valid), BString::from(name));
    }

    #[rstest]
    #[case::empty("")]
    #[case::leading_digit("9to5")]
    #[case::hyphen("a-b")]
    // nushell env names are accessed as `$env.NAME`, so restrict to an ASCII
    // identifier; a non-ASCII name is rejected.
    #[case::non_ascii("café")]
    fn rejects_invalid_names(#[case] name: &str) {
        assert_eq!(
            validate_var_name(BString::from(name), "nu").unwrap_err(),
            VarParsingError::InvalidName {
                shell: "nu",
                name: BString::from(name)
            }
        );
    }
}
