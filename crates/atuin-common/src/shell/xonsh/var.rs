use bstr::BString;

use crate::shell::{
    Rendered, Skipped, Var,
    var::{is_bareword, is_valid_var_name},
};

/// Render vars as xonsh environment assignments: `$NAME=value`. xonsh has only
/// environment variables, so the `export` flag is not represented. Non-bareword
/// values become Python double-quoted strings.
pub(super) fn render_vars(vars: &[Var]) -> Rendered {
    let mut script = BString::default();
    let mut skipped = Vec::new();

    for var in vars {
        if !is_valid_var_name(&var.name) {
            skipped.push(Skipped {
                name: var.name.clone(),
                reason: "not a valid variable name for xonsh".to_owned(),
            });
            continue;
        }
        script.push(b'$');
        script.extend_from_slice(&var.name);
        script.push(b'=');
        if is_bareword(&var.value) {
            script.extend_from_slice(&var.value);
        } else {
            py_str(&var.value, &mut script);
        }
        script.push(b'\n');
    }

    Rendered { script, skipped }
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
        let r = render_vars(&[var(name, value, export)]);
        assert_eq!(r.script, BString::from(expected));
        assert!(r.skipped.is_empty());
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
        let r = render_vars(&[var(name, value, export)]);
        assert_eq!(r.script, BString::from(expected));
    }
}
