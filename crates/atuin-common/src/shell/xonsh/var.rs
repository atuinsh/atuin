use std::borrow::Cow;

use bstr::{BStr, BString, ByteSlice};

use crate::shell::{Var, VarName, VarParsingError};

/// Validate `name` as a xonsh variable name: non-empty, an ASCII letter or `_`
/// first, then ASCII alphanumerics or `_`.
#[allow(unsafe_code)]
pub(super) fn validate_var_name(
    name: BString,
    shell: &'static str,
) -> Result<VarName, VarParsingError> {
    let first_ok = matches!(name.first(), Some(&b) if b == b'_' || b.is_ascii_alphabetic());
    let rest_ok = name.iter().all(|&b| b == b'_' || b.is_ascii_alphanumeric());
    if first_ok && rest_ok {
        // SAFETY: validated as a xonsh variable name just above.
        Ok(unsafe { VarName::new_unchecked(name) })
    } else {
        Err(VarParsingError::InvalidName { shell, name })
    }
}

/// Render vars as xonsh environment assignments: `$NAME=value`. xonsh has only
/// environment variables, so the `export` flag is not represented. Non-bareword
/// values become Python double-quoted strings.
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

/// The value as a xonsh literal: borrowed when bareword-safe (only alphanumerics
/// and `_ - / .`), else an owned Python double-quoted string.
pub(super) fn quote_value(value: &[u8]) -> Cow<'_, BStr> {
    if value
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '.'))
    {
        return Cow::Borrowed(value.as_bstr());
    }
    let mut out = BString::default();
    py_str(value, &mut out);
    Cow::Owned(out)
}

/// Append `value` as a Python double-quoted string literal (`\` and `"`
/// escaped).
fn py_str(value: &[u8], out: &mut BString) {
    out.push(b'"');
    for &b in value {
        match b {
            b'\\' => out.extend_from_slice(br"\\"),
            b'"' => out.extend_from_slice(br#"\""#),
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
    #[case::bareword_value_with_all_safe_chars_is_unquoted(
        "P",
        "a_b-c.d/e",
        true,
        "$P=a_b-c.d/e\n"
    )]
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
    #[case::bareword_value_is_unquoted_and_export_is_ignored("FOO", "bar", false, "$FOO=bar\n")]
    #[case::escapes_backslash_and_double_quote(
        "V",
        r#"a"b\c"#,
        true,
        concat!(r#"$V="a\"b\\c""#, "\n")
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
}
