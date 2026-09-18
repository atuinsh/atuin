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

/// Quote a value for the fish shell.
///
/// `shlex`/POSIX single-quoting is unsafe here: fish recognizes `\'` and `\\` as
/// escapes *inside* single quotes, so a value containing a backslash would leave
/// the string unterminated. Escape backslashes (first) and single quotes instead.
pub fn fish_quote(value: &str) -> String {
    if value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '.')) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

/// Whether a name is safe to emit unquoted as the left-hand side of an `alias`
/// definition.
///
/// A legitimate alias name is an identifier. Anything containing shell
/// metacharacters is treated as a forged or malformed record and dropped by the
/// caller rather than interpolated into shell source.
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Whether a name is a valid shell/environment variable identifier.
///
/// Stricter than [`is_safe_name`]: a variable name is emitted verbatim on the
/// left of an assignment, so it must be a POSIX identifier (`[A-Za-z_][A-Za-z0-9_]*`).
/// A name like `1X` or `A-B` is injection-free but produces an invalid assignment
/// that errors at shell startup, so such records are dropped.
pub fn is_valid_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::{fish_quote, is_safe_name, is_valid_var_name, python_quote};

    #[rstest]
    #[case::simple("simple", "'simple'")]
    #[case::single_quote("don't", "'don\\'t'")]
    #[case::backslash("a\\b", "'a\\\\b'")]
    #[case::newline("a\nb", "'a\\nb'")]
    fn python_quote_cases(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(python_quote(input), expected);
    }

    #[rstest]
    #[case::safe("simple", "simple")]
    #[case::path("path/to/file", "path/to/file")]
    #[case::spaces("hello world", "'hello world'")]
    #[case::single_quote("don't", "'don\\'t'")]
    // A backslash must be escaped, or a trailing one would swallow fish's closing
    // quote and leave the string unterminated.
    #[case::backslash("a\\b", "'a\\\\b'")]
    fn fish_quote_cases(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(fish_quote(input), expected);
    }

    #[rstest]
    #[case::lower("foo", true)]
    #[case::underscore_lead("_foo", true)]
    #[case::env("HOMEBREW_NO_AUTO_UPDATE", true)]
    #[case::empty("", false)]
    #[case::leading_digit("1X", false)]
    #[case::hyphen("A-B", false)]
    #[case::dot("a.b", false)]
    fn is_valid_var_name_cases(#[case] input: &str, #[case] expected: bool) {
        assert_eq!(is_valid_var_name(input), expected);
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
