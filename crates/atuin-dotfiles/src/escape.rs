//! Escaping helpers for generating shell dotfiles from synced records.
//!
//! Dotfile records are end-to-end encrypted but they are *not* trusted: anyone
//! holding the encryption key (a leaked key, a shared key, a compromised paired
//! device) can forge alias/var records that sync to every host and are eval'd at
//! shell startup. Names and values must therefore be escaped here, at the point
//! of generation, regardless of where the record originated.

/// Quote a value as a Python single-quoted string literal, for xonsh.
pub fn python_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Whether a name is safe to emit unquoted as the left-hand side of an `alias`
/// definition or a variable assignment.
///
/// A legitimate alias or environment variable name is an identifier. Anything
/// containing shell metacharacters is treated as a forged or malformed record
/// and dropped by the caller rather than interpolated into shell source.
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::{is_safe_name, python_quote};

    #[rstest]
    #[case::simple("simple", "'simple'")]
    #[case::single_quote("don't", "'don\\'t'")]
    #[case::backslash("a\\b", "'a\\\\b'")]
    #[case::newline("a\nb", "'a\\nb'")]
    fn python_quote_cases(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(python_quote(input), expected);
    }

    #[rstest]
    #[case::short("k", true)]
    #[case::dashed("git-foo", true)]
    #[case::env("HOMEBREW_NO_AUTO_UPDATE", true)]
    #[case::dotted("l.", true)]
    #[case::empty("", false)]
    #[case::space("my alias", false)]
    #[case::semicolon("x;rm", false)]
    #[case::quote("a'b", false)]
    #[case::dollar("$x", false)]
    fn is_safe_name_cases(#[case] input: &str, #[case] expected: bool) {
        assert_eq!(is_safe_name(input), expected);
    }
}
