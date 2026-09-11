use std::ops::Range;

#[cfg_attr(feature = "proto", derive(prost::Message))]
#[cfg_attr(not(feature = "proto"), derive(Debug, Default))]
#[derive(Clone, PartialEq)]
pub struct Segment {
    #[cfg_attr(feature = "proto", prost(string, tag = "1"))]
    pub text: String,
    #[cfg_attr(feature = "proto", prost(bool, tag = "2"))]
    pub highlighted: bool,
}

#[cfg_attr(feature = "proto", derive(prost::Message))]
#[cfg_attr(not(feature = "proto"), derive(Debug, Default))]
#[derive(Clone, PartialEq)]
pub struct HighlightedText {
    #[cfg_attr(feature = "proto", prost(message, repeated, tag = "1"))]
    pub segments: Vec<Segment>,
}

impl HighlightedText {
    #[must_use]
    pub fn from_text_and_matches(text: &str, matches: &[Range<usize>]) -> Self {
        let mut segments = Vec::new();
        let mut cursor = 0;

        for m in matches {
            let start = m.start.min(text.len());
            let end = m.end.min(text.len());
            if start < cursor || start >= end {
                continue;
            }
            let Some(hit) = text.get(start..end) else {
                continue;
            };
            if let Some(plain) = text.get(cursor..start)
                && !plain.is_empty()
            {
                segments.push(Segment {
                    text: plain.to_owned(),
                    highlighted: false,
                });
            }
            segments.push(Segment {
                text: hit.to_owned(),
                highlighted: true,
            });
            cursor = end;
        }

        if let Some(tail) = text.get(cursor..)
            && !tail.is_empty()
        {
            segments.push(Segment {
                text: tail.to_owned(),
                highlighted: false,
            });
        }

        Self { segments }
    }

    #[must_use]
    pub fn text_and_matches(&self) -> (String, Vec<Range<usize>>) {
        let mut text = String::new();
        let mut matches = Vec::new();

        for segment in &self.segments {
            let start = text.len();
            text.push_str(&segment.text);
            if segment.highlighted {
                matches.push(start..text.len());
            }
        }

        (text, matches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, highlighted: bool) -> Segment {
        Segment {
            text: text.to_owned(),
            highlighted,
        }
    }

    #[test]
    fn splits_into_alternating_segments() {
        let hs = HighlightedText::from_text_and_matches("ab cd ef", &[3..5]);
        assert_eq!(hs.segments, vec![seg("ab ", false), seg("cd", true), seg(" ef", false)]);
    }

    #[test]
    fn adjacent_matches_do_not_emit_empty_plain_segments() {
        let hs = HighlightedText::from_text_and_matches("abcd", &[0..2, 2..4]);
        assert_eq!(hs.segments, vec![seg("ab", true), seg("cd", true)]);
    }

    #[test]
    fn round_trips_through_text_and_matches() {
        let text = "the build failed now";
        let matches = vec![4..9, 10..16];
        let hs = HighlightedText::from_text_and_matches(text, &matches);
        assert_eq!(hs.text_and_matches(), (text.to_owned(), matches));
    }

    #[test]
    fn out_of_bounds_and_overlapping_ranges_are_skipped() {
        let hs = HighlightedText::from_text_and_matches("héllo", &[100..200, 1..2]);
        let (text, matches) = hs.text_and_matches();
        assert_eq!(text, "héllo");
        assert!(matches.is_empty());
    }

    #[test]
    fn no_matches_is_a_single_plain_segment() {
        let hs = HighlightedText::from_text_and_matches("plain", &[]);
        assert_eq!(hs.segments, vec![seg("plain", false)]);
    }
}
