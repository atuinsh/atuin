use bstr::BString;

use crate::shell::{
    Rendered, Skipped, Var,
    var::{is_bareword, is_valid_var_name},
};

/// Render vars as fish `set` commands: `set -gx NAME value` for exported vars,
/// `set -g NAME value` for shell vars. Non-bareword values are single-quoted
/// with fish escaping.
pub(super) fn render_vars(vars: &[Var]) -> Rendered {
    let mut script = BString::default();
    let mut skipped = Vec::new();

    for var in vars {
        if !is_valid_var_name(&var.name) {
            skipped.push(Skipped {
                name: var.name.clone(),
                reason: "not a valid variable name for fish".to_owned(),
            });
            continue;
        }
        script.extend_from_slice(if var.export { b"set -gx " } else { b"set -g " });
        script.extend_from_slice(&var.name);
        script.push(b' ');
        if is_bareword(&var.value) {
            script.extend_from_slice(&var.value);
        } else {
            super::fish_single_quote(&var.value, &mut script);
        }
        script.push(b'\n');
    }

    Rendered { script, skipped }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    fn var(name: &str, value: &str, export: bool) -> Var {
        Var {
            name: BString::from(name),
            value: BString::from(value),
            export,
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
        let r = render_vars(&[var(name, value, export)]);
        assert_eq!(r.script, BString::from(expected));
        assert!(r.skipped.is_empty());
    }
}
