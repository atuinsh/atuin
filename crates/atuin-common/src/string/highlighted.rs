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
}

impl<S: AsRef<str>> AsRef<str> for HighlightedText<S> {
    fn as_ref(&self) -> &str {
        self.data.as_ref()
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

pub type HighlightedString = HighlightedText<String>;
pub type HighlightedStr<'a> = HighlightedText<&'a str>;
pub type HighlightedCowStr<'a> = HighlightedText<Cow<'a, str>>;
