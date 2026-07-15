use std::borrow::Cow;

/// Extension trait for anything that can behave like a string to make it easy to
/// escape control characters into a printable, `cat -v`-style representation.
///
/// Intended to help prevent control characters being printed and interpreted by
/// the terminal when printing history as well as to ensure the commands that
/// appear in the interactive search reflect the actual command run rather than
/// just the printable characters.
///
/// The representation is the POSIX caret/meta notation used by `cat -v`:
/// - C0 controls (`0x00..=0x1F`) and DEL (`0x7F`) become `^` followed by the
///   character xor'd with `0x40`, so NUL is `^@`, tab is `^I`, ESC is `^[`, and
///   DEL is `^?`.
/// - C1 controls (`0x80..=0x9F`) become `M-^` followed by their low 7 bits
///   xor'd with `0x40`, so U+009B (single-byte CSI) is `M-^[`.
///
/// Everything else — including spaces and printable multi-byte Unicode — is
/// left untouched.
pub trait EscapeNonPrintablePosixExt: AsRef<str> {
    fn escape_non_printable(&self) -> Cow<'_, str> {
        // Each character escapes to a (possibly empty) prefix followed by a
        // single payload character, so every branch yields the same iterator
        // type without any fixed-size padding.
        let escape_char = |c: char| {
            let (prefix, payload): (&str, char) = if c.is_ascii_control() {
                // C0 controls and DEL: `^@`..`^_` and `^?`.
                ("^", (c as u8 ^ 0x40) as char)
            } else if c.is_control() {
                // C1 controls (U+0080..=U+009F): `cat -v` meta+caret notation.
                ("M-^", ((c as u8 & 0x7f) ^ 0x40) as char)
            } else {
                // Printable (including spaces and multi-byte Unicode): unchanged.
                ("", c)
            };

            prefix.chars().chain(std::iter::once(payload))
        };

        let s = self.as_ref();
        if !s.contains(|c: char| c.is_control()) {
            return Cow::Borrowed(s);
        }

        Cow::Owned(s.chars().flat_map(escape_char).collect())
    }
}

impl<T: AsRef<str>> EscapeNonPrintablePosixExt for T {}

#[cfg(test)]
mod tests {
    use super::EscapeNonPrintablePosixExt;
    use proptest::prelude::*;
    use rstest::rstest;
    use std::borrow::Cow;

    /// Table-driven examples of the exact `cat -v` mapping. Each case is its own
    /// test, so a failure names precisely which input broke.
    #[rstest]
    // Nothing to escape — returned unchanged (space is printable).
    #[case("plain text", "plain text")]
    #[case("two words", "two words")]
    // Printable multi-byte Unicode is preserved; only the control char changes.
    #[case("🐢\x1b[32m🦀", "🐢^[[32m🦀")]
    // C0 controls and DEL → caret notation (`^` + byte ^ 0x40).
    #[case("\x1b[31mfoo", "^[[31mfoo")] // ESC (0x1b)
    #[case("foo\tbar", "foo^Ibar")] // TAB (0x09)
    #[case("a\0b", "a^@b")] // NUL (0x00) — the core of issue #3589
    #[case("a\x7fb", "a^?b")] // DEL (0x7f ^ 0x40 == '?')
    // C1 controls (U+0080..=U+009F) → `cat -v` meta+caret notation.
    #[case("\u{80}", "M-^@")]
    #[case("a\u{9b}b", "aM-^[b")] // single-char CSI: 0x9b & 0x7f == 0x1b → ^[
    #[case("\u{9f}", "M-^_")]
    fn escapes_as_cat_v(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(input.escape_non_printable(), expected);
    }

    #[test]
    fn escapes_all_c0_controls_as_caret() {
        // Exhaustively check every C0 control 0x00..=0x1f → '^' + (byte ^ 0x40).
        for byte in 0x00u8..=0x1f {
            let input = (byte as char).to_string();
            let expected = format!("^{}", (byte ^ 0x40) as char);
            assert_eq!(input.escape_non_printable(), expected, "byte {byte:#04x}");
        }
    }

    proptest! {
        /// The whole point of the function: for ANY input string, the escaped
        /// output never contains a control character.
        #[test]
        fn output_never_contains_control_chars(s in r"(?s).*") {
            let escaped = s.escape_non_printable();
            prop_assert!(!escaped.chars().any(char::is_control));
        }

        /// A string with no control characters is returned borrowed (zero-copy),
        /// and is therefore byte-for-byte unchanged.
        #[test]
        fn control_free_input_is_borrowed(s in r"[^\p{Cc}]*") {
            prop_assert!(matches!(s.escape_non_printable(), Cow::Borrowed(_)));
        }

        /// Escaping is idempotent: the output is already fully printable, so
        /// escaping it a second time changes nothing.
        #[test]
        fn escaping_is_idempotent(s in r"(?s).*") {
            let once = s.escape_non_printable().into_owned();
            let twice = once.escape_non_printable().into_owned();
            prop_assert_eq!(once, twice);
        }
    }
}
