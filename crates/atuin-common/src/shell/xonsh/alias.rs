use std::collections::HashMap;

use bstr::BString;
use serde::Deserialize;

use crate::shell::{Alias, AliasValue, AliasesError, Rendered};

use super::Aliases;

#[derive(Deserialize)]
#[serde(untagged)]
enum RawAlias {
    Command(String),
    Argv(Vec<String>),
}

/// Parse the JSON emitted by [`super::ALIAS_PROBE`] into a name → value map.
///
/// xonsh normalises a string alias into a list of arguments, so most values arrive as arrays.
/// Each value is preserved as its argv vector in an [`AliasValue::Argv`]; rendering it back to a
/// command string is deferred to [`AliasValue::shcmd`], which single-quotes each argument so no
/// argument boundary is lost.
pub(super) fn parse_aliases(input: &[u8]) -> Result<Aliases, AliasesError> {
    let records: HashMap<String, RawAlias> = serde_json::from_slice(input).map_err(|error| {
        let near = String::from_utf8_lossy(input).chars().take(48).collect();

        AliasesError::Parse {
            offset: error.column(),
            near,
        }
    })?;

    Ok(records
        .into_iter()
        .map(|(name, value)| {
            let argv: Vec<BString> = match value {
                RawAlias::Command(command) => vec![BString::from(command)],
                RawAlias::Argv(argv) => argv.into_iter().map(BString::from).collect(),
            };
            (BString::from(name), AliasValue::Argv(argv))
        })
        .collect())
}

/// Render aliases as xonsh assignments: `aliases[name] = value`.
///
/// A [`AliasValue::Command`] becomes a Python string; a [`AliasValue::Argv`]
/// becomes a Python list, so an argv alias is exec'd without a reparse — the
/// shape xonsh itself stores. xonsh alias keys are arbitrary strings, so
/// nothing is skipped.
pub(super) fn render_aliases(aliases: &[Alias]) -> Rendered {
    let mut script = BString::default();

    for alias in aliases {
        script.extend_from_slice(b"aliases[");
        py_str(&alias.name, &mut script);
        script.extend_from_slice(b"] = ");
        match &alias.value {
            AliasValue::Command(cmd) => py_str(cmd, &mut script),
            AliasValue::Argv(args) => {
                script.push(b'[');
                for (index, arg) in args.iter().enumerate() {
                    if index > 0 {
                        script.extend_from_slice(b", ");
                    }
                    py_str(arg, &mut script);
                }
                script.push(b']');
            }
        }
        script.push(b'\n');
    }

    Rendered {
        script,
        skipped: Vec::new(),
    }
}

/// Append `bytes` to `out` as a single-quoted Python string literal.
fn py_str(bytes: &[u8], out: &mut BString) {
    out.push(b'\'');
    for &b in bytes {
        match b {
            b'\\' => out.extend_from_slice(br"\\"),
            b'\'' => out.extend_from_slice(br"\'"),
            b'\n' => out.extend_from_slice(br"\n"),
            b'\r' => out.extend_from_slice(br"\r"),
            b'\t' => out.extend_from_slice(br"\t"),
            0x00..=0x1f | 0x7f => out.extend_from_slice(format!("\\x{b:02x}").as_bytes()),
            _ => out.push(b),
        }
    }
    out.push(b'\'');
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

    #[test]
    fn renders_command_as_python_string() {
        let r = render_aliases(&[alias("ll", AliasValue::Command(BString::from("ls -l")))]);
        assert_eq!(r.script, BString::from("aliases['ll'] = 'ls -l'\n"));
        assert!(r.skipped.is_empty());
    }

    #[rstest]
    #[case::renders_argv_as_python_list(
        "g",
        AliasValue::Argv(vec![BString::from("git"), BString::from("status")]),
        "aliases['g'] = ['git', 'status']\n"
    )]
    #[case::escapes_quotes_and_backslashes(
        "q",
        AliasValue::Command(BString::from(r"it's \ x")),
        concat!(r"aliases['q'] = 'it\'s \\ x'", "\n")
    )]
    fn renders_alias(#[case] name: &str, #[case] value: AliasValue, #[case] expected: &str) {
        let r = render_aliases(&[alias(name, value)]);
        assert_eq!(r.script, BString::from(expected));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    fn parse(input: &[u8]) -> Aliases {
        parse_aliases(input).unwrap()
    }

    #[test]
    fn parses_string_and_list_values() {
        let m = parse(br#"{"a": "ls -l", "b": ["git", "status"]}"#);
        assert_eq!(
            m[&BString::from(&b"a"[..])],
            AliasValue::Argv(vec![BString::from(&b"ls -l"[..])])
        );
        assert_eq!(
            m[&BString::from(&b"b"[..])],
            AliasValue::Argv(vec![
                BString::from(&b"git"[..]),
                BString::from(&b"status"[..])
            ])
        );
    }

    #[rstest]
    #[case::shcmd_quotes_each_argument(
        br#"{"commit": ["git", "commit", "-m", "hello world"]}"#.as_slice(),
        b"commit".as_slice(),
        br"'git' 'commit' '-m' 'hello world'".as_slice()
    )]
    #[case::shcmd_preserves_empty_arguments(
        br#"{"e": ["echo", "", "x"]}"#.as_slice(),
        b"e".as_slice(),
        br"'echo' '' 'x'".as_slice()
    )]
    #[case::shcmd_escapes_embedded_quote(
        br#"{"q": ["echo", "it's"]}"#.as_slice(),
        b"q".as_slice(),
        br"'echo' 'it'\''s'".as_slice()
    )]
    fn shcmd_renders_arguments(#[case] input: &[u8], #[case] key: &[u8], #[case] expected: &[u8]) {
        let m = parse(input);
        assert_eq!(m[&BString::from(key)].shcmd(), BString::from(expected));
    }

    #[test]
    fn parses_empty_object() {
        assert_eq!(parse(b"{}"), Aliases::new());
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(parse_aliases(b"{not json").is_err());
    }
}
