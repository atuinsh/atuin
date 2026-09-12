//! Provides the [`HighlightedText`] utility which expresses the idea of a piece of text with
//! highlight markers.
//!
//! Consider the string:
//!
//! ```text
//! h e l l o w o r l d
//! ```
//!
//! If you add some bytes which express highlighting zones, you can end up with a string
//!
//! ```text
//! [ h e l ] l o w [ o r ] l d
//! ```
//!
//! In this example, I use the bytes `[` and `]` for visual clarity, but normally you'd use unicode
//! characters.
//!
//! We consider the text `h e l` and `[ o r ]` to be "highlighted".
//!
//! TODO(markovejnovic): Currently, only [`char`]s are supported as markers, but it would perhaps be
//!                      wise to support arbitrary marker strings. This is blocked by using smolstr
//!                      or a similar small-string-optimized utility.
//!
//! TODO(markovejnovic): Another relatively useful feature here would be to add support for multiple
//!                      different highlighting schemes -- nested highlights, as well as
//!                      cross-region highlights. We don't need this at this moment, but it would be
//!                      good to support highlighting `[ h ( e l ) ] o w o (r l) d`, as well as
//!                      `[ h ( e l ] o w ) o ( r l ) d`.
use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum NewTextHighlighterError {
    #[error("identical start and end markers given: {0:?}")]
    IdenticalMarkers([char; 2]),
}

/// Structure which encodes how exactly text was highlighted.
#[derive(Debug, Clone, Copy)]
pub struct TextHighlighter {
    /// The first marker used to highlight a piece of text.
    open: char,
    /// The second marker used to highlight a piece of text.
    close: char,
}

impl Default for TextHighlighter {
    fn default() -> Self {
        // Private-use codepoints, so they never collide with anything a terminal would show.
        const DEFAULT_MATCH_OPEN: char = '\u{E000}';
        const DEFAULT_MATCH_CLOSE: char = '\u{E001}';

        TextHighlighter::with_markers([DEFAULT_MATCH_OPEN, DEFAULT_MATCH_CLOSE])
            .expect("the default markers are distinct")
    }
}

impl TextHighlighter {
    /// Create the highlighter with the given markers.
    pub fn with_markers(markers: [char; 2]) -> Result<Self, NewTextHighlighterError> {
        if markers[0] == markers[1] {
            return Err(NewTextHighlighterError::IdenticalMarkers(markers));
        }

        Ok(TextHighlighter {
            open: markers[0],
            close: markers[1],
        })
    }

    /// The `[open, close]` markers this highlighter wraps matches in.
    pub fn markers(self) -> [char; 2] {
        [self.open, self.close]
    }

    /// Take a string which may or may not contain highlight markers and strip it of said highlight
    /// markers.
    ///
    /// TODO(markovejnovic): Could be more optimized for mutable strings and sanitize them in-place.
    pub fn sanitize<'a>(self, text: &'a str) -> Cow<'a, str> {
        if text.contains([self.open, self.close]) {
            Cow::Owned(text.replace([self.open, self.close], ""))
        } else {
            Cow::Borrowed(text)
        }
    }

    /// Take a string which may or may not contain highlight markers and mark it as a highlighted
    /// string.
    ///
    /// If the string does not contain any highlight markers, the resulting HighlightedText won't
    /// either.
    pub fn as_highlighted<S: AsRef<str>>(self, data: S) -> HighlightedText<S> {
        HighlightedText {
            data,
            highlighter: self,
        }
    }
}

/// Represents text which was highlighted by the `TextHighlighter`.
///
/// [`Display`] implementations come in the form of [`Self::display_plain`], [`Self::display_subs`]
/// and [`Self::display_raw`].
pub struct HighlightedText<S> {
    data: S,
    highlighter: TextHighlighter,
}

impl<S> HighlightedText<S> {
    /// Grab a handle to the raw data.
    pub fn raw(&self) -> &S {
        &self.data
    }

    /// Grab a mutable handle to the underlying data.
    pub fn raw_mut(&mut self) -> &mut S {
        &mut self.data
    }

    pub fn markers(&self) -> [char; 2] {
        self.highlighter.markers()
    }
}

impl<S: AsRef<str>> AsRef<str> for HighlightedText<S> {
    fn as_ref(&self) -> &str {
        self.data.as_ref()
    }
}

impl<S: AsRef<str>> fmt::Debug for HighlightedText<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.data.as_ref())
    }
}

impl<S: AsRef<str>> HighlightedText<S> {
    pub fn ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        HighlightedRanges {
            src: self,
            cursor: 0,
            start: None,
        }
    }

    /// [`Display`] the highlighted text, stripping away the highlight markers.
    pub fn display_plain(&self) -> impl fmt::Display + '_ {
        DisplayPlain(self)
    }

    /// [`Display`] the highlighted text, replacing highlighted markers with `subs`.
    pub fn display_subs(&self, subs: [char; 2]) -> impl fmt::Display + '_ {
        DisplaySubs { src: self, subs }
    }

    /// [`Display`] the highlighted text as-is, including markers.
    pub fn display_raw(&self) -> impl fmt::Display + '_ {
        DisplayRaw(self)
    }
}

struct HighlightedRanges<'a, S> {
    src: &'a HighlightedText<S>,
    cursor: usize,
    start: Option<usize>,
}

impl<S: AsRef<str>> Iterator for HighlightedRanges<'_, S> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Range<usize>> {
        let text = self.src.data.as_ref();
        let open = self.src.highlighter.open;
        let close = self.src.highlighter.close;
        loop {
            let rest = &text[self.cursor..];
            let at = rest.find([open, close])?;
            let marker_pos = self.cursor + at;
            if rest[at..].starts_with(open) {
                self.cursor = marker_pos + open.len_utf8();
                self.start = Some(self.cursor);
            } else {
                self.cursor = marker_pos + close.len_utf8();
                if let Some(start) = self.start.take() {
                    return Some(start..marker_pos);
                }
            }
        }
    }
}

struct DisplayPlain<'a, S>(&'a HighlightedText<S>);

impl<S: AsRef<str>> fmt::Display for DisplayPlain<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let h = self.0.highlighter;
        for piece in self.0.data.as_ref().split([h.open, h.close]) {
            f.write_str(piece)?;
        }
        Ok(())
    }
}

struct DisplaySubs<'a, S> {
    src: &'a HighlightedText<S>,
    subs: [char; 2],
}

impl<S: AsRef<str>> fmt::Display for DisplaySubs<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let h = self.src.highlighter;
        let [open_sub, close_sub] = self.subs;
        let mut rest = self.src.data.as_ref();
        while let Some(at) = rest.find([h.open, h.close]) {
            f.write_str(&rest[..at])?;
            let after = &rest[at..];
            if after.starts_with(h.open) {
                write!(f, "{open_sub}")?;
                rest = &after[h.open.len_utf8()..];
            } else {
                write!(f, "{close_sub}")?;
                rest = &after[h.close.len_utf8()..];
            }
        }
        f.write_str(rest)
    }
}

struct DisplayRaw<'a, S>(&'a HighlightedText<S>);

impl<S: AsRef<str>> fmt::Display for DisplayRaw<'_, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.data.as_ref())
    }
}

pub type HighlightedString = HighlightedText<String>;
pub type HighlightedStr<'a> = HighlightedText<&'a str>;
pub type HighlightedCowStr<'a> = HighlightedText<Cow<'a, str>>;

#[cfg(feature = "proto")]
mod proto {
    use thiserror::Error;

    use super::{HighlightedString, HighlightedText, NewTextHighlighterError, TextHighlighter};

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct HighlightedTextProto {
        #[prost(uint32, tag = "1")]
        pub open: u32,
        #[prost(uint32, tag = "2")]
        pub close: u32,
        #[prost(string, tag = "3")]
        pub raw: String,
    }

    #[derive(Debug, Error)]
    pub enum FromHighlightedTextProtoError {
        #[error("marker code point {0:#x} is not a valid char")]
        InvalidMarker(u32),
        #[error(transparent)]
        Markers(#[from] NewTextHighlighterError),
    }

    impl<S: AsRef<str>> From<&HighlightedText<S>> for HighlightedTextProto {
        fn from(value: &HighlightedText<S>) -> Self {
            let [open, close] = value.highlighter.markers();
            Self {
                open: u32::from(open),
                close: u32::from(close),
                raw: value.data.as_ref().to_owned(),
            }
        }
    }

    impl TryFrom<HighlightedTextProto> for HighlightedString {
        type Error = FromHighlightedTextProtoError;

        fn try_from(value: HighlightedTextProto) -> Result<Self, Self::Error> {
            let open = char::from_u32(value.open)
                .ok_or(FromHighlightedTextProtoError::InvalidMarker(value.open))?;
            let close = char::from_u32(value.close)
                .ok_or(FromHighlightedTextProtoError::InvalidMarker(value.close))?;
            Ok(TextHighlighter::with_markers([open, close])?.as_highlighted(value.raw))
        }
    }
}

#[cfg(feature = "proto")]
pub use proto::{FromHighlightedTextProtoError, HighlightedTextProto};

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    /// A highlighter with visible, multibyte markers — distinct from anything the test bodies hold.
    fn highlighter() -> TextHighlighter {
        TextHighlighter::with_markers(['«', '»']).expect("distinct markers")
    }

    /// Assert `ranges()` yields `expected`, and that indexing the raw (still-marked) text with each
    /// range recovers `hits`.
    fn assert_ranges(h: TextHighlighter, body: &str, expected: &[Range<usize>], hits: &[&str]) {
        let hl = h.as_highlighted(body);
        let ranges: Vec<Range<usize>> = hl.ranges().collect();
        assert_eq!(ranges.as_slice(), expected);
        let raw: &str = hl.as_ref();
        let got: Vec<&str> = ranges.iter().map(|r| &raw[r.clone()]).collect();
        assert_eq!(got.as_slice(), hits);
    }

    #[rstest]
    #[case::ascii(['x', 'x'], false)]
    #[case::emoji(['😀', '😀'], false)]
    #[case::whitespace([' ', ' '], false)]
    #[case::null(['\0', '\0'], false)]
    #[case::brackets(['[', ']'], true)]
    #[case::mixed_whitespace([' ', '\n'], true)]
    #[case::control(['\0', '\u{1}'], true)]
    #[case::default_pua(['\u{E000}', '\u{E001}'], true)]
    fn with_markers_ok_iff_distinct(#[case] markers: [char; 2], #[case] ok: bool) {
        assert_eq!(TextHighlighter::with_markers(markers).is_ok(), ok);
    }

    #[test]
    fn identical_markers_error_carries_the_markers() {
        assert!(matches!(
            TextHighlighter::with_markers(['x', 'x']),
            Err(NewTextHighlighterError::IdenticalMarkers(['x', 'x']))
        ));
    }

    #[rstest]
    #[case::empty("", "", true)]
    #[case::clean("clean text", "clean text", true)]
    #[case::only_open("a«b«c", "abc", false)]
    #[case::only_close("a»b»c", "abc", false)]
    #[case::both_markers("a«b»c«d»", "abcd", false)]
    // Entirely-markers input still allocates: same content as `sanitize("")` but a different Cow.
    #[case::all_markers("«»«»", "", false)]
    fn sanitize_strips_markers_and_borrows_only_when_clean(
        #[case] input: &str,
        #[case] expected: &str,
        #[case] borrowed: bool,
    ) {
        let out = highlighter().sanitize(input);
        assert_eq!(matches!(out, Cow::Borrowed(_)), borrowed);
        assert_eq!(out, expected);
    }

    #[test]
    fn markers_chosen_from_ordinary_text_delete_that_text() {
        // `with_markers` only rejects markers equal to each other, never ones that collide with
        // content — so visible markers turn `sanitize` into a data-destroying filter. This is why
        // `Default` uses private-use codepoints.
        let h = TextHighlighter::with_markers(['a', 'b']).unwrap();
        assert_eq!(h.sanitize("banana bread"), "nn red");
    }

    // `ranges()` indexes the raw (still-marked) text, pointing at the content between each
    // open/close pair.
    #[rstest]
    #[case::single_span("the build «failed» now", vec![12..18], vec!["failed"])]
    #[case::each_span_its_own_range("«a» b «c»", vec![2..3, 10..11], vec!["a", "c"])]
    #[case::multibyte_prefix_keeps_offsets("café «error»", vec![8..13], vec!["error"])]
    #[case::no_markers("nothing here", vec![], vec![])]
    // A second open before a close overwrites the first, so the outer span is silently dropped.
    #[case::nested_open_keeps_inner("«a«b»", vec![5..6], vec!["b"])]
    #[case::nested_pair_keeps_inner("«a«b»c»", vec![5..6], vec!["b"])]
    #[case::doubled_markers_collapse("««x»»", vec![4..5], vec!["x"])]
    // An open immediately closed yields a valid zero-width range.
    #[case::empty_span("«»", vec![2..2], vec![""])]
    #[case::adjacent_empty_spans("«»«»", vec![2..2, 6..6], vec!["", ""])]
    #[case::empty_span_between_real("«a»«»«b»", vec![2..3, 7..7, 11..12], vec!["a", "", "b"])]
    #[case::back_to_back_spans("«a»«b»", vec![2..3, 7..8], vec!["a", "b"])]
    #[case::whitespace_span("«   »", vec![2..5], vec!["   "])]
    // Unmatched opens never emit; an orphan close is a no-op.
    #[case::unterminated_opens("«a«b", vec![], vec![])]
    #[case::orphan_close_then_open("»«", vec![], vec![])]
    #[case::orphan_close_ignored("a»b«c»", vec![6..7], vec!["c"])]
    #[case::extra_close_ignored("«a»b»c", vec![2..3], vec!["a"])]
    fn ranges_index_the_raw_text(
        #[case] body: &str,
        #[case] expected: Vec<Range<usize>>,
        #[case] hits: Vec<&str>,
    ) {
        assert_ranges(highlighter(), body, &expected, &hits);
    }

    #[rstest]
    #[case::multibyte_text_before("café \u{E000}error\u{E001}", vec![9..14], vec!["error"])]
    #[case::multibyte_text_inside("\u{E000}café\u{E001}", vec![3..8], vec!["café"])]
    // Ranges are code-point boundaries, not grapheme boundaries: a marker mid-grapheme splits it.
    #[case::splits_combining_mark("e\u{E000}\u{0301}\u{E001}", vec![4..6], vec!["\u{0301}"])]
    fn ranges_with_default_multibyte_markers(
        #[case] body: &str,
        #[case] expected: Vec<Range<usize>>,
        #[case] hits: Vec<&str>,
    ) {
        assert_ranges(TextHighlighter::default(), body, &expected, &hits);
    }

    #[test]
    fn markers_are_positional_so_swapping_them_inverts_parsing() {
        let swapped = TextHighlighter::with_markers(['»', '«']).unwrap();
        assert_eq!(swapped.as_highlighted("«failed»").ranges().count(), 0);
    }

    /// Alphabet that stresses the byte-offset arithmetic: plain ASCII for clean runs plus the two
    /// visible markers, the private-use defaults, and a spread of multi-byte / zero-width /
    /// combining / RTL code points so marker collisions and mid-grapheme splits are frequent.
    fn nasty_char() -> impl Strategy<Value = char> {
        prop_oneof![
            3 => prop::char::range('a', 'e'),
            2 => Just('«'),
            2 => Just('»'),
            1 => Just('\u{E000}'),
            1 => Just('\u{E001}'),
            1 => Just('é'),
            1 => Just('中'),
            1 => Just('👩'),
            1 => Just('\u{200D}'),
            1 => Just('\u{0301}'),
            1 => Just('\u{200F}'),
            1 => Just(' '),
        ]
    }

    fn nasty_string() -> impl Strategy<Value = String> {
        prop::collection::vec(nasty_char(), 0..24).prop_map(|cs| cs.into_iter().collect())
    }

    /// Two distinct markers drawn from the same nasty alphabet, so the byte width of the markers
    /// varies against the text and can collide with the content.
    fn distinct_markers() -> impl Strategy<Value = [char; 2]> {
        [nasty_char(), nasty_char()].prop_filter("markers must differ", |m| m[0] != m[1])
    }

    /// Two distinct markers, text free of them, and ascending non-overlapping char-index spans over
    /// that text. Drawing the markers from the same alphabet varies their byte width (1–4 bytes), so
    /// the round-trip stresses the `marker.len_utf8()` cursor arithmetic against exact recovery.
    fn markers_clean_and_spans()
    -> impl Strategy<Value = ([char; 2], Vec<char>, Vec<(usize, usize)>)> {
        distinct_markers().prop_flat_map(|markers| {
            let [open, close] = markers;
            prop::collection::vec(
                nasty_char()
                    .prop_filter("clean text holds no marker", move |c| *c != open && *c != close),
                0..12,
            )
            .prop_flat_map(move |chars| {
                let n = chars.len();
                let spans = prop::collection::vec(0usize..=n, 0..8).prop_map(|mut v| {
                    v.sort_unstable();
                    if v.len() % 2 == 1 {
                        v.pop();
                    }
                    v.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).collect::<Vec<_>>()
                });
                (Just(markers), Just(chars), spans)
            })
        })
    }

    /// Wrap each `(start_char, end_char)` span of `chars` in `markers`, returning the marked string
    /// and the byte ranges the highlighted content occupies *within that marked string*.
    fn wrap_spans(
        markers: [char; 2],
        chars: &[char],
        spans: &[(usize, usize)],
    ) -> (String, Vec<Range<usize>>) {
        let [open, close] = markers;
        let mut marked = String::new();
        let mut ranges = Vec::with_capacity(spans.len());
        let mut cursor = 0;
        for &(s, e) in spans {
            marked.extend(chars[cursor..s].iter());
            marked.push(open);
            let start = marked.len();
            marked.extend(chars[s..e].iter());
            let end = marked.len();
            marked.push(close);
            ranges.push(start..end);
            cursor = e;
        }
        marked.extend(chars[cursor..].iter());
        (marked, ranges)
    }

    proptest! {
        // Marker collisions and multibyte-boundary splits are rare per sample, so hammer them with
        // a high case count. The filtered strategies (distinct markers, marker-free clean text)
        // reject frequently, so the reject caps and shrink budget are lifted well above default.
        #![proptest_config(ProptestConfig {
            cases: 2048,
            max_shrink_iters: 8192,
            max_local_rejects: 1 << 16,
            max_global_rejects: 1 << 16,
            ..ProptestConfig::default()
        })]

        // (A) never panics, and (E) every yielded range is an in-bounds, char-boundary, ascending,
        // non-overlapping slice of the raw text — the load-bearing "ranges index the raw text"
        // contract. `str::get(range)` returns Some only for an in-bounds, char-boundary,
        // start <= end range, so it subsumes every bound except the non-overlap check.
        #[test]
        fn ranges_are_valid_ordered_slices_of_the_raw_text(
            markers in distinct_markers(),
            text in nasty_string(),
        ) {
            let h = TextHighlighter::with_markers(markers).unwrap();
            let hl = h.as_highlighted(text.as_str());
            let raw: &str = hl.as_ref();
            let mut prev_end = 0usize;
            for r in hl.ranges() {
                prop_assert!(r.start >= prev_end);
                prop_assert!(raw.get(r.clone()).is_some());
                prev_end = r.end;
            }
        }

        // (B) sanitize removes every marker, (C) is idempotent (a second pass borrows), and
        // (D) equals the input with the marker chars filtered out.
        #[test]
        fn sanitize_is_a_marker_free_idempotent_filter(
            markers in distinct_markers(),
            text in nasty_string(),
        ) {
            let [open, close] = markers;
            let h = TextHighlighter::with_markers(markers).unwrap();
            let clean = h.sanitize(&text);
            prop_assert!(!clean.contains([open, close]));
            let filtered: String = text.chars().filter(|c| *c != open && *c != close).collect();
            prop_assert_eq!(clean.as_ref(), filtered.as_str());
            let again = h.sanitize(&clean);
            prop_assert_eq!(again.as_ref(), clean.as_ref());
            prop_assert!(matches!(again, Cow::Borrowed(_)));
        }

        // (F) round-trip: markers wrapped around chosen spans of marker-free text yield exactly
        // those spans' byte ranges in the marked string, and sanitize strips them to recover the
        // original text. Repeated span indices exercise empty spans (s == e) and adjacent spans.
        #[test]
        fn wrapping_spans_round_trips_through_ranges_and_sanitize(
            (markers, chars, spans) in markers_clean_and_spans(),
        ) {
            let h = TextHighlighter::with_markers(markers).unwrap();
            let (marked, expected) = wrap_spans(markers, &chars, &spans);
            let clean: String = chars.iter().collect();
            let stripped = h.sanitize(&marked);
            prop_assert_eq!(stripped.as_ref(), clean.as_str());
            let ranges = h.as_highlighted(marked.as_str()).ranges().collect::<Vec<_>>();
            prop_assert_eq!(ranges, expected);
        }
    }
}
