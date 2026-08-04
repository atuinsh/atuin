use std::borrow::Cow;

use bstr::{BStr, BString};

use crate::shell::{Var, VarName, VarParsingError};

/// Validate `name` as a PowerShell variable name.
///
/// Rendered bare as `$env:NAME` / `$NAME`, so restrict to an identifier: an
/// ASCII letter or `_` first, then ASCII alphanumerics or `_`. PowerShell can
/// express more via `${env:...}`, but atuin only syncs names the user chose.
#[allow(unsafe_code)]
pub(super) fn validate_var_name(
    name: BString,
    shell: &'static str,
) -> Result<VarName, VarParsingError> {
    let first_ok = matches!(name.first(), Some(&b) if b == b'_' || b.is_ascii_alphabetic());
    let rest_ok = name.iter().all(|&b| b == b'_' || b.is_ascii_alphanumeric());
    if first_ok && rest_ok {
        // SAFETY: validated as a PowerShell variable name just above.
        Ok(unsafe { VarName::new_unchecked(name) })
    } else {
        Err(VarParsingError::InvalidName { shell, name })
    }
}

/// Render vars as PowerShell assignments: `$env:NAME = 'value'` for exported
/// (environment) vars, `$NAME = 'value'` for session vars. Each line is wrapped
/// in [`super::secure_command`] so a bad one does not abort shell startup.
pub(super) fn render_vars(vars: &[Var]) -> BString {
    let mut script = BString::default();
    for var in vars {
        let mut inner = BString::default();
        inner.extend_from_slice(if var.export { b"$env:" } else { b"$" });
        inner.extend_from_slice(&var.name);
        inner.extend_from_slice(b" = ");
        inner.extend_from_slice(&quote_value(&var.value));
        script.extend_from_slice(&super::secure_command(&inner));
    }
    script
}

/// The value as a PowerShell single-quoted string literal: wrap in `'…'`, with
/// an embedded `'` doubled to `''`. A single-quoted string is fully literal, and
/// the value is always quoted (`$env:X = bar` would run `bar` as a command).
pub(super) fn quote_value(value: &[u8]) -> Cow<'_, BStr> {
    let mut out = BString::default();
    out.push(b'\'');
    for &b in value {
        if b == b'\'' {
            out.extend_from_slice(b"''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    Cow::Owned(out)
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
    #[case::exported_env_var(
        "FOO",
        "bar baz",
        true,
        "Invoke-Expression -ErrorAction Continue -Command '$env:FOO = ''bar baz'''\n"
    )]
    #[case::session_var_when_not_exported(
        "TEST",
        "1",
        false,
        "Invoke-Expression -ErrorAction Continue -Command '$TEST = ''1'''\n"
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

    // `quote_value` is the inner literal; an embedded `'` is doubled once here
    // (`secure_command` doubles again when the line is wrapped).
    #[rstest]
    #[case::plain("bar", "'bar'")]
    #[case::spaces("bar baz", "'bar baz'")]
    #[case::embedded_quote("bar 'baz'", "'bar ''baz'''")]
    fn quotes_value(#[case] value: &str, #[case] expected: &str) {
        assert_eq!(
            quote_value(value.as_bytes()).into_owned(),
            BString::from(expected)
        );
    }

    #[rstest]
    #[case::letter("FOO")]
    #[case::leading_underscore("_x")]
    #[case::digit_after_letter("A1")]
    fn accepts_valid_names(#[case] name: &str) {
        let valid = validate_var_name(BString::from(name), "powershell").unwrap();
        assert_eq!(BString::from(valid), BString::from(name));
    }

    #[rstest]
    #[case::empty("")]
    #[case::leading_digit("9to5")]
    #[case::hyphen("a-b")]
    #[case::non_ascii("café")]
    fn rejects_invalid_names(#[case] name: &str) {
        assert_eq!(
            validate_var_name(BString::from(name), "powershell").unwrap_err(),
            VarParsingError::InvalidName {
                shell: "powershell",
                name: BString::from(name)
            }
        );
    }
}
