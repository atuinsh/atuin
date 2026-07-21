use crate::{Utf32Str, Utf32String};

#[test]
fn test_utf32str_ascii() {
    /// Helper function for testing
    fn expect_ascii(src: &str, is_ascii: bool) {
        let mut buffer = Vec::new();
        assert!(Utf32Str::new(src, &mut buffer).is_ascii() == is_ascii);
        assert!(Utf32String::from(src).slice(..).is_ascii() == is_ascii);
        assert!(Utf32String::from(src.to_owned()).slice(..).is_ascii() == is_ascii);
    }

    // ascii
    expect_ascii("", true);
    expect_ascii("a", true);
    expect_ascii("a\nb", true);
    expect_ascii("\n\r", true);

    // not ascii
    expect_ascii("aü", false);
    expect_ascii("au\u{0308}", false);

    // windows-style newline
    expect_ascii("a\r\nb", false);
    expect_ascii("ü\r\n", false);
    expect_ascii("\r\n", false);
}

#[test]
fn test_grapheme_truncation() {
    // ascii is preserved
    let s = Utf32String::from("ab");
    assert_eq!(s.slice(..).get(0), 'a');
    assert_eq!(s.slice(..).get(1), 'b');

    // windows-style newline is truncated to '\n'
    let s = Utf32String::from("\r\n");
    assert_eq!(s.slice(..).get(0), '\n');

    // normal graphemes are truncated to the first character
    let s = Utf32String::from("u\u{0308}\r\n");
    assert_eq!(s.slice(..).get(0), 'u');
    assert_eq!(s.slice(..).get(1), '\n');
}
