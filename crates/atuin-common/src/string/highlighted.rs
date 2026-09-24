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
use std::fmt::Write as _;
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

        Self::with_markers([DEFAULT_MATCH_OPEN, DEFAULT_MATCH_CLOSE])
            .expect("the default markers are distinct")
    }
}

impl TextHighlighter {
    /// Create the highlighter with the given markers.
    pub fn with_markers(markers: [char; 2]) -> Result<Self, NewTextHighlighterError> {
        if markers[0] == markers[1] {
            return Err(NewTextHighlighterError::IdenticalMarkers(markers));
        }

        Ok(Self {
            open: markers[0],
            close: markers[1],
        })
    }

    /// The `[open, close]` markers this highlighter wraps matches in.
    #[must_use]
    pub fn markers(self) -> [char; 2] {
        [self.open, self.close]
    }

    /// Take a string which may or may not contain highlight markers and strip it of said highlight
    /// markers.
    ///
    /// TODO(markovejnovic): Could be more optimized for mutable strings and sanitize them in-place.
    #[must_use]
    pub fn sanitize(self, text: &str) -> Cow<'_, str> {
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
/// `Display` implementations come in the form of [`Self::display_plain`], [`Self::display_subs`]
/// and [`Self::display_raw`].
#[derive(Clone, Copy)]
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

    /// Swap the underlying string type, keeping the highlighter (e.g. `line.map(str::to_owned)`).
    pub fn map<T>(self, f: impl FnOnce(S) -> T) -> HighlightedText<T> {
        HighlightedText {
            data: f(self.data),
            highlighter: self.highlighter,
        }
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

    /// Walk the text as a stream of marker-free chunks: [`Piece::Text`] for unhighlighted runs and
    /// [`Piece::Match`] for each highlighted span.
    ///
    /// Concatenating every chunk yields the same string as [`Self::display_plain`], and the
    /// [`Piece::Match`] chunks are exactly the spans [`Self::ranges`] reports. Empty chunks are
    /// skipped.
    pub fn pieces(&self) -> impl Iterator<Item = Piece<'_>> + '_ {
        Pieces {
            text: self.data.as_ref(),
            open: self.highlighter.open,
            close: self.highlighter.close,
            ranges: HighlightedRanges {
                src: self,
                cursor: 0,
                start: None,
            },
            cursor: 0,
            gap: "",
            pending: None,
            tail_done: false,
        }
    }

    /// Whether any highlighted span is present.
    pub fn has_match(&self) -> bool {
        self.ranges().next().is_some()
    }

    /// Each line as its own highlighted text. A match never spans a newline, so every span stays
    /// within one line.
    pub fn lines(&self) -> impl Iterator<Item = HighlightedText<&str>> + '_ {
        self.data.as_ref().lines().map(|line| self.highlighter.as_highlighted(line))
    }

    /// The marker-free text together with the byte range of every match within *that* text --
    /// unlike [`Self::ranges`], whose offsets index the raw, marker-bearing string.
    pub fn to_plain(&self) -> Plain<'_> {
        let raw = self.data.as_ref();
        if !raw.contains(self.highlighter.markers()) {
            return Plain {
                text: Cow::Borrowed(raw),
                ranges: Vec::new(),
            };
        }
        let (text, ranges) =
            self.pieces().fold((String::new(), Vec::new()), |(mut plain, mut ranges), piece| {
                let start = plain.len();
                match piece {
                    Piece::Text(text) => plain.push_str(text),
                    Piece::Match(text) => {
                        plain.push_str(text);
                        ranges.push(start..plain.len());
                    }
                }
                (plain, ranges)
            });
        Plain {
            text: Cow::Owned(text),
            ranges,
        }
    }

    /// `Display` the highlighted text, stripping away the highlight markers.
    pub fn display_plain(&self) -> impl fmt::Display + '_ {
        DisplayPlain(self)
    }

    /// `Display` the highlighted text, replacing highlighted markers with `subs`.
    pub fn display_subs(&self, subs: [char; 2]) -> impl fmt::Display + '_ {
        DisplaySubs { src: self, subs }
    }

    /// `Display` the highlighted text as-is, including markers.
    pub fn display_raw(&self) -> impl fmt::Display + '_ {
        DisplayRaw(self)
    }
}

impl<'a> HighlightedText<&'a str> {
    /// The marker-free text, borrowing the source when it holds no markers.
    ///
    /// The lightweight counterpart to [`Self::to_plain`], which also reports each match's range.
    #[must_use]
    pub fn plain(self) -> Cow<'a, str> {
        self.highlighter.sanitize(self.data)
    }
}

/// The marker-free view of a [`HighlightedText`], from [`HighlightedText::to_plain`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plain<'a> {
    /// Borrowed from the source when it holds no markers.
    pub text: Cow<'a, str>,
    /// Byte ranges of the matches within `text`.
    pub ranges: Vec<Range<usize>>,
}

/// One marker-free chunk of a [`HighlightedText`], produced by [`HighlightedText::pieces`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece<'a> {
    /// A run of text outside any highlight.
    Text(&'a str),
    /// The content of one highlighted match.
    Match(&'a str),
}

struct Pieces<'a, S> {
    text: &'a str,
    open: char,
    close: char,
    ranges: HighlightedRanges<'a, S>,
    /// Start of the not-yet-emitted region of `text`, just past the previous match's close marker.
    cursor: usize,
    /// The gap before `pending`, drained into marker-free [`Piece::Text`] runs before it is emitted.
    gap: &'a str,
    /// A match whose content is emitted once `gap` is drained.
    pending: Option<&'a str>,
    /// Whether the trailing gap after the final match has been queued into `gap`.
    tail_done: bool,
}

impl<'a, S: AsRef<str>> Iterator for Pieces<'a, S> {
    type Item = Piece<'a>;

    fn next(&mut self) -> Option<Piece<'a>> {
        loop {
            // Drain the current gap into marker-free text runs, dropping any stray markers.
            if !self.gap.is_empty() {
                let Some(at) = self.gap.find([self.open, self.close]) else {
                    return Some(Piece::Text(std::mem::take(&mut self.gap)));
                };
                let seg = &self.gap[..at];
                let marker_len = if self.gap[at..].starts_with(self.open) {
                    self.open.len_utf8()
                } else {
                    self.close.len_utf8()
                };
                self.gap = &self.gap[at + marker_len..];
                if !seg.is_empty() {
                    return Some(Piece::Text(seg));
                }
                continue;
            }

            // Gap drained: emit the pending match, then advance to the next span.
            if let Some(matched) = self.pending.take() {
                if !matched.is_empty() {
                    return Some(Piece::Match(matched));
                }
                continue;
            }

            match self.ranges.next() {
                Some(r) => {
                    let open_start = (r.start - self.open.len_utf8()).max(self.cursor);
                    let next_cursor = r.end + self.close.len_utf8();
                    self.gap = &self.text[self.cursor..open_start];
                    self.pending = Some(&self.text[r]);
                    self.cursor = next_cursor;
                }
                None if !self.tail_done => {
                    self.tail_done = true;
                    self.gap = &self.text[self.cursor..];
                }
                None => return None,
            }
        }
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
        for c in self.src.data.as_ref().chars() {
            if c == h.open {
                f.write_char(open_sub)?;
            } else if c == h.close {
                f.write_char(close_sub)?;
            } else {
                f.write_char(c)?;
            }
        }
        Ok(())
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

    use super::{
        HighlightedStr, HighlightedString, HighlightedText, NewTextHighlighterError,
        TextHighlighter,
    };

    #[derive(Clone, PartialEq, Eq, Hash, prost::Message)]
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

    impl<'a> TryFrom<&'a HighlightedTextProto> for HighlightedStr<'a> {
        type Error = FromHighlightedTextProtoError;

        fn try_from(value: &'a HighlightedTextProto) -> Result<Self, Self::Error> {
            let open = char::from_u32(value.open)
                .ok_or(FromHighlightedTextProtoError::InvalidMarker(value.open))?;
            let close = char::from_u32(value.close)
                .ok_or(FromHighlightedTextProtoError::InvalidMarker(value.close))?;
            Ok(TextHighlighter::with_markers([open, close])?.as_highlighted(value.raw.as_str()))
        }
    }

    impl TryFrom<HighlightedTextProto> for HighlightedString {
        type Error = FromHighlightedTextProtoError;

        fn try_from(value: HighlightedTextProto) -> Result<Self, Self::Error> {
            Ok(HighlightedStr::try_from(&value)?.map(str::to_owned))
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

    // The `display_*` views are context-free per-marker transforms: every marker char is acted on
    // whether or not it pairs up, so orphan and nested markers get substituted/stripped just like
    // matched ones. This is deliberately unlike `ranges()`, which reports only matched pairs — so
    // no marker (a private-use codepoint by default) can ever leak into the rendered output.
    #[rstest]
    #[case::clean("clean text", "clean text")]
    #[case::single_span("the build «failed» now", "the build [failed] now")]
    #[case::two_spans("«a» b «c»", "[a] b [c]")]
    #[case::orphan_open("«a", "[a")]
    #[case::orphan_close("a»", "a]")]
    #[case::nested_open("«a«b»", "[a[b]")]
    #[case::doubled("««x»»", "[[x]]")]
    #[case::empty_span("«»", "[]")]
    #[case::multibyte_text("café «error»", "café [error]")]
    fn display_subs_substitutes_every_marker(#[case] body: &str, #[case] expected: &str) {
        let out = highlighter().as_highlighted(body).display_subs(['[', ']']).to_string();
        assert_eq!(out, expected);
    }

    #[rstest]
    #[case::clean("clean text", "clean text")]
    #[case::single_span("the build «failed» now", "the build failed now")]
    #[case::orphan_and_nested("«a«b»", "ab")]
    #[case::doubled("««x»»", "x")]
    fn display_plain_strips_every_marker(#[case] body: &str, #[case] expected: &str) {
        let out = highlighter().as_highlighted(body).display_plain().to_string();
        assert_eq!(out, expected);
    }

    #[rstest]
    #[case::clean("clean text", "clean text", vec![])]
    #[case::span_between_text("the «build» failed", "the build failed", vec![4..9])]
    #[case::back_to_back("«a»«b»", "ab", vec![0..1, 1..2])]
    #[case::multibyte_before_span("héllo «wörld»", "héllo wörld", vec![7..13])]
    fn to_plain_indexes_the_plain_text(
        #[case] body: &str,
        #[case] plain: &str,
        #[case] ranges: Vec<Range<usize>>,
    ) {
        let hl = highlighter().as_highlighted(body);
        let got = hl.to_plain();
        assert_eq!(got.text, plain);
        assert_eq!(got.ranges, ranges);
        assert_eq!(matches!(got.text, Cow::Borrowed(_)), ranges.is_empty());
        for r in got.ranges {
            assert!(got.text.is_char_boundary(r.start) && got.text.is_char_boundary(r.end));
        }
    }

    #[test]
    fn display_raw_is_verbatim() {
        let body = "«a«b»";
        assert_eq!(highlighter().as_highlighted(body).display_raw().to_string(), body);
    }

    #[rstest]
    #[case::clean("clean text", vec![(false, "clean text")])]
    #[case::span_between_text("the «build» failed", vec![(false, "the "), (true, "build"), (false, " failed")])]
    #[case::back_to_back("«a»«b»", vec![(true, "a"), (true, "b")])]
    #[case::no_markers("", vec![])]
    #[case::empty_span_skipped("«»", vec![])]
    // Markers are stripped context-free (like `display_plain`), but only paired spans are matches
    // (like `ranges`): the abandoned outer open's content is plain text, only the inner span matches.
    #[case::nested_open_outer_is_text("«a«b»", vec![(false, "a"), (true, "b")])]
    #[case::orphan_close_stripped("»a«c»", vec![(false, "a"), (true, "c")])]
    fn pieces_split_text_and_matches(#[case] body: &str, #[case] expected: Vec<(bool, &str)>) {
        let hl = highlighter().as_highlighted(body);
        let got: Vec<(bool, &str)> = hl
            .pieces()
            .map(|p| match p {
                Piece::Text(t) => (false, t),
                Piece::Match(m) => (true, m),
            })
            .collect();
        assert_eq!(got, expected);
    }

    #[rstest]
    #[case::single("a «b»", vec![("a b", true)])]
    #[case::mixed("x\n«hit» here\ny\n", vec![("x", false), ("hit here", true), ("y", false)])]
    #[case::blank_lines("\n\n«a»", vec![("", false), ("", false), ("a", true)])]
    #[case::empty("", vec![])]
    fn lines_keep_each_lines_own_markers(#[case] body: &str, #[case] expected: Vec<(&str, bool)>) {
        let hl = highlighter().as_highlighted(body);
        let got: Vec<(String, bool)> =
            hl.lines().map(|l| (l.display_plain().to_string(), l.has_match())).collect();
        let expected: Vec<(String, bool)> =
            expected.into_iter().map(|(t, m)| (t.to_owned(), m)).collect();
        assert_eq!(got, expected);
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

        // (G) `pieces()` strips exactly the markers `display_plain` does, and its `Match` chunks are
        // exactly the non-empty spans `ranges()` reports.
        #[test]
        fn pieces_reconstruct_plain_and_report_matches(
            markers in distinct_markers(),
            text in nasty_string(),
        ) {
            let h = TextHighlighter::with_markers(markers).unwrap();
            let hl = h.as_highlighted(text.as_str());

            let mut plain = String::new();
            let mut matches: Vec<&str> = Vec::new();
            for piece in hl.pieces() {
                match piece {
                    Piece::Text(t) => plain.push_str(t),
                    Piece::Match(m) => {
                        plain.push_str(m);
                        matches.push(m);
                    }
                }
            }
            prop_assert_eq!(plain, hl.display_plain().to_string());

            let raw: &str = hl.as_ref();
            let want: Vec<&str> = hl.ranges().map(|r| &raw[r]).filter(|s| !s.is_empty()).collect();
            prop_assert_eq!(matches, want);
        }
    }

    #[cfg(feature = "proto")]
    proptest! {
        /// A highlighted text survives the round-trip through its proto -- same markers, same raw --
        /// and `plain()` (via `sanitize`'s `replace`) strips exactly what `display_plain` (via
        /// `split`) does.
        #[test]
        fn proto_round_trips_and_plain_agrees_with_display(
            markers in distinct_markers(),
            text in nasty_string(),
        ) {
            let hl = TextHighlighter::with_markers(markers).unwrap().as_highlighted(text.as_str());
            let proto = HighlightedTextProto::from(&hl);

            let borrowed = HighlightedStr::try_from(&proto).unwrap();
            prop_assert_eq!(borrowed.as_ref(), text.as_str());
            prop_assert_eq!(borrowed.markers(), markers);
            prop_assert_eq!(borrowed.plain().into_owned(), hl.display_plain().to_string());

            let owned = HighlightedString::try_from(proto).unwrap();
            prop_assert_eq!(owned.as_ref(), text.as_str());
            prop_assert_eq!(owned.markers(), markers);
        }
    }

    #[cfg(feature = "proto")]
    #[rstest]
    #[case::open_surrogate(0xD800, u32::from('»'))]
    #[case::close_above_char_max(u32::from('«'), 0x0011_0000)]
    fn proto_rejects_non_char_markers(#[case] open: u32, #[case] close: u32) {
        let proto = HighlightedTextProto {
            open,
            close,
            raw: "hi".to_owned(),
        };
        assert!(matches!(
            HighlightedStr::try_from(&proto),
            Err(FromHighlightedTextProtoError::InvalidMarker(_))
        ));
    }

    #[cfg(feature = "proto")]
    #[rstest]
    fn proto_rejects_identical_markers() {
        let proto = HighlightedTextProto {
            open: u32::from('x'),
            close: u32::from('x'),
            raw: "x".to_owned(),
        };
        assert!(matches!(
            HighlightedStr::try_from(&proto),
            Err(FromHighlightedTextProtoError::Markers(NewTextHighlighterError::IdenticalMarkers(
                _
            )))
        ));
    }
}
