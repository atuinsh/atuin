use bstr::{BString, ByteSlice};

/// A shell variable in atuin's neutral model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    pub name: BString,
    pub value: BString,
    /// `true` for an exported environment variable, `false` for a plain shell
    /// variable. Shells that only have environment variables (xonsh) ignore it.
    pub export: bool,
}

/// Whether `value` is safe to emit unquoted in every supported shell: only
/// alphanumerics and `_ - / .`. The empty value is a bareword. Matches the
/// legacy escaping's fast path (Unicode `char::is_alphanumeric`, via bstr's
/// lossy `chars()` — invalid UTF-8 yields U+FFFD, which is not alphanumeric and
/// therefore forces quoting).
pub(super) fn is_bareword(value: &[u8]) -> bool {
    value
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '.'))
}

/// Whether `name` is a usable variable name across the supported shells:
/// non-empty, an ASCII letter or `_` first, then ASCII alphanumerics or `_`.
pub(super) fn is_valid_var_name(name: &[u8]) -> bool {
    match name.first() {
        Some(&b) if b == b'_' || b.is_ascii_alphabetic() => {}
        _ => return false,
    }
    name.iter().all(|&b| b == b'_' || b.is_ascii_alphanumeric())
}
