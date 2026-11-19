//! The text of an rc file in the encoding it's kept in: UTF-8, or UTF-16 after a byte order mark,
//! as Windows PowerShell 5.1 writes a profile with `>` or `Out-File`.

use std::io;

/// UTF-16's byte order marks.
const LITTLE_ENDIAN: [u8; 2] = [0xFF, 0xFE];
const BIG_ENDIAN: [u8; 2] = [0xFE, 0xFF];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

impl Encoding {
    /// The encoding of a file's `bytes`, and their text. Fails on bytes that aren't text in it.
    pub(super) fn decode(bytes: &[u8]) -> io::Result<(Self, String)> {
        let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
            let (pairs, []) = rest.as_chunks::<2>() else {
                return Err(invalid("an odd number of bytes isn't UTF-16"));
            };
            let units = pairs.iter().map(|&pair| unit(pair));
            char::decode_utf16(units).collect::<Result<String, _>>().map_err(invalid)
        };
        if let Some(rest) = bytes.strip_prefix(&LITTLE_ENDIAN) {
            return Ok((Self::Utf16Le, utf16(rest, u16::from_le_bytes)?));
        }
        if let Some(rest) = bytes.strip_prefix(&BIG_ENDIAN) {
            return Ok((Self::Utf16Be, utf16(rest, u16::from_be_bytes)?));
        }
        // Neither mark can start UTF-8.
        Ok((Self::Utf8, String::from_utf8(bytes.to_vec()).map_err(invalid)?))
    }

    /// `text` in this encoding, after its byte order mark if `starts` the file.
    pub(super) fn encode(self, text: &str, starts: bool) -> Vec<u8> {
        let utf16 = |mark: [u8; 2], bytes: fn(u16) -> [u8; 2]| {
            let mark = starts.then_some(mark).into_iter().flatten();
            mark.chain(text.encode_utf16().flat_map(bytes)).collect()
        };
        match self {
            Self::Utf8 => text.as_bytes().to_vec(),
            Self::Utf16Le => utf16(LITTLE_ENDIAN, u16::to_le_bytes),
            Self::Utf16Be => utf16(BIG_ENDIAN, u16::to_be_bytes),
        }
    }
}

fn invalid(err: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    proptest! {
        /// A file's text comes back as it was, in the encoding it was in.
        #[test]
        fn text_round_trips(
            text in any::<String>(),
            encoding in prop_oneof![
                Just(Encoding::Utf8),
                Just(Encoding::Utf16Le),
                Just(Encoding::Utf16Be),
            ],
        ) {
            let bytes = encoding.encode(&text, true);
            prop_assert_eq!(Encoding::decode(&bytes).unwrap(), (encoding, text));
        }
    }

    /// Text appended after the start keeps the file's encoding, and no second mark.
    #[rstest]
    fn appended_text_has_no_mark() {
        let mut file = Encoding::Utf16Le.encode("a\r\n", true);
        file.extend(Encoding::Utf16Le.encode("b\r\n", false));
        assert_eq!(Encoding::decode(&file).unwrap(), (Encoding::Utf16Le, "a\r\nb\r\n".into()));
    }

    #[rstest]
    #[case::odd(&[0xFF, 0xFE, b'a'])]
    #[case::lone_surrogate(&[0xFF, 0xFE, 0x00, 0xD8])]
    #[case::not_utf8(&[b'a', 0xFF])]
    fn what_isnt_text_fails(#[case] bytes: &[u8]) {
        let err = Encoding::decode(bytes).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
