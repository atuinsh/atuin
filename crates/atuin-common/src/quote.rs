//! Quoting of words for the languages that Atuin writes commands in.

use std::borrow::Cow;

/// Quote `word` so that Zsh, Bash, and fish all read it as that one word.
///
/// A word of only unremarkable characters is left bare. Anything else goes in single quotes,
/// inside which none of the three shells expands anything. The two characters that fish still
/// treats specially there, `'` and `\`, are left outside the quotes and backslash-escaped, which
/// reads the same in all three shells. For example, `it's` is `'it'\''s'`.
#[must_use]
pub fn sh(word: &str) -> Cow<'_, str> {
    let bare = |c: char| c.is_ascii_alphanumeric() || "_-./:@,+".contains(c);
    // `=` too, but not first: Zsh expands a leading `=cmd` to the command's path.
    if !word.is_empty() && word.chars().all(|c| bare(c) || c == '=') && !word.starts_with('=') {
        return Cow::Borrowed(word);
    }
    let mut out = String::with_capacity(word.len() + 2);
    let mut open = false;
    for c in word.chars() {
        let special = c == '\'' || c == '\\';
        if open == special {
            // Close the quotes before a special character, reopen them after it.
            out.push('\'');
            open = !open;
        }
        if special {
            out.push('\\');
        }
        out.push(c);
    }
    if open {
        out.push('\'');
    }
    if out.is_empty() {
        out.push_str("''");
    }
    Cow::Owned(out)
}

/// Pushes `value` to `out` as a raw string of Nushell, which takes every character as it is.
///
/// A raw string ends at `'` followed by as many `#` as it starts with, so the string uses one `#`
/// more than the longest such run in `value`.
pub fn push_nu_raw(out: &mut String, value: &str) {
    let longest = value
        .split('\'')
        .skip(1)
        .map(|after| after.len() - after.trim_start_matches('#').len())
        .max()
        .unwrap_or(0);
    let hashes = longest + 1;
    out.push('r');
    out.extend(std::iter::repeat_n('#', hashes));
    out.push('\'');
    out.push_str(value);
    out.push('\'');
    out.extend(std::iter::repeat_n('#', hashes));
}

/// The characters that make an argument of a desktop entry's `Exec` need quotes, by the desktop
/// entry specification.
const DESKTOP_RESERVED: &[char] = &[
    ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')',
    '`',
];

/// Pushes the concatenation of `parts` to `exec` as one argument of a desktop entry's `Exec`.
///
/// The argument still needs the escapes of the entry's string type, which
/// [`push_desktop_string`] adds.
pub fn push_desktop_exec_arg(exec: &mut String, parts: &[&str]) {
    let quoted = parts.iter().all(|part| part.is_empty())
        || parts.iter().any(|part| part.contains(DESKTOP_RESERVED));
    if quoted {
        exec.push('"');
    }
    for c in parts.iter().flat_map(|part| part.chars()) {
        match c {
            '"' | '`' | '$' | '\\' if quoted => {
                exec.push('\\');
                exec.push(c);
            }
            // A single `%` would start a field code.
            '%' => exec.push_str("%%"),
            _ => exec.push(c),
        }
    }
    if quoted {
        exec.push('"');
    }
}

/// Pushes `value` to `out` with the escapes of a desktop entry's string type.
pub fn push_desktop_string(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::bare("abc-123_x.y/z", "abc-123_x.y/z")]
    #[case::uuid("0199aaaa-bbbb-4ccc-8ddd-eeeeffff0000", "0199aaaa-bbbb-4ccc-8ddd-eeeeffff0000")]
    #[case::empty("", "''")]
    #[case::space("/home/u/my project", "'/home/u/my project'")]
    #[case::single_quote("it's", r"'it'\''s'")]
    #[case::only_quote("'", r"\'")]
    #[case::backslash(r"a\b", r"'a'\\'b'")]
    #[case::double_backslash_quote(r"a\\'", r"'a'\\\\\'")]
    #[case::leading_equals("=ls", "'=ls'")]
    #[case::inner_equals("a=b", "a=b")]
    #[case::dollar("$HOME", "'$HOME'")]
    #[case::glob("*?[x]", "'*?[x]'")]
    #[case::tilde("~/x", "'~/x'")]
    #[case::unicode("/tmp/プロジェクト ü", "'/tmp/プロジェクト ü'")]
    #[case::newline("a\nb", "'a\nb'")]
    #[case::bang("hi!", "'hi!'")]
    fn sh_words(#[case] word: &str, #[case] expected: &str) {
        assert_eq!(sh(word), expected);
    }

    #[rstest]
    #[case::plain("/usr/bin/atuin", "r#'/usr/bin/atuin'#")]
    #[case::a_quote("/home/o'neil", "r#'/home/o'neil'#")]
    #[case::a_quote_and_hash("/x'#y", "r##'/x'#y'##")]
    #[case::two_runs("'##a'#", "r###''##a'#'###")]
    fn nu_raw_strings(#[case] value: &str, #[case] raw: &str) {
        let mut out = String::new();
        push_nu_raw(&mut out, value);
        assert_eq!(out, raw);
    }

    #[rstest]
    #[case::plain(&["/usr/bin/atuin"], "/usr/bin/atuin")]
    #[case::space(&["/a b/atuin"], "\"/a b/atuin\"")]
    #[case::specials(&["/a $b`c\"d\\e"], "\"/a \\$b\\`c\\\"d\\\\e\"")]
    #[case::percent(&["/100%/atuin"], "/100%%/atuin")]
    #[case::parts(&["A", "=", "/x y"], "\"A=/x y\"")]
    #[case::empty(&[""], "\"\"")]
    fn desktop_exec_args(#[case] parts: &[&str], #[case] quoted: &str) {
        let mut exec = String::new();
        push_desktop_exec_arg(&mut exec, parts);
        assert_eq!(exec, quoted);
    }

    #[rstest]
    fn desktop_strings_escape_backslashes_and_controls() {
        let mut out = String::new();
        push_desktop_string(&mut out, "a\\b\nc");
        assert_eq!(out, "a\\\\b\\nc");
    }
}
