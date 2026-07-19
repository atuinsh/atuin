//! Rendering ANSI-encoded terminal output into clean plain text.
//!
//! Terminal programs emit **ANSI escape sequences** (ECMA-48 / ISO 6429: the
//! CSI, SGR, and OSC codes for color, cursor motion, screen clears, progress
//! bars, and so on) interleaved with the visible characters. Recovering just the
//! text a user would *see* is not a matter of deleting "escape-looking" bytes —
//! carriage returns rewrite lines, backspaces erase characters, and cursor moves
//! reorder them.
//!
//! [`to_plain_text`] does it correctly: it drives the bytes through a real VT100
//! terminal emulator and reads back the resulting screen. The emulator is an
//! implementation detail — the input is *terminal output*, not "vt100 text".

/// Upper bound on the emulated screen height, in rows.
///
/// Rows are estimated from the input so scrollback is preserved without
/// wrapping, but pathological input (millions of newlines) must not allocate an
/// unbounded grid, so output is capped at this many lines.
const MAX_ROWS: usize = 10_000;

/// Render ANSI-encoded terminal output to plain text, as it would appear on a
/// `cols`-wide terminal.
///
/// `bytes` is a raw terminal byte stream (e.g. captured stdout/stderr or PTY
/// output) containing ANSI escape sequences. The returned string is what the
/// terminal would display:
///
/// - ANSI escape sequences (colors, cursor motion, screen clears, OSC/DCS) are
///   interpreted and removed.
/// - Carriage-return line rewrites (progress bars) resolve to the final text.
/// - Backspaces erase the preceding character, so terminal echo edits collapse
///   to what the user actually left on screen.
/// - Bare line feeds are treated as newlines even without a carriage return
///   (pipe-captured output), matching a terminal driver's `ONLCR`; a no-op for
///   input that already uses `\r\n`.
/// - Trailing whitespace per line and trailing blank lines are trimmed.
///
/// `cols` is the terminal width; long lines wrap at this boundary, and a `cols`
/// of `0` is treated as `1`. The result contains no terminal control characters
/// other than `\n` line separators; the emulator resolves tabs to spaces, so no
/// literal `\t` survives. Empty input yields an empty string.
///
/// # Cost
///
/// This drives a full terminal emulator and allocates a grid up to
/// `cols * MAX_ROWS` cells. It is intended for post-hoc cleanup of captured
/// command output, not for hot loops.
pub fn to_plain_text(bytes: &[u8], cols: u16) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    let cols = cols.max(1);
    let normalized = normalize_newlines(bytes);
    let rows = estimated_rows(&normalized, cols);

    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(&normalized);

    normalize_screen_contents(&parser.screen().contents())
}

/// Insert a carriage return before any line feed that is not already preceded by
/// one, mimicking a terminal driver's `ONLCR` flag.
///
/// Pipe-captured output uses bare `\n`; in a real terminal a line feed only moves
/// the cursor down without returning to column 0, so without this every line
/// would start further right than the last and eventually wrap into garbage.
/// Input that already uses `\r\n` is returned unchanged (the insert never fires),
/// which is why this is safe to apply unconditionally to PTY-sourced input too.
fn normalize_newlines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 8);
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' && (i == 0 || bytes[i - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

/// Estimate how many rows the emulated screen needs to hold `bytes` without
/// losing content off the top, capped at [`MAX_ROWS`].
///
/// Real terminal output tends to have short lines, so a `bytes / cols` estimate
/// alone badly under-counts; we add the newline count. The extra `+1` leaves a
/// row of headroom for a final partial line.
fn estimated_rows(bytes: &[u8], cols: u16) -> u16 {
    let newline_rows = bytes.iter().filter(|&&b| b == b'\n').count() + 1;
    let wrapped_rows = bytes.len() / cols as usize;
    newline_rows
        .saturating_add(wrapped_rows)
        .saturating_add(1)
        .clamp(1, MAX_ROWS) as u16
}

/// Trim trailing whitespace from each line and drop trailing blank lines,
/// then rejoin with `\n`.
fn normalize_screen_contents(contents: &str) -> String {
    let mut lines = contents.lines().map(str::trim_end).collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rstest::rstest;

    /// Assert the rendered text carries no terminal control characters other
    /// than the line separators a plain-text document legitimately uses.
    fn assert_no_terminal_controls(text: &str) {
        assert!(
            !text
                .chars()
                .any(|ch| ch.is_control() && ch != '\n' && ch != '\t'),
            "text still contains terminal controls: {text:?}"
        );
    }

    /// Table of raw-input -> expected-plain-text cases. These consolidate the
    /// original pty-proxy and atuin-ai unit tests plus additional edge cases.
    #[rstest]
    // Plain text is passed straight through.
    #[case::plain("echo hi", "echo hi")]
    // Empty input -> empty output.
    #[case::empty("", "")]
    // Backspace erases the preceding character (terminal echo edit).
    #[case::single_backspace("e\x08echo hi", "echo hi")]
    // A storm of backspaces still collapses to the final line.
    #[case::backspace_storm(
        "e\x08echo\x08 \x08\x08 \x08\x08\x08e \x08\x08 \x08e\x08echo hi",
        "echo hi"
    )]
    // SGR color codes and a zsh "no trailing newline" `%` marker are removed.
    #[case::ansi_and_percent_marker(
        "\x1b[32mhi\x1b[0m\r\n%                                    \r \r",
        "hi"
    )]
    // Multi-byte UTF-8 survives a backspace edit in the middle of the line.
    #[case::utf8_after_backspace("🦀x\x08 \x08 crab", "🦀 crab")]
    // A carriage-return progress-bar style rewrite keeps only the final text.
    #[case::carriage_return_rewrite("aaaa\rbbbb", "bbbb")]
    // Bare LF (pipe capture) is treated as a newline, so lines start at column 0.
    #[case::bare_lf_is_newline("line one\nline two", "line one\nline two")]
    fn renders_expected_plain_text(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(to_plain_text(input.as_bytes(), 80), expected);
        assert_no_terminal_controls(&to_plain_text(input.as_bytes(), 80));
    }

    #[test]
    fn empty_input_is_empty_regardless_of_cols() {
        assert_eq!(to_plain_text(b"", 0), "");
        assert_eq!(to_plain_text(b"", 1), "");
        assert_eq!(to_plain_text(b"", u16::MAX), "");
    }

    #[test]
    fn zero_cols_is_treated_as_one() {
        // Must not panic (no divide-by-zero, no zero-width grid).
        let _ = to_plain_text(b"anything at all", 0);
    }

    #[test]
    fn trailing_blank_lines_are_trimmed() {
        assert_eq!(to_plain_text(b"hi\r\n\r\n\r\n", 80), "hi");
    }

    #[test]
    fn long_lines_wrap_at_the_column_boundary() {
        // A 10-character line laid out on a 4-wide terminal occupies three grid
        // rows ("abcd" / "efgh" / "ij"), proving the emulator autowraps at the
        // column boundary rather than truncating or overflowing. vt100 marks the
        // continuation rows as soft-wrapped, so `Screen::contents()` rejoins them
        // into the single logical line the user originally typed.
        let wrapped = to_plain_text(b"abcdefghij", 4);
        assert_eq!(wrapped, "abcdefghij");
    }

    proptest! {
        /// For ANY bytes and ANY width, rendering must not panic and must not
        /// leave terminal control characters behind.
        #[test]
        fn never_panics_and_strips_controls(bytes in proptest::collection::vec(any::<u8>(), 0..4096), cols in any::<u16>()) {
            let out = to_plain_text(&bytes, cols);
            prop_assert!(!out.chars().any(|c| c.is_control() && c != '\n' && c != '\t'));
        }

        /// Output never exceeds the row cap, even when every input line carries
        /// content (so blank-line collapsing cannot mask an unbounded grid).
        #[test]
        fn respects_row_cap(lines in (super::MAX_ROWS + 1)..(super::MAX_ROWS + 5_000)) {
            let mut bytes = Vec::with_capacity(lines * 2);
            for _ in 0..lines {
                bytes.extend_from_slice(b"x\n");
            }
            let out = to_plain_text(&bytes, 80);
            prop_assert!(out.lines().count() <= super::MAX_ROWS);
        }

        /// Rendering already-plain single-line printable ASCII (no wrapping) is
        /// idempotent: the first pass trims any trailing spaces, and a second pass
        /// over that output changes nothing.
        #[test]
        fn to_plain_text_is_idempotent_on_clean_single_line(s in "[ -~]{0,80}") {
            let once = to_plain_text(s.as_bytes(), 200);
            let twice = to_plain_text(once.as_bytes(), 200);
            prop_assert_eq!(once, twice);
        }
    }
}
