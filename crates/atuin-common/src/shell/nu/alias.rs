use bstr::BString;
use serde::Deserialize;

use crate::shell::{Alias, AliasValue, AliasesError, Rendered, Skipped};

use super::Aliases;

/// One record of `scope aliases | to json`. Only `name` and `expansion` are
/// read; nushell also emits a description and decl ids, which are ignored.
#[derive(Deserialize)]
struct RawAlias {
    name: String,
    expansion: String,
}

/// Parse the JSON emitted by [`super::ALIAS_PROBE`] into a name → value map.
///
/// The `expansion` is the raw command the alias stands for, kept verbatim as an
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
        .map(|RawAlias { name, expansion }| {
            (
                BString::from(name),
                AliasValue::Command(BString::from(expansion)),
            )
        })
        .collect())
}

/// Render aliases as nushell definitions: `alias name = body`.
///
/// A nushell alias RHS is raw code, not a quoted string, so the body is emitted
/// verbatim via [`AliasValue::shcmd`] (a passthrough for a `Command`). An alias
/// whose name would break the `alias name = ` line, or whose body spans multiple
/// lines — a nushell alias is a single line — is skipped rather than emitted.
pub(super) fn render_aliases(aliases: &[Alias]) -> Rendered {
    let mut script = BString::default();
    let mut skipped = Vec::new();

    for alias in aliases {
        if !is_valid_alias_name(&alias.name) {
            skipped.push(Skipped {
                name: alias.name.clone(),
                reason: "not a valid alias name for nushell".to_owned(),
            });
            continue;
        }

        let body = alias.value.shcmd();
        if body.iter().any(|&b| b == b'\n' || b == b'\r') {
            skipped.push(Skipped {
                name: alias.name.clone(),
                reason: "a nushell alias body cannot span multiple lines".to_owned(),
            });
            continue;
        }

        script.extend_from_slice(b"alias ");
        script.extend_from_slice(&alias.name);
        script.extend_from_slice(b" = ");
        script.extend_from_slice(&body);
        script.push(b'\n');
    }

    Rendered { script, skipped }
}

/// A nushell alias name must be non-empty and free of whitespace, control bytes,
/// and the characters that would break parsing of `alias <name> = ...`: the `=`
/// separator, quotes, and nushell's expansion/grouping/comment metacharacters.
fn is_valid_alias_name(name: &[u8]) -> bool {
    const META: &[u8] = br#"="'`$(){}[]<>;&|#\"#;
    !name.is_empty()
        && !name
            .iter()
            .any(|&b| META.contains(&b) || b.is_ascii_whitespace() || b.is_ascii_control())
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::shell::AliasValue;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    fn alias(name: &str, value: AliasValue) -> Alias {
        Alias {
            name: BString::from(name),
            value,
        }
    }

    #[rstest]
    // A command body is emitted raw — a nushell alias RHS is code, not a string.
    #[case::plain_command(
        "ll",
        AliasValue::Command(BString::from("ls -l")),
        "alias ll = ls -l\n"
    )]
    // A pipeline body is likewise raw nushell code.
    #[case::pipeline_body(
        "gs",
        AliasValue::Command(BString::from("git status | head")),
        "alias gs = git status | head\n"
    )]
    // An argv body has no native nushell form; fall back to the POSIX-quoted
    // rendering so argument boundaries survive.
    #[case::argv_via_shcmd(
        "g",
        AliasValue::Argv(vec![BString::from("git"), BString::from("st")]),
        "alias g = 'git' 'st'\n"
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
    #[case::hash("a#b")]
    #[case::paren("a(b)")]
    fn skips_names_that_break_the_alias_line(#[case] name: &str) {
        let r = render_aliases(&[alias(name, AliasValue::Command(BString::from("x")))]);
        assert!(r.script.is_empty(), "expected {name:?} to be skipped");
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(r.skipped[0].name, BString::from(name));
    }

    // A nushell alias RHS is a single line of code; a body with a newline cannot
    // be represented and is skipped rather than truncating the definition.
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
        assert_eq!(r.script, BString::from("alias ok = cmd\n"));
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
    use crate::shell::AliasValue;
    use pretty_assertions::assert_eq;

    fn parse(input: &[u8]) -> Aliases {
        parse_aliases(input).unwrap()
    }

    // `scope aliases | to json` yields a record per alias; only `name` and
    // `expansion` are used, the rest (description, decl ids) are ignored.
    #[test]
    fn parses_name_and_expansion_ignoring_other_fields() {
        let input = br#"[
            {"name":"ll","expansion":"ls -l","description":"Alias for `ls -l`","decl_id":506,"aliased_decl_id":238},
            {"name":"gs","expansion":"git status | head","description":"x","decl_id":506,"aliased_decl_id":null}
        ]"#;
        let m = parse(input);
        assert_eq!(
            m[&BString::from("ll")],
            AliasValue::Command(BString::from("ls -l"))
        );
        assert_eq!(
            m[&BString::from("gs")],
            AliasValue::Command(BString::from("git status | head"))
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
