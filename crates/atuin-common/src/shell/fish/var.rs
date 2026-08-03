use std::borrow::Cow;

use bstr::{BStr, BString, ByteSlice};

use crate::shell::{Var, VarName, VarParsingError};

/// Validate `name` as a fish variable name.
///
/// fish's `valid_var_name` accepts any non-empty name whose characters are all
/// `_` or Unicode alphanumeric (`fish_iswalnum`): there is no leading-digit or
/// ASCII-only restriction, so `9to5` and `café` are valid. We mirror that.
#[allow(unsafe_code)]
pub(super) fn validate_var_name(
    name: BString,
    shell: &'static str,
) -> Result<VarName, VarParsingError> {
    let ok = !name.is_empty() && name.chars().all(|c| c == '_' || c.is_alphanumeric());
    if ok {
        // SAFETY: validated as a fish variable name just above.
        Ok(unsafe { VarName::new_unchecked(name) })
    } else {
        Err(VarParsingError::InvalidName { shell, name })
    }
}

/// Render vars as fish `set` commands: `set -gx NAME value` for exported vars,
/// `set -g NAME value` for shell vars. Non-bareword values are single-quoted
/// with fish escaping.
pub(super) fn render_vars(vars: &[Var]) -> BString {
    let mut script = BString::default();
    for var in vars {
        script.extend_from_slice(if var.export { b"set -gx " } else { b"set -g " });
        script.extend_from_slice(&var.name);
        script.push(b' ');
        script.extend_from_slice(&quote_value(&var.value));
        script.push(b'\n');
    }
    script
}

/// The value as a fish literal: borrowed when bareword-safe (only alphanumerics
/// and `_ - / .`), else an owned single-quoted string with fish escaping.
pub(super) fn quote_value(value: &[u8]) -> Cow<'_, BStr> {
    if value
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '.'))
    {
        return Cow::Borrowed(value.as_bstr());
    }
    let mut out = BString::default();
    super::fish_single_quote(value, &mut out);
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
    #[case::exported_var_uses_gx_and_quotes_spaces(
        "FOO",
        "bar baz",
        true,
        "set -gx FOO 'bar baz'\n"
    )]
    #[case::shell_var_uses_g_not_gx("FOO", "bar baz", false, "set -g FOO 'bar baz'\n")]
    #[case::bareword_value_is_unquoted("FOO", "bar", true, "set -gx FOO bar\n")]
    #[case::bareword_value_with_all_safe_chars_is_unquoted(
        "P",
        "a_b-c.d/e",
        true,
        "set -gx P a_b-c.d/e\n"
    )]
    #[case::escapes_backslash_and_quote_the_fish_way(
        "V",
        r"a'b\",
        true,
        concat!(r"set -gx V 'a\'b\\'", "\n")
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
    // fish has no leading-digit or ASCII-only restriction.
    #[case::leading_digit("9to5")]
    #[case::non_ascii("café")]
    fn accepts_valid_names(#[case] name: &str) {
        let valid = validate_var_name(BString::from(name), "fish").unwrap();
        assert_eq!(BString::from(valid), BString::from(name));
    }

    #[rstest]
    #[case::empty("")]
    #[case::space("a b")]
    #[case::hyphen("a-b")]
    fn rejects_invalid_names(#[case] name: &str) {
        assert!(validate_var_name(BString::from(name), "fish").is_err());
    }
}
