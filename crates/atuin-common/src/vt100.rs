//! Rendering raw terminal byte streams into clean plain text.
//!
//! Terminal programs emit far more than the characters you see: ANSI SGR color
//! codes, cursor-movement escapes, carriage returns that rewrite the current
//! line, backspaces that erase already-typed characters, progress bars, and
//! bracketed-paste / OSC sequences. Naively stripping "escape-looking" bytes
//! with a regex gets this wrong constantly.
//!
//! [`Vt100PlainTextExt`] takes the correct approach: it feeds the bytes through
//! an actual VT100 terminal emulator ([`vt100`]) and reads back the resulting
//! screen contents. Whatever a real terminal would *display* is what you get —
//! nothing more, nothing less.

/// Upper bound on the emulated screen height, in rows.
///
/// The row count is estimated from the input so scrollback is preserved without
/// wrapping, but a pathological input (millions of newlines) must not be able to
/// make us allocate an unbounded grid. Output is therefore capped at this many
/// lines.
const MAX_ROWS: usize = 10_000;

/// Extension trait that renders raw terminal byte streams into clean plain text.
///
/// Implemented for anything that is `AsRef<[u8]>` (e.g. `Vec<u8>`, `&[u8]`,
/// `[u8; N]`), so you can call [`to_plain_text`](Vt100PlainTextExt::to_plain_text)
/// directly on captured output buffers.
///
/// # What it does
///
/// The bytes are fed through a [`vt100`] terminal emulator sized to hold the
/// full output (bounded by [`MAX_ROWS`]), and the emulator's final screen
/// contents are returned. This means:
///
/// - ANSI escape sequences (colors, cursor motion, screen clears, OSC/DCS) are
///   interpreted and removed, leaving only displayed text.
/// - Carriage returns (`\r`) that rewrite a line — as used by progress bars —
///   resolve to the final line contents.
/// - Backspaces (`\x08`) erase the preceding character, so terminal echo edits
///   collapse to what the user actually left on screen.
/// - Bare line feeds (`\n`) are treated as newlines even when the source did not
///   emit a carriage return (pipe-captured output), matching a terminal driver's
///   `ONLCR` behavior. This is a no-op for input that already uses `\r\n`.
/// - Trailing whitespace on each line and trailing blank lines are trimmed.
///
/// The result contains no terminal control characters. The only C0 controls that
/// may remain are `\n` (line separators) and `\t` (if the emulator preserved a
/// literal tab; tabs are typically expanded to spaces).
///
/// # Cost
///
/// This parses the entire input through a terminal emulator and allocates a grid
/// up to `cols * MAX_ROWS` cells. It is intended for post-hoc cleanup of captured
/// command output, not for hot loops.
pub trait Vt100PlainTextExt: AsRef<[u8]> {
    /// Render the bytes to plain text as they would appear on a `cols`-wide
    /// terminal.
    ///
    /// `cols` is the emulated terminal width; long lines wrap at this boundary.
    /// A `cols` of `0` is treated as `1`. Empty input yields an empty string.
    ///
    /// See the [trait docs](Vt100PlainTextExt) for the full list of transforms.
    fn to_plain_text(&self, cols: u16) -> String {
        todo!("implemented in Task 3")
    }
}

impl<T: AsRef<[u8]> + ?Sized> Vt100PlainTextExt for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rstest::rstest;

    /// Assert the rendered text carries no terminal control characters other
    /// than the line/column separators a plain-text document legitimately uses.
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
        assert_eq!(input.as_bytes().to_plain_text(80), expected);
        assert_no_terminal_controls(&input.as_bytes().to_plain_text(80));
    }

    #[test]
    fn empty_input_is_empty_regardless_of_cols() {
        assert_eq!(b"".to_plain_text(0), "");
        assert_eq!(b"".to_plain_text(1), "");
        assert_eq!(b"".to_plain_text(u16::MAX), "");
    }

    #[test]
    fn zero_cols_is_treated_as_one() {
        // Must not panic (no divide-by-zero, no zero-width grid).
        let _ = b"anything at all".to_plain_text(0);
    }

    #[test]
    fn works_on_vec_and_array_receivers() {
        // Trait is available on owned and array receivers, not just &[u8].
        let owned: Vec<u8> = b"echo hi".to_vec();
        assert_eq!(owned.to_plain_text(80), "echo hi");
        let arr: [u8; 7] = *b"echo hi";
        assert_eq!(arr.to_plain_text(80), "echo hi");
    }

    #[test]
    fn trailing_blank_lines_are_trimmed() {
        assert_eq!(b"hi\r\n\r\n\r\n".to_plain_text(80), "hi");
    }

    proptest! {
        /// For ANY bytes and ANY width, rendering must not panic and must not
        /// leave terminal control characters behind.
        #[test]
        fn never_panics_and_strips_controls(bytes in proptest::collection::vec(any::<u8>(), 0..4096), cols in any::<u16>()) {
            let out = bytes.to_plain_text(cols);
            prop_assert!(!out.chars().any(|c| c.is_control() && c != '\n' && c != '\t'));
        }

        /// Output never exceeds the row cap, no matter how many newlines the
        /// input contains.
        #[test]
        fn respects_row_cap(newlines in 0usize..50_000) {
            let bytes = vec![b'\n'; newlines];
            let out = bytes.to_plain_text(80);
            prop_assert!(out.lines().count() <= super::MAX_ROWS);
        }

        /// Rendering already-clean single-line printable ASCII (shorter than the
        /// width, so no wrapping) is idempotent.
        #[test]
        fn idempotent_on_clean_single_line(s in "[ -~]{0,80}") {
            let once = s.as_bytes().to_plain_text(200);
            let twice = once.as_bytes().to_plain_text(200);
            prop_assert_eq!(once, twice);
        }
    }
}
