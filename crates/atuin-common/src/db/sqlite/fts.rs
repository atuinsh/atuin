//! Sqlite utilities for full-text search.
//!
//! Please see <https://www.sqlite.org/fts5.html> for more details in the Sqlite implementation of
//! FTS.
//!
//! The [`TextHighlighterBindExt`] extension trait binds a [`TextHighlighter`]'s markers into an
//! FTS5 `highlight(table, index, ?, ?)` query and sanitizes text on the way into the index. The
//! highlighter itself lives in [`crate::string::highlighted`].

use sqlx::query::Query;
use sqlx::{Database, Sqlite};

pub use crate::string::highlighted::TextHighlighter;

pub trait TextHighlighterBindExt {
    /// Bind the relevant highlight points into a `highlight(table, index, ?, ?)` sql query.
    ///
    /// See [`TextHighlighter`].
    #[must_use]
    fn bind_highlight(self, highlighter: TextHighlighter) -> Self;

    /// Sanitize `highlightable` for `highlighter` and bind it. See [`TextHighlighter::sanitize`].
    #[must_use]
    fn bind_highlightable(self, highlighter: TextHighlighter, highlightable: &str) -> Self;
}

impl TextHighlighterBindExt for Query<'_, Sqlite, <Sqlite as Database>::Arguments> {
    fn bind_highlight(self, highlighter: TextHighlighter) -> Self {
        // sqlite has no native `char` type; bind the markers as one-char strings.
        let [open, close] = highlighter.markers();
        self.bind(open.to_string()).bind(close.to_string())
    }

    fn bind_highlightable(self, highlighter: TextHighlighter, highlightable: &str) -> Self {
        // `into_owned` so the bound value outlives this call rather than borrowing the local `Cow`.
        self.bind(highlighter.sanitize(highlightable).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::ops::Range;

    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use sqlx::Row;

    use super::*;

    /// Insert `body` (sanitized) into an in-memory FTS5 table, then return the `highlight()` output
    /// for a query matching `term`. `bind_highlight` lives only on `Query`, so this fetches via
    /// `query()` + `Row` rather than the ergonomic `query_scalar`.
    async fn highlight_roundtrip(h: TextHighlighter, body: &str, term: &str) -> String {
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

    // `highlight()` wraps matches with the default markers; `ranges()` then indexes those matches in
    // the raw (still-marked) output.
    #[rstest]
    #[case::start_of_body("error here", "error", vec![3..8], vec!["error"])]
    #[case::middle_of_body("the build failed now", "failed", vec![13..19], vec!["failed"])]
    #[case::multibyte_prefix("café error", "error", vec![9..14], vec!["error"])]
    #[case::multibyte_match("the café", "café", vec![7..12], vec!["café"])]
    // FTS5 folds case, but highlight() wraps the original-case bytes, so both hits keep their case.
    #[case::two_matches_case_preserved("Error and error", "error", vec![3..8, 19..24], vec!["Error", "error"])]
    // A stray default marker in the source is stripped on insert, so it produces no phantom span.
    #[case::stray_source_marker("a\u{E000}b error", "error", vec![6..11], vec!["error"])]
    #[tokio::test]
    async fn highlight_round_trips_through_fts5(
        #[case] body: &str,
        #[case] term: &str,
        #[case] expected: Vec<Range<usize>>,
        #[case] hits: Vec<&str>,
    ) {
        let h = TextHighlighter::default();
        let marked = highlight_roundtrip(h, body, term).await;
        // Sanitizing the highlight() output reconstructs the stored (sanitized) body.
        assert_eq!(h.sanitize(&marked), h.sanitize(body));

        let hl = h.as_highlighted(marked.as_str());
        let ranges: Vec<Range<usize>> = hl.ranges().collect();
        assert_eq!(ranges.as_slice(), expected.as_slice());

        let raw: &str = hl.as_ref();
        let got: Vec<&str> = ranges.iter().map(|r| &raw[r.clone()]).collect();
        assert_eq!(got.as_slice(), hits.as_slice());
    }

    #[tokio::test]
    async fn highlight_of_an_unmatched_column_yields_no_ranges() {
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

        assert_eq!(h.as_highlighted(marked.as_str()).ranges().count(), 0);
        assert!(matches!(h.sanitize(&marked), Cow::Borrowed(_)));
    }
}
