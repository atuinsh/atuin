use std::collections::HashMap;

use bstr::BString;
use serde::Deserialize;

use crate::shell::{AliasValue, AliasesError};

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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

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

    #[test]
    fn shcmd_quotes_each_argument() {
        let m = parse(br#"{"commit": ["git", "commit", "-m", "hello world"]}"#);
        assert_eq!(
            m[&BString::from(&b"commit"[..])].shcmd(),
            BString::from(&br"'git' 'commit' '-m' 'hello world'"[..])
        );
    }

    #[test]
    fn shcmd_preserves_empty_arguments() {
        let m = parse(br#"{"e": ["echo", "", "x"]}"#);
        assert_eq!(
            m[&BString::from(&b"e"[..])].shcmd(),
            BString::from(&br"'echo' '' 'x'"[..])
        );
    }

    #[test]
    fn shcmd_escapes_embedded_quote() {
        let m = parse(br#"{"q": ["echo", "it's"]}"#);
        assert_eq!(
            m[&BString::from(&b"q"[..])].shcmd(),
            BString::from(&br"'echo' 'it'\''s'"[..])
        );
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
