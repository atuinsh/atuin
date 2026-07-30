use ratatui::style::Style;
use ratatui::text::Span;

use crate::theme::SyntaxTheme;

/// Bytes that start/act as shell operators. A run of these is one operator, and
/// resets the "next word is a command" state.
fn is_operator(b: u8) -> bool {
    matches!(
        b,
        b'|' | b'&' | b';' | b'<' | b'>' | b'(' | b')' | b'{' | b'}' | b'`'
    )
}

/// Bytes that end a bare word.
fn is_word_end(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'"' || b == b'\'' || b == b'$' || is_operator(b)
}

/// A dependency-free, "roughly right" shell highlighter. It splits `cmd` into
/// styled spans borrowed from the input: command names, `-flags`, "strings",
/// `$variables`, operators, `# comments`, and plain text. Not a real parser —
/// just enough to read like the legacy tree-sitter highlighting.
pub fn highlight<'a>(cmd: &'a str, syntax: &SyntaxTheme, base: Style) -> Vec<Span<'a>> {
    let bytes = cmd.as_bytes();
    let n = bytes.len();
    let mut spans = Vec::new();
    let mut i = 0;
    // The first word — and the first after each operator (`|`, `;`, `&&`, …) —
    // is a command name.
    let mut expect_command = true;

    while i < n {
        let start = i;
        match bytes[i] {
            b' ' | b'\t' => {
                while i < n && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                spans.push(Span::styled(&cmd[start..i], base));
            }
            b'#' => {
                // `#` only reaches the top of the loop at a token boundary.
                spans.push(Span::styled(&cmd[start..], syntax.comment));
                break;
            }
            quote @ (b'"' | b'\'') => {
                i += 1;
                while i < n && bytes[i] != quote {
                    i += 1;
                }
                i = (i + 1).min(n); // include the closing quote if present
                spans.push(Span::styled(&cmd[start..i], syntax.string));
            }
            b'$' => {
                i += 1;
                if i < n && bytes[i] == b'{' {
                    while i < n && bytes[i] != b'}' {
                        i += 1;
                    }
                    i = (i + 1).min(n);
                } else {
                    while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                        i += 1;
                    }
                }
                spans.push(Span::styled(&cmd[start..i], syntax.variable));
            }
            b if is_operator(b) => {
                while i < n && is_operator(bytes[i]) {
                    i += 1;
                }
                spans.push(Span::styled(&cmd[start..i], syntax.operator));
                expect_command = true;
            }
            _ => {
                while i < n && !is_word_end(bytes[i]) {
                    i += 1;
                }
                let word = &cmd[start..i];
                let style = if expect_command {
                    expect_command = false;
                    syntax.command
                } else if word.starts_with('-') {
                    syntax.flag
                } else {
                    base
                };
                spans.push(Span::styled(word, style));
            }
        }
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(cmd: &str) -> Vec<(String, Style)> {
        let syntax = SyntaxTheme::default();
        highlight(cmd, &syntax, Style::default())
            .into_iter()
            .map(|s| (s.content.into_owned(), s.style))
            .collect()
    }

    #[test]
    fn roundtrips_the_text() {
        let cmd = "git commit -m \"fix #12\" | tee $LOG && echo done # note";
        let joined: String = styles(cmd).into_iter().map(|(t, _)| t).collect();
        assert_eq!(joined, cmd, "highlighting must not drop or reorder bytes");
    }

    #[test]
    fn classifies_categories() {
        let syntax = SyntaxTheme::default();
        let out = styles("git -m \"x\" $HOME | grep");
        // command, flag, string, variable, operator, then a fresh command.
        assert_eq!(out[0], ("git".into(), syntax.command));
        assert!(out.iter().any(|(t, s)| t == "-m" && *s == syntax.flag));
        assert!(out.iter().any(|(t, s)| t == "\"x\"" && *s == syntax.string));
        assert!(out.iter().any(|(t, s)| t == "$HOME" && *s == syntax.variable));
        assert!(out.iter().any(|(t, s)| t == "|" && *s == syntax.operator));
        // `grep` after the pipe is a command again.
        assert!(out.iter().any(|(t, s)| t == "grep" && *s == syntax.command));
    }

    #[test]
    fn comment_runs_to_end() {
        let syntax = SyntaxTheme::default();
        let out = styles("ls # the rest");
        assert_eq!(out.last().unwrap(), &("# the rest".into(), syntax.comment));
    }
}
