use bstr::BString;
use serde::Deserialize;

use crate::shell::{Alias, AliasValue, AliasesError, Rendered, Skipped};

use super::Aliases;

/// One record of the alias probe (`... | ConvertTo-Json`). PowerShell emits
/// PascalCase property names; only `Name` and `Definition` are read.
#[derive(Deserialize)]
struct RawAlias {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Definition")]
    definition: String,
}

/// Parse the JSON emitted by [`super::ALIAS_PROBE`] into a name → value map.
///
/// A PowerShell alias resolves to a single command name (its `Definition`); it
/// cannot carry arguments, so the value is that command name as an
/// [`AliasValue::Command`].
pub(super) fn parse_aliases(input: &[u8]) -> Result<Aliases, AliasesError> {
    let records: Vec<RawAlias> = serde_json::from_slice(input).map_err(|error| {
        let near = String::from_utf8_lossy(input).chars().take(48).collect();

        AliasesError::Parse {
            offset: error.column(),
            near,
        }
    })?;

    Ok(records
        .into_iter()
        .map(|RawAlias { name, definition }| {
            (
                BString::from(name),
                AliasValue::Command(BString::from(definition)),
            )
        })
        .collect())
}

/// Render aliases as PowerShell functions.
///
/// `Set-Alias` cannot bind arguments (see PowerShell/PowerShell#12962), so an
/// alias is rendered as a function that forwards `@args`. A body that begins
/// with a quote is invoked with the call operator `&`. Each definition is
/// wrapped in [`super::secure_command`] so a bad one does not abort shell
/// startup. An alias whose name would break the `function <name>` header, or
/// whose body spans multiple lines (which would misplace `@args`), is skipped.
pub(super) fn render_aliases(aliases: &[Alias]) -> Rendered {
    let mut script = BString::default();
    let mut skipped = Vec::new();

    for alias in aliases {
        if !is_valid_function_name(&alias.name) {
            skipped.push(Skipped {
                name: alias.name.clone(),
                reason: "not a valid function name for powershell".to_owned(),
            });
            continue;
        }

        let body = alias.value.shcmd();
        if body.iter().any(|&b| b == b'\n' || b == b'\r') {
            skipped.push(Skipped {
                name: alias.name.clone(),
                reason: "a powershell alias body cannot span multiple lines".to_owned(),
            });
            continue;
        }

        let mut inner = BString::default();
        inner.extend_from_slice(b"function ");
        inner.extend_from_slice(&alias.name);
        inner.extend_from_slice(b" {\n    ");
        if matches!(body.first(), Some(&b'"' | &b'\'')) {
            inner.extend_from_slice(b"& ");
        }
        inner.extend_from_slice(&body);
        inner.extend_from_slice(b" @args\n}");
        script.extend_from_slice(&super::secure_command(&inner));
    }

    Rendered { script, skipped }
}

/// A PowerShell function name that will not break the `function <name> {`
/// header: non-empty, no whitespace or control bytes, and free of PowerShell
/// metacharacters.
fn is_valid_function_name(name: &[u8]) -> bool {
    const META: &[u8] = br#"{}()[]<>@$;&|,'"`#="#;
    !name.is_empty()
        && !name
            .iter()
            .any(|&b| META.contains(&b) || b.is_ascii_whitespace() || b.is_ascii_control())
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    fn alias(name: &str, value: AliasValue) -> Alias {
        Alias {
            name: BString::from(name),
            value,
        }
    }

    #[rstest]
    // A plain command body forwards `@args` from a function.
    #[case::plain_command(
        "ll",
        AliasValue::Command(BString::from("ls -l")),
        "Invoke-Expression -ErrorAction Continue -Command 'function ll {\n    ls -l @args\n}'\n"
    )]
    // A body that starts with a quote is invoked with the call operator `&`.
    #[case::quoted_body_uses_call_operator(
        "spc",
        AliasValue::Command(BString::from("\"path with spaces\" arg")),
        "Invoke-Expression -ErrorAction Continue -Command 'function spc {\n    & \"path with spaces\" arg @args\n}'\n"
    )]
    // An argv body has no native form; the POSIX-quoted rendering starts with a
    // quote, so it too is called with `&`, and its `'` are doubled by the wrapper.
    #[case::argv_via_shcmd(
        "g",
        AliasValue::Argv(vec![BString::from("git"), BString::from("st")]),
        "Invoke-Expression -ErrorAction Continue -Command 'function g {\n    & ''git'' ''st'' @args\n}'\n"
    )]
    fn renders_alias(#[case] name: &str, #[case] value: AliasValue, #[case] expected: &str) {
        let r = render_aliases(&[alias(name, value)]);
        assert_eq!(r.script, BString::from(expected));
        assert!(r.skipped.is_empty());
    }

    #[rstest]
    #[case::space("has space")]
    #[case::equals("a=b")]
    #[case::pipe("a|b")]
    #[case::dollar("a$b")]
    #[case::paren("a(b)")]
    #[case::brace("a{b}")]
    fn skips_names_that_break_the_function_header(#[case] name: &str) {
        let r = render_aliases(&[alias(name, AliasValue::Command(BString::from("x")))]);
        assert!(r.script.is_empty(), "expected {name:?} to be skipped");
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].name, BString::from(name));
    }

    // A multi-line body would misplace the `@args` forwarding, so it is skipped.
    #[test]
    fn skips_multiline_body() {
        let r = render_aliases(&[alias(
            "m",
            AliasValue::Command(BString::from("line one\nline two")),
        )]);
        assert!(r.script.is_empty());
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].name, BString::from("m"));
    }

    #[test]
    fn skips_invalid_but_keeps_the_rest() {
        let r = render_aliases(&[
            alias("ok", AliasValue::Command(BString::from("cmd"))),
            alias("has space", AliasValue::Command(BString::from("cmd"))),
        ]);
        assert_eq!(
            r.script,
            BString::from(
                "Invoke-Expression -ErrorAction Continue -Command 'function ok {\n    cmd @args\n}'\n"
            )
        );
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].name, BString::from("has space"));
    }

    #[test]
    fn empty_input_renders_nothing() {
        let r = render_aliases(&[]);
        assert!(r.script.is_empty());
        assert!(r.skipped.is_empty());
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn parse(input: &[u8]) -> Aliases {
        parse_aliases(input).unwrap()
    }

    // `Select-Object Name,Definition | ConvertTo-Json` yields a record per alias;
    // any other properties are ignored.
    #[test]
    fn parses_name_and_definition() {
        let input = br#"[
            {"Name":"g","Definition":"git"},
            {"Name":"gco","Definition":"git-checkout"}
        ]"#;
        let m = parse(input);
        assert_eq!(
            m[&BString::from("g")],
            AliasValue::Command(BString::from("git"))
        );
        assert_eq!(
            m[&BString::from("gco")],
            AliasValue::Command(BString::from("git-checkout"))
        );
    }

    #[test]
    fn parses_empty_array() {
        assert_eq!(parse(b"[]"), Aliases::new());
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(parse_aliases(b"[not json").is_err());
    }
}
