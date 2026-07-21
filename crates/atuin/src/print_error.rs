use std::io::IsTerminal;

use atuin_client::record::sync::SyncError;
use colored::Colorize;
use crossterm::terminal;

/// Print a prominent error to stderr. Colored and box-bordered when stderr is
/// a TTY, plain "Error: ..." header otherwise. The description is word-wrapped
/// to the terminal width (capped at 100 columns) so the message stays readable.
pub fn print_error(title: &str, description: &str) {
    let is_tty = std::io::stderr().is_terminal();
    let width = if is_tty {
        terminal::size().map_or(80, |(w, _)| w as usize)
    } else {
        80
    }
    .min(100);

    eprintln!();
    if is_tty {
        let bar = "━".repeat(width).red().bold().to_string();
        eprintln!("{bar}");
        eprintln!("  {} {}", "✗".red().bold(), title.red().bold());
        eprintln!("{bar}");
    } else {
        eprintln!("Error: {title}");
        eprintln!("{}", "-".repeat(width));
    }
    eprintln!();

    for line in wrap_text(description, width.saturating_sub(2)) {
        eprintln!("  {line}");
    }
    eprintln!();
}

/// Convert a `SyncError` into an `eyre::Report`, exiting on `WrongKey` after
/// painting the prominent banner.
pub fn format_sync_error(e: SyncError) -> eyre::Report {
    if matches!(e, SyncError::WrongKey) {
        print_error(
            "Wrong encryption key",
            "Your local encryption key cannot decrypt the data on the server. \
             This usually means another machine wrote records with a different key.\n\n\
             To fix this, find the correct key by running `atuin key` on a machine that \
             already syncs successfully, then run `atuin store rekey <key>` here.",
        );
        std::process::exit(1);
    }
    e.into()
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut line_len = 0;
        for word in paragraph.split_whitespace() {
            let word_len = word.chars().count();
            if !line.is_empty() && line_len + 1 + word_len > width {
                out.push(std::mem::take(&mut line));
                line_len = 0;
            }
            if !line.is_empty() {
                line.push(' ');
                line_len += 1;
            }
            line.push_str(word);
            line_len += word_len;
        }
        // Push every paragraph's final line (even empty) so `\n\n` in the
        // input becomes a blank line in the output.
        out.push(line);
    }
    while out.first().is_some_and(String::is_empty) {
        out.remove(0);
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::wrap_text;

    #[test]
    fn wraps_long_text() {
        let lines = wrap_text("the quick brown fox jumps over the lazy dog", 20);
        for line in &lines {
            assert!(line.chars().count() <= 20, "line too long: {line:?}");
        }
        assert_eq!(
            lines.join(" "),
            "the quick brown fox jumps over the lazy dog"
        );
    }

    #[test]
    fn preserves_explicit_newlines() {
        let lines = wrap_text("first line\nsecond line", 80);
        assert_eq!(lines, vec!["first line", "second line"]);
    }

    #[test]
    fn handles_word_longer_than_width() {
        let lines = wrap_text("short superlongword more", 5);
        assert_eq!(lines, vec!["short", "superlongword", "more"]);
    }

    #[test]
    fn preserves_blank_lines_between_paragraphs() {
        let lines = wrap_text("first paragraph\n\nsecond paragraph", 80);
        assert_eq!(lines, vec!["first paragraph", "", "second paragraph"]);
    }

    #[test]
    fn trims_leading_and_trailing_blank_lines() {
        let lines = wrap_text("\n\nbody\n\n", 80);
        assert_eq!(lines, vec!["body"]);
    }
}
