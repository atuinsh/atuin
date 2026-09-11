//! Rendering ANSI-encoded terminal output.

use std::num::NonZeroU16;

use crate::string::TrimExt;

/// Render ANSI-encoded terminal output to plain text, as it would appear on a terminal of size
/// `rows` and `cols`.
///
/// The full output will be returned, even if it produces more rows than `rows`: rows that scroll
/// off the top of the emulated screen are captured as they go. The alternate screen is the
/// exception, and behaves as it does in a real terminal -- what it shows leaves nothing behind
/// when it scrolls, and it hides the main screen for as long as it's active.
///
/// Uses [`vt100::Parser`] under the hood meaning that backspaces, ANSI codes, etc. are gracefully
/// handled.
///
/// **Note** that this function is not cheap. It drives a full terminal emulator.
#[must_use]
pub fn to_plain_text(input: &[u8], rows: NonZeroU16, cols: NonZeroU16) -> String {
    #[derive(Default)]
    struct Capture {
        plain_contents: String,
        /// Temporary buffer used to store a partial piece of basic formatted output. We use this as
        /// a holding area to strip the chunk's SGR sequences before appending it to
        /// `plain_contents`.
        ///
        /// TODO(taylordotfish): If we add `RowContents::write_plain` and `Parser::write_plain` to
        /// `atuin-vt100`, we can write directly into `plain_contents` without this intermediate
        /// step.
        buffer: String,
        state: vt100::capture::BasicFormattedCaptureState,
    }

    /// Push a chunk of basic formatted output onto a plain text capture, stripping out SGR escape
    /// sequences.
    fn push_formatted(plain_contents: &mut String, formatted_chunk: &str) {
        for plain_chunk in vt100::capture::basic_formatted_to_plain(formatted_chunk) {
            let mut first = true;
            for plain_line in plain_chunk.split('\n') {
                if !std::mem::take(&mut first) {
                    // Trim the end of the line that was pushed in the previous iteration, and add
                    // the newline that we didn't push.
                    plain_contents.trim_end_matches_in_place([' ', '\t']);
                    plain_contents.push('\n');
                }
                plain_contents.push_str(plain_line);
            }
        }
    }

    impl vt100::Callbacks for Capture {
        fn on_scroll(&mut self, contents: vt100::capture::RowContents<'_>, alternate_screen: bool) {
            if alternate_screen {
                return;
            }
            contents
                .write_formatted_basic(&mut self.buffer, &mut self.state)
                .expect("writing to a String cannot fail");
            push_formatted(&mut self.plain_contents, &self.buffer);
            self.buffer.clear();
        }
    }

    if input.is_empty() {
        return String::new();
    }

    let mut parser = vt100::Parser::new_with_callbacks(rows, cols, 0, Capture::default());
    onlcr(input).for_each(|chunk| parser.process(chunk));

    let (screen, capture) = parser.screen_and_callbacks_mut();
    screen
        .write_contents_formatted_basic(
            &mut capture.buffer,
            vt100::capture::BasicFormattedCaptureRange::Full(&mut capture.state),
        )
        .expect("writing to a String cannot fail");

    // `push_formatted` trims trailing space from each row, but only if the row is followed by a
    // newline. Push a newline so the last row gets trimmed; we will trim trailing newlines anyway
    // before returning.
    capture.buffer.push('\n');
    push_formatted(&mut capture.plain_contents, &capture.buffer);

    // Trim trailing blank lines; a command that doesn't produce much output will leave blank lines
    // at the bottom of the terminal.
    capture.plain_contents.trim_end_matches_in_place('\n');
    std::mem::take(&mut capture.plain_contents)
}

/// Insert a `\r` before any `\n` that is not already preceded by one, mimicking
/// [`onlcr`](https://man7.org/linux/man-pages/man1/stty.1.html).
pub fn onlcr(mut bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        if bytes.is_empty() {
            return None;
        }

        for i in bytes.iter().copied().enumerate().filter_map(|(i, b)| (b == b'\n').then_some(i)) {
            if i.checked_sub(1).is_some_and(|prev| bytes[prev] == b'\r') {
                continue;
            }
            let before = &bytes[..i];
            bytes = &bytes[i + 1..];
            return Some([before, b"\r\n"]);
        }

        Some([std::mem::take(&mut bytes), b""])
    })
    .flatten()
    .filter(|slice| !slice.is_empty())
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    fn nz(size: u16) -> NonZeroU16 {
        NonZeroU16::new(size).expect("test terminal size must be nonzero")
    }

    /// Render `input` on a terminal roomy enough that nothing scrolls off.
    fn render(input: &str) -> String {
        to_plain_text(input.as_bytes(), nz(24), nz(80))
    }

    fn assert_no_terminal_controls(text: &str) {
        assert!(
            !text.chars().any(|ch| ch.is_control() && ch != '\n' && ch != '\t'),
            "text still contains terminal controls: {text:?}"
        );
    }

    /// The previous byte-at-a-time implementation, kept as an oracle for the chunked one.
    fn onlcr_reference(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        let mut prev = None;
        for &b in bytes {
            if b == b'\n' && prev != Some(b'\r') {
                out.push(b'\r');
            }
            out.push(b);
            prev = Some(b);
        }
        out
    }

    fn onlcr_bytes(bytes: &[u8]) -> Vec<u8> {
        onlcr(bytes).flatten().copied().collect()
    }

    #[rstest]
    #[case::plain("echo hi", "echo hi")]
    #[case::empty("", "")]
    #[case::single_backspace("e\x08echo hi", "echo hi")]
    #[case::backspace_storm(
        "e\x08echo\x08 \x08\x08 \x08\x08\x08e \x08\x08 \x08e\x08echo hi",
        "echo hi"
    )]
    #[case::ansi_and_percent_marker(
        "\x1b[32mhi\x1b[0m\r\n%                                    \r \r",
        "hi"
    )]
    #[case::utf8_after_backspace("🦀x\x08 \x08 crab", "🦀 crab")]
    #[case::carriage_return_rewrite("aaaa\rbbbb", "bbbb")]
    #[case::bare_lf_is_newline("line one\nline two", "line one\nline two")]
    #[case::tab_expands_to_spaces("a\tb", "a       b")]
    fn renders_expected_plain_text(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(render(input), expected);
        assert_no_terminal_controls(&render(input));
    }

    #[rstest]
    fn empty_input_is_empty_regardless_of_size(
        #[values(nz(1), nz(24), nz(u16::MAX))] rows: NonZeroU16,
        #[values(nz(1), nz(80), nz(u16::MAX))] cols: NonZeroU16,
    ) {
        assert_eq!(to_plain_text(b"", rows, cols), "");
    }

    #[rstest]
    fn wide_character_on_a_one_column_screen() {
        // A double-width character on a one-column screen used to panic inside vt100 ("attempt to
        // subtract with overflow"); found by never_panics_and_strips_controls. A one-column screen
        // cannot actually render a wide character, so we expect an empty snapshot.
        assert_eq!(to_plain_text("⺀".as_bytes(), nz(24), nz(1)), "");
    }

    #[rstest]
    fn trailing_blank_lines_are_trimmed() {
        assert_eq!(render("hi\r\n\r\n\r\n"), "hi");
    }

    #[rstest]
    fn interior_and_leading_blank_lines_are_kept() {
        assert_eq!(render("\r\n\r\nfirst\r\n\r\n\r\nlast\r\n"), "\n\nfirst\n\n\nlast");
    }

    #[rstest]
    fn row_padding_is_trimmed_but_wrapped_spaces_are_kept() {
        // Spaces the command actually wrote are padding at the end of a row...
        assert_eq!(to_plain_text(b"a  \r\nb  \r\nc  \r\n", nz(24), nz(20)), "a\nb\nc");
        // ...but not when the row soft-wraps, because then they're in the middle of a line.
        assert_eq!(to_plain_text(b"abc   def", nz(24), nz(6)), "abc   def");
    }

    #[rstest]
    fn long_lines_wrap_at_the_column_boundary() {
        let wrapped = to_plain_text(b"abcdefghij", nz(24), nz(4));
        assert_eq!(wrapped, "abcdefghij");
    }

    #[rstest]
    #[case::one_row(nz(1))]
    #[case::shorter_than_the_output(nz(3))]
    #[case::taller_than_the_output(nz(24))]
    fn rows_scrolled_off_the_screen_are_still_returned(#[case] rows: NonZeroU16) {
        let input: String = (0..20).map(|i| format!("line{i}\r\n")).collect();
        let expected: Vec<String> = (0..20).map(|i| format!("line{i}")).collect();
        assert_eq!(to_plain_text(input.as_bytes(), rows, nz(80)), expected.join("\n"));
    }

    #[rstest]
    fn alternate_screen_output_is_dropped_once_it_is_left() {
        let input = b"before\r\n\x1b[?1049hinside the pager\r\n\x1b[?1049lafter\r\n";
        assert_eq!(to_plain_text(input, nz(24), nz(80)), "before\nafter");
    }

    #[rstest]
    fn output_still_on_the_alternate_screen_is_returned() {
        // A pager that never got to restore the main screen: what's on screen is all there is.
        let input = b"before\r\n\x1b[?1049hinside the pager\r\n";
        assert_eq!(to_plain_text(input, nz(24), nz(80)), "inside the pager");
    }

    #[rstest]
    #[case::empty(b"")]
    #[case::bare_lf(b"\n")]
    #[case::crlf(b"\r\n")]
    #[case::bare_lf_between_text(b"a\nb")]
    #[case::crlf_between_text(b"a\r\nb")]
    #[case::consecutive_bare_lfs(b"\n\n")]
    #[case::consecutive_crlfs(b"\r\n\r\n")]
    #[case::trailing_cr(b"a\r")]
    #[case::leading_cr(b"\ra")]
    #[case::crlf_then_bare_lf(b"a\r\n\nb")]
    fn onlcr_matches_the_byte_at_a_time_implementation(#[case] input: &[u8]) {
        assert_eq!(onlcr_bytes(input), onlcr_reference(input));
    }

    #[rstest]
    fn onlcr_never_yields_an_empty_chunk() {
        assert!(onlcr(b"\n\na\n\n").all(|chunk| !chunk.is_empty()));
    }

    proptest! {
        #[test]
        fn never_panics_and_strips_controls(
            bytes in proptest::collection::vec(any::<u8>(), 0..4096),
            // The emulator allocates the whole grid up front, so keep `rows * cols` to a size that
            // won't have the test hogging memory at the widest `cols`.
            rows in 1u16..=16,
            cols in 1u16..=u16::MAX,
        ) {
            let out = to_plain_text(&bytes, nz(rows), nz(cols));
            prop_assert!(!out.chars().any(|c| c.is_control() && c != '\n' && c != '\t'));
        }

        /// Nothing is dropped just because the screen is short: however few rows the emulator has,
        /// every line of output comes back.
        #[test]
        fn all_rows_are_returned_however_short_the_screen(
            lines in 1usize..256,
            rows in 1u16..=64,
        ) {
            let input: String = (0..lines).map(|i| format!("line{i}\r\n")).collect();
            let out = to_plain_text(input.as_bytes(), nz(rows), nz(80));
            prop_assert_eq!(out.lines().count(), lines);
        }

        #[test]
        fn to_plain_text_is_idempotent_on_clean_single_line(s in "[ -~]{0,80}") {
            let once = to_plain_text(s.as_bytes(), nz(24), nz(200));
            let twice = to_plain_text(once.as_bytes(), nz(24), nz(200));
            prop_assert_eq!(once, twice);
        }

        #[test]
        fn onlcr_agrees_with_the_byte_at_a_time_implementation(
            bytes in proptest::collection::vec(prop_oneof![Just(b'\r'), Just(b'\n'), any::<u8>()], 0..256),
        ) {
            prop_assert_eq!(onlcr_bytes(&bytes), onlcr_reference(&bytes));
        }

        #[test]
        fn onlcr_is_idempotent(
            bytes in proptest::collection::vec(prop_oneof![Just(b'\r'), Just(b'\n'), any::<u8>()], 0..256),
        ) {
            let once = onlcr_bytes(&bytes);
            prop_assert_eq!(onlcr_bytes(&once), once);
        }
    }
}
