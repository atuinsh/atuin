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

    fn var(name: &str, value: &str, export: bool) -> Var {
        Var {
            name: BString::from(name),
            value: BString::from(value),
            export,
        }
    }

    #[test]
    fn spaces_force_python_double_quotes() {
        let r = render_vars(&[var("FOO", "bar baz", true)]);
        assert_eq!(r.script, BString::from("$FOO=\"bar baz\"\n"));
        assert!(r.skipped.is_empty());
    }

    #[test]
    fn bareword_value_is_unquoted_and_export_is_ignored() {
        let r = render_vars(&[var("FOO", "bar", false)]);
        assert_eq!(r.script, BString::from("$FOO=bar\n"));
    }

    #[test]
    fn escapes_backslash_and_double_quote() {
        let r = render_vars(&[var("V", r#"a"b\c"#, true)]);
        assert_eq!(r.script, BString::from(concat!(r#"$V="a\"b\\c""#, "\n")));
    }
}
