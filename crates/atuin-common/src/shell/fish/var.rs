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

    fn var(name: &str, value: &str, export: bool) -> Var {
        Var {
            name: BString::from(name),
            value: BString::from(value),
            export,
        }
    }

    #[test]
    fn exported_var_uses_gx_and_quotes_spaces() {
        let r = render_vars(&[var("FOO", "bar baz", true)]);
        assert_eq!(r.script, BString::from("set -gx FOO 'bar baz'\n"));
        assert!(r.skipped.is_empty());
    }

    #[test]
    fn shell_var_uses_g_not_gx() {
        let r = render_vars(&[var("FOO", "bar baz", false)]);
        assert_eq!(r.script, BString::from("set -g FOO 'bar baz'\n"));
    }

    #[test]
    fn bareword_value_is_unquoted() {
        let r = render_vars(&[var("FOO", "bar", true)]);
        assert_eq!(r.script, BString::from("set -gx FOO bar\n"));
    }

    #[test]
    fn bareword_value_with_all_safe_chars_is_unquoted() {
        let r = render_vars(&[var("P", "a_b-c.d/e", true)]);
        assert_eq!(r.script, BString::from("set -gx P a_b-c.d/e\n"));
        assert!(r.skipped.is_empty());
    }

    #[test]
    fn escapes_backslash_and_quote_the_fish_way() {
        let r = render_vars(&[var("V", r"a'b\", true)]);
        assert_eq!(
            r.script,
            BString::from(concat!(r"set -gx V 'a\'b\\'", "\n"))
        );
    }
}
