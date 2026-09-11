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
            if marker == self.open {
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
    use super::*;

    /// A highlighter with visible, multibyte markers — distinct from anything the test bodies hold.
    fn highlighter() -> TextHighlighter {
        TextHighlighter::with_markers(['«', '»']).expect("distinct markers")
    }

    #[test]
    fn identical_markers_are_rejected() {
        assert!(matches!(
            TextHighlighter::with_markers(['x', 'x']),
            Err(NewTextHighlighterError::IdenticalMarkers(['x', 'x']))
        ));
    }

    #[test]
    fn sanitize_borrows_when_there_is_nothing_to_strip() {
        assert!(matches!(highlighter().sanitize("clean text"), Cow::Borrowed(_)));
    }

    #[test]
    fn sanitize_strips_every_marker() {
        assert_eq!(highlighter().sanitize("a«b»c«d»"), "abcd");
    }

    #[test]
    fn ranges_index_into_the_sanitized_text() {
        let h = highlighter();
        let body = "the build «failed» now";
        let clean = h.sanitize(body);

        let ranges: Vec<_> = h.parse_highlighted(body).collect();
        assert_eq!(ranges, vec![10..16]);
        assert_eq!(&clean[ranges[0].clone()], "failed");
    }

    #[test]
    fn every_span_gets_its_own_range() {
        let h = highlighter();
        let body = "«a» b «c»";
        let clean = h.sanitize(body);

        let hits: Vec<_> = h.parse_highlighted(body).map(|r| clean[r].to_owned()).collect();
        assert_eq!(hits, vec!["a", "c"]);
    }

    #[test]
    fn a_multibyte_char_before_a_span_does_not_shift_the_range() {
        let h = highlighter();
        let body = "café «error»";
        let clean = h.sanitize(body);

        let ranges: Vec<_> = h.parse_highlighted(body).collect();
        assert_eq!(ranges, vec![6..11]);
        assert_eq!(&clean[ranges[0].clone()], "error");
    }

    #[test]
    fn text_without_markers_yields_no_ranges() {
        assert_eq!(highlighter().parse_highlighted("nothing here").count(), 0);
    }

    #[test]
    fn probe_parse() {
        let h = highlighter();
        for body in ["«a«b»", "«a«b»c»", "««x»»", "«»", "«a«b", "»«", "«»«»", "«a»«»«b»", "«a»«b»", "«   »"] {
            let clean = h.sanitize(body);
            let ranges: Vec<_> = h.parse_highlighted(body).collect();
            let hits: Vec<_> = ranges.iter().map(|r| clean[r.clone()].to_owned()).collect();
            eprintln!("PROBE body={body:?} clean={:?} ranges={ranges:?} hits={hits:?}", &*clean);
        }
        let hd = TextHighlighter::default();
        for body in ["café \u{E000}error\u{E001}", "\u{E000}café\u{E001}", "e\u{E000}\u{0301}\u{E001}"] {
            let clean = hd.sanitize(body);
            let ranges: Vec<_> = hd.parse_highlighted(body).collect();
            eprintln!("PROBED body={body:?} clean={:?} ranges={ranges:?}", &*clean);
        }
    }

    #[tokio::test]
    async fn probe_fts() {
        use sqlx::Row;
        let h = TextHighlighter::default();
        let sqlite = crate::db::sqlite::Sqlite::builder_in_memory().open().await.unwrap();
        let mut conn = sqlite.pool().acquire().await.unwrap();
        crate::db::query::<Sqlite>("create virtual table docs using fts5(body)")
            .execute(&mut *conn).await.unwrap();
        for body in ["error here", "the build failed now", "café error", "the café", "Error and error", "a\u{E000}b error"] {
            crate::db::query::<Sqlite>("insert into docs (body) values (?)")
                .bind_highlightable(h, body).execute(&mut *conn).await.unwrap();
        }
        for (term, _label) in [("error", "error"), ("failed", "failed"), ("café", "café")] {
            let rows = crate::db::query::<Sqlite>(
                "select highlight(docs, 0, ?, ?) as body from docs where docs match ?")
                .bind_highlight(h).bind(term).fetch_all(&mut *conn).await.unwrap();
            for row in rows {
                let marked: String = row.try_get("body").unwrap();
                let clean = h.sanitize(&marked);
                let ranges: Vec<_> = h.parse_highlighted(&marked).collect();
                let hits: Vec<_> = ranges.iter().map(|r| clean[r.clone()].to_owned()).collect();
                eprintln!("FTS term={term:?} marked={marked:?} clean={:?} ranges={ranges:?} hits={hits:?}", &*clean);
            }
        }
    }
}
