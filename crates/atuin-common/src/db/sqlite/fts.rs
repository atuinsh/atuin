//! Sqlite utilities for working with FTS.
//!
//! Please see <https://www.sqlite.org/fts5.html> for more details in the Sqlite implementation of
//! FTS.
//!
//! The most useful utility here is [`TextHighlighter`] which is responsible for creating matching
//! markers when doing full text search.
//!
//! TODO(markovejnovic): Large parts of this logic probably belong somewhere in string.rs.

use std::borrow::Cow;
use std::ops::Range;

use sqlx::query::Query;
use sqlx::{Database, Sqlite};
use thiserror::Error;

/// Responsible for inserting appropriate markers in a [`highlight(text, index, start-marker,
/// stop-marker)`](https://www.sqlite.org/fts5.html#the_highlight_function) statement, and for
/// parsing the marked-up result back into byte ranges.
///
/// There is an extension trait [`TextHighlighterBindExt`] that provides you with the following API:
///
/// ```ignore
/// let highlighter = TextHighlighter::default();
///
/// let rows = db::query::<Sqlite>(
///     "SELECT history_id, highlight(output_fts, 1, ?, ?) AS body, -bm25(output_fts) AS score \
///      FROM output_fts WHERE output_fts MATCH ? ORDER BY score DESC LIMIT ?",
/// )
/// .bind_highlight(&highlighter)
/// .bind(match_expr)
/// .bind(limit);
/// ```
///
/// This will automatically insert two binds for the two `highlight` bind-points.
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

#[derive(Debug, Error)]
pub enum NewTextHighlighterError {
    #[error("identical start and end markers given: {0:?}")]
    IdenticalMarkers([char; 2]),
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

    /// Take a string that you want to insert into a database and sanitize any markers for this
    /// highlighter.
    ///
    /// This is important to call for any piece of raw text you're inserting into FTS that you plan
    /// on highlighting: it guarantees that a stray marker in the source text can't later be mistaken
    /// for one we inserted.
    ///
    /// There is a convenience [`TextHighlighterBindExt::bind_highlightable`] which will invoke this
    /// for you.
    ///
    /// TODO(markovejnovic): Could be more optimized for mutable strings and sanitize them in-place.
    pub fn sanitize<'a>(self, text: &'a str) -> Cow<'a, str> {
        if text.contains([self.open, self.close]) {
            Cow::Owned(text.replace([self.open, self.close], ""))
        } else {
            Cow::Borrowed(text)
        }
    }

    pub fn parse_highlighted<'s>(self, text: &'s str) -> impl Iterator<Item = Range<usize>> {
        HighlightRanges {
            text,
            highlighter: self,
            cursor: 0,
            demarked_cursor: 0,
            start: None,
        }
    }
}

/// Iterator behind [`TextHighlighter::parse_highlighted`].
struct HighlightRanges<'s> {
    /// The text we're iterating through.
    text: &'s str,
    /// The highlighter that was used to highlight the string.
    highlighter: TextHighlighter,

    /// Byte offset into the marked `text` where the next scan resumes.
    ///
    /// This is the index into the original source.
    cursor: usize,

    /// Byte offset into the demarked_cursor text corresponding to `cursor`.
    demarked_cursor: usize,

    /// Where the currently-open span began, in demarked_cursor space, if one is open.
    start: Option<usize>,
}

impl<'s> Iterator for HighlightRanges<'s> {
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Range<usize>> {
        loop {
            let rest = &self.text[self.cursor..];
            let at = rest.find([self.highlighter.open, self.highlighter.close])?;
            let marker = rest[at..].chars().next().expect("`find` landed on a char");

            self.demarked_cursor += at;
            self.cursor += at + marker.len_utf8();
            if marker == self.highlighter.open {
                self.start = Some(self.demarked_cursor);
            } else if let Some(start) = self.start.take() {
                return Some(start..self.demarked_cursor);
            }
        }
    }
}

pub trait TextHighlighterBindExt {
    /// Bind the relevant highlight points into a `highlight(table, index, ?, ?)` sql query.
    ///
    /// See [`TextHighlighter`].
    fn bind_highlight(self, highlighter: TextHighlighter) -> Self;

    /// Sanitize `highlightable` for `highlighter` and bind it. See [`TextHighlighter::sanitize`].
    fn bind_highlightable(self, highlighter: TextHighlighter, highlightable: &str) -> Self;
}

impl<'q> TextHighlighterBindExt for Query<'q, Sqlite, <Sqlite as Database>::Arguments> {
    fn bind_highlight(self, highlighter: TextHighlighter) -> Self {
        // sqlite has no native `char` type; bind the markers as one-char strings.
        self.bind(highlighter.open.to_string()).bind(highlighter.close.to_string())
    }

    fn bind_highlightable(self, highlighter: TextHighlighter, highlightable: &str) -> Self {
        // `into_owned` so the bound value outlives this call rather than borrowing the local `Cow`.
        self.bind(highlighter.sanitize(highlightable).into_owned())
    }
}

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

    /// Assert `parse_highlighted` yields `expected`, and that indexing the sanitized text with each
    /// range recovers `hits`.
    fn assert_parse(h: TextHighlighter, body: &str, expected: &[Range<usize>], hits: &[&str]) {
        let clean = h.sanitize(body);
        let ranges: Vec<Range<usize>> = h.parse_highlighted(body).collect();
        assert_eq!(ranges.as_slice(), expected);
        let got: Vec<&str> = ranges.iter().map(|r| &clean[r.clone()]).collect();
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

    #[rstest]
    #[case::single_span("the build «failed» now", vec![10..16], vec!["failed"])]
    #[case::each_span_its_own_range("«a» b «c»", vec![0..1, 4..5], vec!["a", "c"])]
    #[case::multibyte_prefix_keeps_offsets("café «error»", vec![6..11], vec!["error"])]
    #[case::no_markers("nothing here", vec![], vec![])]
    // A second open before a close overwrites the first, so the outer span is silently dropped.
    #[case::nested_open_keeps_inner("«a«b»", vec![1..2], vec!["b"])]
    #[case::nested_pair_keeps_inner("«a«b»c»", vec![1..2], vec!["b"])]
    #[case::doubled_markers_collapse("««x»»", vec![0..1], vec!["x"])]
    // An open immediately closed yields a valid zero-width range.
    #[case::empty_span("«»", vec![0..0], vec![""])]
    #[case::adjacent_empty_spans("«»«»", vec![0..0, 0..0], vec!["", ""])]
    #[case::empty_span_between_real("«a»«»«b»", vec![0..1, 1..1, 1..2], vec!["a", "", "b"])]
    #[case::back_to_back_spans("«a»«b»", vec![0..1, 1..2], vec!["a", "b"])]
    #[case::whitespace_span("«   »", vec![0..3], vec!["   "])]
    // Unmatched opens never emit; an orphan close is a no-op.
    #[case::unterminated_opens("«a«b", vec![], vec![])]
    #[case::orphan_close_then_open("»«", vec![], vec![])]
    #[case::orphan_close_ignored("a»b«c»", vec![2..3], vec!["c"])]
    #[case::extra_close_ignored("«a»b»c", vec![0..1], vec!["a"])]
    fn parse_highlighted_yields_sanitized_ranges(
        #[case] body: &str,
        #[case] expected: Vec<Range<usize>>,
        #[case] hits: Vec<&str>,
    ) {
        assert_parse(highlighter(), body, &expected, &hits);
    }

    #[rstest]
    #[case::multibyte_text_before("café \u{E000}error\u{E001}", vec![6..11], vec!["error"])]
    #[case::multibyte_text_inside("\u{E000}café\u{E001}", vec![0..5], vec!["café"])]
    // Ranges are code-point boundaries, not grapheme boundaries: a marker mid-grapheme splits it.
    #[case::splits_combining_mark("e\u{E000}\u{0301}\u{E001}", vec![1..3], vec!["\u{0301}"])]
    fn parse_with_default_multibyte_markers(
        #[case] body: &str,
        #[case] expected: Vec<Range<usize>>,
        #[case] hits: Vec<&str>,
    ) {
        assert_parse(TextHighlighter::default(), body, &expected, &hits);
    }

    #[test]
    fn markers_are_positional_so_swapping_them_inverts_parsing() {
        let swapped = TextHighlighter::with_markers(['»', '«']).unwrap();
        assert_eq!(swapped.parse_highlighted("«failed»").count(), 0);
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
    fn markers_clean_and_spans() -> impl Strategy<Value = ([char; 2], Vec<char>, Vec<(usize, usize)>)>
    {
        distinct_markers().prop_flat_map(|markers| {
            let [open, close] = markers;
            prop::collection::vec(
                nasty_char().prop_filter("clean text holds no marker", move |c| {
                    *c != open && *c != close
                }),
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
    /// and the byte ranges those spans occupy in the *unmarked* text.
    fn wrap_spans(
        markers: [char; 2],
        chars: &[char],
        spans: &[(usize, usize)],
    ) -> (String, Vec<Range<usize>>) {
        let [open, close] = markers;
        let byte: Vec<usize> = std::iter::once(0)
            .chain(chars.iter().scan(0, |acc, c| {
                *acc += c.len_utf8();
                Some(*acc)
            }))
            .collect();

        let mut marked = String::new();
        let mut ranges = Vec::with_capacity(spans.len());
        let mut cursor = 0;
        for &(s, e) in spans {
            marked.extend(chars[cursor..s].iter());
            marked.push(open);
            marked.extend(chars[s..e].iter());
            marked.push(close);
            ranges.push(byte[s]..byte[e]);
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
        // non-overlapping slice of `sanitize(text)` — the load-bearing "ranges index the sanitized
        // text" contract. `str::get(range)` returns Some only for an in-bounds, char-boundary,
        // start <= end range, so it subsumes every bound except the non-overlap check.
        #[test]
        fn ranges_are_valid_ordered_slices_of_sanitized(
            markers in distinct_markers(),
            text in nasty_string(),
        ) {
            let h = TextHighlighter::with_markers(markers).unwrap();
            let clean = h.sanitize(&text);
            let mut prev_end = 0usize;
            for r in h.parse_highlighted(&text) {
                prop_assert!(r.start >= prev_end);
                prop_assert!(clean.get(r.clone()).is_some());
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

        // (F) round-trip: markers wrapped around chosen spans of marker-free text parse back to
        // exactly those spans, and sanitize strips them to recover the original text. Repeated span
        // indices exercise empty spans (s == e) and adjacent spans (e_i == s_{i+1}).
        #[test]
        fn wrapping_spans_round_trips_through_parse_and_sanitize(
            (markers, chars, spans) in markers_clean_and_spans(),
        ) {
            let h = TextHighlighter::with_markers(markers).unwrap();
            let (marked, expected) = wrap_spans(markers, &chars, &spans);
            let clean: String = chars.iter().collect();
            let stripped = h.sanitize(&marked);
            prop_assert_eq!(stripped.as_ref(), clean.as_str());
            prop_assert_eq!(h.parse_highlighted(&marked).collect::<Vec<_>>(), expected);
        }
    }

    /// Insert `body` (sanitized) into an in-memory FTS5 table, then return the `highlight()` output
    /// for a query matching `term`. `bind_highlight` lives only on `Query`, so this fetches via
    /// `query()` + `Row` rather than the ergonomic `query_scalar`.
    async fn highlight_roundtrip(h: TextHighlighter, body: &str, term: &str) -> String {
        use sqlx::Row;

        let sqlite = crate::db::sqlite::Sqlite::builder_in_memory().open().await.unwrap();
        let mut conn = sqlite.pool().acquire().await.unwrap();

        crate::db::query::<Sqlite>("create virtual table docs using fts5(body)")
            .execute(&mut *conn)
            .await
            .unwrap();
        crate::db::query::<Sqlite>("insert into docs (body) values (?)")
            .bind_highlightable(h, body)
            .execute(&mut *conn)
            .await
            .unwrap();

        let row = crate::db::query::<Sqlite>(
            "select highlight(docs, 0, ?, ?) as body from docs where docs match ?",
        )
        .bind_highlight(h)
        .bind(term)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        row.try_get("body").unwrap()
    }

    #[rstest]
    #[case::start_of_body("error here", "error", vec![0..5], vec!["error"])]
    #[case::middle_of_body("the build failed now", "failed", vec![10..16], vec!["failed"])]
    #[case::multibyte_prefix("café error", "error", vec![6..11], vec!["error"])]
    #[case::multibyte_match("the café", "café", vec![4..9], vec!["café"])]
    // FTS5 folds case, but highlight() wraps the original-case bytes, so both hits keep their case.
    #[case::two_matches_case_preserved("Error and error", "error", vec![0..5, 10..15], vec!["Error", "error"])]
    // A stray default marker in the source is stripped on insert, so it produces no phantom span.
    #[case::stray_source_marker("a\u{E000}b error", "error", vec![3..8], vec!["error"])]
    #[tokio::test]
    async fn highlight_round_trips_through_fts5(
        #[case] body: &str,
        #[case] term: &str,
        #[case] expected: Vec<Range<usize>>,
        #[case] hits: Vec<&str>,
    ) {
        let h = TextHighlighter::default();
        let marked = highlight_roundtrip(h, body, term).await;
        let clean = h.sanitize(&marked);
        // Sanitizing the highlight() output reconstructs the stored (sanitized) body.
        assert_eq!(clean.as_ref(), h.sanitize(body).as_ref());
        let ranges: Vec<_> = h.parse_highlighted(&marked).collect();
        assert_eq!(ranges, expected);
        let got: Vec<_> = ranges.iter().map(|r| clean[r.clone()].to_owned()).collect();
        assert_eq!(got, hits);
    }

    #[tokio::test]
    async fn highlight_of_an_unmatched_column_yields_no_ranges() {
        use sqlx::Row;

        let h = TextHighlighter::default();
        let sqlite = crate::db::sqlite::Sqlite::builder_in_memory().open().await.unwrap();
        let mut conn = sqlite.pool().acquire().await.unwrap();

        crate::db::query::<Sqlite>("create virtual table docs using fts5(title, body)")
            .execute(&mut *conn)
            .await
            .unwrap();
        crate::db::query::<Sqlite>("insert into docs (title, body) values (?, ?)")
            .bind_highlightable(h, "alpha")
            .bind_highlightable(h, "beta gamma")
            .execute(&mut *conn)
            .await
            .unwrap();

        // Match the title column but highlight the body column: highlight() inserts no markers.
        let row = crate::db::query::<Sqlite>(
            "select highlight(docs, 1, ?, ?) as body from docs where docs match ?",
        )
        .bind_highlight(h)
        .bind("alpha")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        let marked: String = row.try_get("body").unwrap();

        assert_eq!(h.parse_highlighted(&marked).count(), 0);
        assert!(matches!(h.sanitize(&marked), Cow::Borrowed(_)));
    }
}
