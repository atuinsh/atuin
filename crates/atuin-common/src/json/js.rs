//! JSON as JavaScript writes and reads it.

use serde::de::DeserializeOwned;

/// Parse `bytes` as `JSON.parse` would read them: a value `serde_json` rejects is parsed again
/// with invalid UTF-8 and each unpaired surrogate escape replaced by U+FFFD. The error is the
/// original's when that does not help either.
pub fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> serde_json::Result<T> {
    serde_json::from_slice(bytes).or_else(|err| match well_formed(bytes) {
        Some(repaired) => serde_json::from_str(&repaired).map_err(|_| err),
        None => Err(err),
    })
}

/// `bytes` as UTF-8 with invalid sequences and unpaired `\uXXXX` surrogate escapes replaced by
/// U+FFFD, as `String.prototype.toWellFormed` would leave the strings, or `None` when there is
/// neither.
///
/// TODO(markovejnovic): Should this perhaps live in some sort of ut16 interop utility module?
fn well_formed(bytes: &[u8]) -> Option<String> {
    /// The UTF-16 code unit a `\uXXXX` escape at the start of `s` names.
    fn unit(s: &str) -> Option<u16> {
        let hex = s.strip_prefix("\\u")?.get(..4)?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u16::from_str_radix(hex, 16).ok()
    }
    const HIGH: std::ops::RangeInclusive<u16> = 0xD800..=0xDBFF;
    const LOW: std::ops::RangeInclusive<u16> = 0xDC00..=0xDFFF;

    let text = String::from_utf8_lossy(bytes);
    let mut changed = matches!(text, std::borrow::Cow::Owned(_));
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_ref();
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let len = match unit(rest) {
            Some(high)
                if HIGH.contains(&high)
                    && unit(&rest[6..]).is_some_and(|low| LOW.contains(&low)) =>
            {
                12
            }
            Some(lone) if HIGH.contains(&lone) || LOW.contains(&lone) => {
                out.push_str("\\ufffd");
                rest = &rest[6..];
                changed = true;
                continue;
            }
            Some(_) => 6,
            // Any other escape: the backslash and the character it escapes.
            None => 1 + rest[1..].chars().next().map_or(0, char::len_utf8),
        };
        out.push_str(&rest[..len]);
        rest = &rest[len..];
    }
    out.push_str(rest);
    changed.then_some(out)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;
    use serde_json::Value;

    use super::*;

    #[rstest]
    #[case::lone_low(br#""...\ude00aaa""#.as_slice(), Some(r#""...\ufffdaaa""#))]
    #[case::lone_high_at_the_end(br#"{"t":"a\uD83D"}"#.as_slice(), Some(r#"{"t":"a\ufffd"}"#))]
    #[case::high_before_a_non_surrogate(br#""\ud83d\u0041""#.as_slice(), Some(r#""\ufffd\u0041""#))]
    #[case::high_before_a_pair(br#""\ud83d\ud83d\ude00""#.as_slice(), Some(r#""\ufffd\ud83d\ude00""#))]
    #[case::low_after_a_pair(br#""\ud83d\ude00\ude00""#.as_slice(), Some(r#""\ud83d\ude00\ufffd""#))]
    #[case::invalid_utf8(b"\"a\xffb\"".as_slice(), Some("\"a\u{fffd}b\""))]
    #[case::a_pair(br#""\ud83d\ude00""#.as_slice(), None)]
    #[case::an_escaped_backslash(br#""\\ude00 \\\\ud83d""#.as_slice(), None)]
    #[case::other_escapes(br#""\n\"\u00e9\/""#.as_slice(), None)]
    #[case::not_an_escape_at_all(br#""\uzz00""#.as_slice(), None)]
    #[case::cut_short(br#""\ud8"#.as_slice(), None)]
    fn well_formed_replaces_only_what_json_parse_would_make_u_fffd(
        #[case] raw: &[u8],
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(well_formed(raw).as_deref(), expected);
    }

    #[rstest]
    #[case::lone(br#"{"output":"...\ude00a"}"#.as_slice(), serde_json::json!({"output": "...\u{fffd}a"}))]
    #[case::whole(br#"{"output":"\ud83d\ude00"}"#.as_slice(), serde_json::json!({"output": "\u{1f600}"}))]
    #[case::invalid_utf8(b"{\"output\":\"a\xffb\"}".as_slice(), serde_json::json!({"output": "a\u{fffd}b"}))]
    fn a_value_json_parse_reads_is_read(#[case] raw: &[u8], #[case] expected: Value) {
        assert_eq!(from_slice::<Value>(raw).unwrap(), expected);
    }

    /// A value nothing repairs keeps the error it had, not the repaired value's.
    #[rstest]
    fn a_malformed_value_keeps_its_error() {
        let raw = br#"{"output":"\ude00"#;
        let err = from_slice::<Value>(raw).unwrap_err();
        assert_eq!(err.to_string(), serde_json::from_slice::<Value>(raw).unwrap_err().to_string());
        assert!(!err.is_eof(), "the repaired value's end-of-input error leaked out");
    }

    /// `JSON.stringify` of a JavaScript string: its UTF-16 code units, a lone surrogate escaped
    /// as `\uXXXX` and every other character as JSON would have it.
    fn stringify(units: &[u16]) -> String {
        let mut out = String::from("\"");
        for unit in char::decode_utf16(units.iter().copied()) {
            match unit {
                Ok(c) => {
                    let quoted = serde_json::to_string(&c.to_string()).unwrap();
                    out.push_str(&quoted[1..quoted.len() - 1]);
                }
                Err(lone) => out.push_str(&format!("\\u{:04x}", lone.unpaired_surrogate())),
            }
        }
        out.push('"');
        out
    }

    proptest! {
        /// Whatever JavaScript wrote reads back as JavaScript would have it made well-formed.
        #[test]
        fn any_string_javascript_wrote_is_read(
            units in proptest::collection::vec(
                prop_oneof![any::<u16>(), 0xD800u16..=0xDFFF, Just(u16::from(b'\\'))],
                0..48,
            ),
        ) {
            let read: Value = from_slice(stringify(&units).as_bytes()).unwrap();
            prop_assert_eq!(read, Value::from(String::from_utf16_lossy(&units)));
        }

        /// The same with every code unit escaped, the other way `JSON.stringify` output can look.
        #[test]
        fn fully_escaped_strings_are_read(units: Vec<u16>) {
            let escaped: String = units.iter().map(|unit| format!("\\u{unit:04x}")).collect();
            let read: String = from_slice(format!("\"{escaped}\"").as_bytes()).unwrap();
            prop_assert_eq!(read, String::from_utf16_lossy(&units));
        }
    }
}
