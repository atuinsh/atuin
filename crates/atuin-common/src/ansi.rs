//! Rendering ANSI-encoded terminal output.

use std::borrow::Borrow;
use std::num::NonZeroU16;

/// Arbitrary upper bound on the emulated screen height, in rows. Mitigates OOMs with long output.
const MAX_ROWS: usize = 16_384;

/// Render ANSI-encoded terminal output to plain text, as it would appear on a `cols`-wide terminal.
///
/// Uses [`vt100::Parser`] under the hood meaning that backspaces, ANSI codes, etc. are gracefully
/// handled.
///
/// **Note** that this function is not cheap. It drives a full terminal emulator.
pub fn to_plain_text(input: impl AsRef<[u8]>, cols: NonZeroU16) -> String {
    let bytes = input.as_ref();
    if bytes.is_empty() {
        return String::new();
    }

    let cols = cols.get();

    let mut newlines = 0usize;
    let normalized: Vec<u8> = onlcr(bytes)
        .inspect(|&b| {
            if b == b'\n' {
                newlines += 1;
            }
        })
        .collect();

    // Size the grid to fit all output.
    // Terminal output tends to have short lines. `bytes / cols` under-counts because not all lines
    // have the same length. That's why we count newlines (and add one for the last).
    // We then add the bytes/cols case for the extra rows created due to soft-wrapping.
    // Note this overshoots, but that's OK, we'll clean up later.
    let newline_rows = newlines + 1;
    let wrapped_rows = normalized.len() / cols as usize;
    let rows = newline_rows
        .saturating_add(wrapped_rows)
        .saturating_add(1)
        .clamp(1, MAX_ROWS.min(u16::MAX as usize)) as u16;

    let mut parser = vt100::Parser::new(rows, cols, 0);
    parser.process(&normalized);

    // The emulator renders onto a fixed grid, so `contents()` comes back with each row right-padded
    // and blank rows at the bottom. Gotta clean those up.
    let contents = parser.screen().contents();
    let trimmed = contents.trim_end();
    let mut out = String::with_capacity(trimmed.len());
    let mut first = true;
    for line in trimmed.lines() {
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(line.trim_end());
    }
    out
}

/// Insert a `\r` before any `\n` that is not already preceded by one, mimicking
/// [`onlcr`](https://man7.org/linux/man-pages/man1/stty.1.html).
pub fn onlcr<B: Borrow<u8>>(bytes: impl IntoIterator<Item = B>) -> impl Iterator<Item = u8> {
    let mut prev = None;
    bytes.into_iter().flat_map(move |item| {
        let b = *item.borrow();
        let cr = (b == b'\n' && prev != Some(b'\r')).then_some(b'\r');
        prev = Some(b);
        cr.into_iter().chain(std::iter::once(b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rstest::rstest;
    use std::num::NonZeroU16;

    fn nz(cols: u16) -> NonZeroU16 {
        NonZeroU16::new(cols).expect("test column width must be nonzero")
    }

    fn assert_no_terminal_controls(text: &str) {
        assert!(
            !text
                .chars()
                .any(|ch| ch.is_control() && ch != '\n' && ch != '\t'),
            "text still contains terminal controls: {text:?}"
        );
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
    fn renders_expected_plain_text(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(to_plain_text(input.as_bytes(), nz(80)), expected);
        assert_no_terminal_controls(&to_plain_text(input.as_bytes(), nz(80)));
    }

    #[test]
    fn empty_input_is_empty_regardless_of_cols() {
        assert_eq!(to_plain_text(b"", nz(1)), "");
        assert_eq!(to_plain_text(b"", nz(80)), "");
        assert_eq!(to_plain_text(b"", nz(u16::MAX)), "");
    }

    #[test]
    fn trailing_blank_lines_are_trimmed() {
        assert_eq!(to_plain_text(b"hi\r\n\r\n\r\n", nz(80)), "hi");
    }

    #[test]
    fn long_lines_wrap_at_the_column_boundary() {
        let wrapped = to_plain_text(b"abcdefghij", nz(4));
        assert_eq!(wrapped, "abcdefghij");
    }

    proptest! {
        #[test]
        fn never_panics_and_strips_controls(bytes in proptest::collection::vec(any::<u8>(), 0..4096), cols in 1u16..=u16::MAX) {
            let out = to_plain_text(&bytes, nz(cols));
            prop_assert!(!out.chars().any(|c| c.is_control() && c != '\n' && c != '\t'));
        }

        #[test]
        fn respects_row_cap(lines in (super::MAX_ROWS + 1)..(super::MAX_ROWS + 5_000)) {
            let mut bytes = Vec::with_capacity(lines * 2);
            for _ in 0..lines {
                bytes.extend_from_slice(b"x\n");
            }
            let out = to_plain_text(&bytes, nz(80));
            prop_assert!(out.lines().count() <= super::MAX_ROWS);
        }

        #[test]
        fn to_plain_text_is_idempotent_on_clean_single_line(s in "[ -~]{0,80}") {
            let once = to_plain_text(s.as_bytes(), nz(200));
            let twice = to_plain_text(once.as_bytes(), nz(200));
            prop_assert_eq!(once, twice);
        }
    }
}
