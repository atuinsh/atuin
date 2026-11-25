//! Writing a list of values as JSON: one array of every value, or one value per line.

use std::io;

use serde::Serialize;

/// How [`write_framed`] frames its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// One JSON array of every value.
    Array,
    /// One JSON value per line.
    Lines,
}

/// Writes each of `items` as JSON, framed by `framing`.
///
/// # Errors
///
/// Fails when `out` fails, or when an item can't be serialized.
pub fn write_framed<T: Serialize>(
    out: &mut impl io::Write,
    items: impl IntoIterator<Item = T>,
    framing: Framing,
) -> io::Result<()> {
    if framing == Framing::Array {
        out.write_all(b"[")?;
    }
    for (n, item) in items.into_iter().enumerate() {
        if framing == Framing::Array && n > 0 {
            out.write_all(b",")?;
        }
        serde_json::to_writer(&mut *out, &item)?;
        if framing == Framing::Lines {
            writeln!(out)?;
        }
    }
    if framing == Framing::Array {
        writeln!(out, "]")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn written(items: &[u8], framing: Framing) -> String {
        let mut out = Vec::new();
        write_framed(&mut out, items, framing).expect("write");
        String::from_utf8(out).expect("UTF-8")
    }

    #[rstest]
    #[case::array(Framing::Array, &[1, 2], "[1,2]\n")]
    #[case::lines(Framing::Lines, &[1, 2], "1\n2\n")]
    #[case::empty_array(Framing::Array, &[], "[]\n")]
    #[case::no_lines(Framing::Lines, &[], "")]
    fn items_are_framed(#[case] framing: Framing, #[case] items: &[u8], #[case] expected: &str) {
        assert_eq!(written(items, framing), expected);
    }
}
