//! Extension trait for truncating a string to a budget (display columns or
//! bytes) with an ellipsis.

use std::borrow::Cow;
use std::fmt;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Which side of the string to elide when it does not fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pos {
    /// Keep the tail, elide the head: `…orld`.
    Start,
    /// Keep both ends, elide the middle: `he…ld`.
    Middle,
    /// Keep the head, elide the tail: `hello…`.
    End,
}

/// The marker spliced in where content was dropped: [`Indicator::ASCII`]
/// (`...`), [`Indicator::UNICODE`] (`…`), or any custom string via
/// [`Indicator::new`] (e.g. `[output truncated]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::AsRef)]
pub struct Indicator<'a>(#[as_ref(str)] &'a str);

impl<'a> Indicator<'a> {
    /// Three ASCII periods `...` (3 columns, 3 bytes).
    pub const ASCII: Self = Self("...");
    /// The single Unicode ellipsis `…` (U+2026): 1 column, 3 bytes.
    pub const UNICODE: Self = Self("…");

    /// Wrap an arbitrary marker string.
    pub const fn new(marker: &'a str) -> Self {
        Self(marker)
    }
}

impl Default for Indicator<'_> {
    /// The Unicode ellipsis `…`.
    fn default() -> Self {
        Self::UNICODE
    }
}

/// How much room to truncate into, and the unit it is measured in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    /// A UTF-8 byte budget.
    Bytes(usize),
    /// A display-column budget via `unicode-width` - a double-width glyph such
    /// as `世` or `🦀` counts as two. Use for presentation.
    Columns(usize),
}

impl Budget {
    /// The numeric limit, in this budget's own unit.
    fn amount(self) -> usize {
        match self {
            Budget::Bytes(n) | Budget::Columns(n) => n,
        }
    }

    /// Total cost of `s` in this budget's unit.
    fn cost(self, s: &str) -> usize {
        match self {
            Budget::Bytes(_) => s.len(),
            Budget::Columns(_) => s.width(),
        }
    }
}

pub trait EllipsizeExt: AsRef<str> {
    /// Truncate this string to fit within `budget`, splicing in `indicator` on
    /// `side` if any content had to be dropped. Returns a lazy [`Ellipsized`]
    /// view - no allocation until you ask for an owned string.
    fn ellipsize<'a>(
        &'a self,
        budget: Budget,
        side: Pos,
        indicator: Indicator<'a>,
    ) -> Ellipsized<'a> {
        let s = self.as_ref();
        let amount = budget.amount();
        let len = s.len();
        let cost = |seg: &str| budget.cost(seg);

        if cost(s) <= amount {
            return Ellipsized::contiguous(s, 0);
        }

        // Not enough room for the indicator itself: hard-truncate to a bare, unmarked slice.
        let indicator_cost = cost(indicator.as_ref());
        if amount < indicator_cost {
            return match side {
                Pos::Start => {
                    let start = suffix_boundary(s, amount, cost);
                    Ellipsized::contiguous(&s[start..], start)
                }
                Pos::Middle | Pos::End => {
                    Ellipsized::contiguous(&s[..prefix_boundary(s, amount, cost)], 0)
                }
            };
        }

        let content = amount - indicator_cost;
        match side {
            Pos::End => Ellipsized::spliced(s, prefix_boundary(s, content, cost), len, indicator),
            Pos::Start => Ellipsized::spliced(s, 0, suffix_boundary(s, content, cost), indicator),
            Pos::Middle => {
                let end = prefix_boundary(s, content.div_ceil(2), cost);
                let start = suffix_boundary(s, content / 2, cost).max(end);
                Ellipsized::spliced(s, end, start, indicator)
            }
        }
    }
}

impl<T: AsRef<str>> EllipsizeExt for T {}

/// A budget-truncated view of a source string - either a single contiguous
/// slice (it fit, or there was no room even for the indicator) or a head +
/// indicator + tail with content elided between them.
///
/// Cheap `Copy`. `Display` writes the pieces straight to the formatter.
#[derive(Debug, Clone, Copy)]
pub struct Ellipsized<'a>(Repr<'a>);

#[derive(Debug, Clone, Copy)]
enum Repr<'a> {
    /// The whole result is one contiguous slice of the source; `source_offset`
    /// is the slice's byte offset in the source.
    Contiguous { text: &'a str, source_offset: usize },
    /// A head slice, an indicator, and a tail slice - always with a marker.
    Spliced {
        head: &'a str,
        indicator: Indicator<'a>,
        tail: &'a str,
        /// Byte offset of `tail` in the source, for [`Ellipsized::source_index`].
        tail_source_offset: usize,
    },
}

impl<'a> Ellipsized<'a> {
    fn contiguous(text: &'a str, source_offset: usize) -> Self {
        Self(Repr::Contiguous {
            text,
            source_offset,
        })
    }

    fn spliced(
        source: &'a str,
        head_end: usize,
        tail_start: usize,
        indicator: Indicator<'a>,
    ) -> Self {
        Self(Repr::Spliced {
            head: &source[..head_end],
            tail: &source[tail_start..],
            tail_source_offset: tail_start,
            indicator,
        })
    }

    /// Map a byte offset in the output back to its byte offset in the source,
    /// or `None` if it lands on the spliced indicator.
    pub fn source_index(self, output_byte: usize) -> Option<usize> {
        match self.0 {
            Repr::Contiguous { source_offset, .. } => Some(output_byte + source_offset),
            Repr::Spliced {
                head,
                indicator,
                tail_source_offset,
                ..
            } => {
                let indicator_len = indicator.as_ref().len();
                if output_byte < head.len() {
                    Some(output_byte)
                } else if output_byte >= head.len() + indicator_len {
                    Some(output_byte - head.len() - indicator_len + tail_source_offset)
                } else {
                    None
                }
            }
        }
    }
}

impl fmt::Display for Ellipsized<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Repr::Contiguous { text, .. } => f.write_str(text),
            Repr::Spliced {
                head,
                indicator,
                tail,
                ..
            } => {
                f.write_str(head)?;
                f.write_str(indicator.as_ref())?;
                f.write_str(tail)
            }
        }
    }
}

impl<'a> From<Ellipsized<'a>> for Cow<'a, str> {
    /// Borrowed for a contiguous slice; owned only when an indicator is spliced.
    fn from(ellipsized: Ellipsized<'a>) -> Self {
        match ellipsized.0 {
            Repr::Contiguous { text, .. } => Cow::Borrowed(text),
            Repr::Spliced { .. } => Cow::Owned(ellipsized.to_string()),
        }
    }
}

impl PartialEq<str> for Ellipsized<'_> {
    /// Compares against the concatenated output without allocating.
    fn eq(&self, other: &str) -> bool {
        match self.0 {
            Repr::Contiguous { text, .. } => text == other,
            Repr::Spliced {
                head,
                indicator,
                tail,
                ..
            } => [head, indicator.as_ref(), tail]
                .into_iter()
                .try_fold(other, |rest, piece| rest.strip_prefix(piece))
                .is_some_and(str::is_empty),
        }
    }
}

impl PartialEq<&str> for Ellipsized<'_> {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

/// Total byte length of the longest leading run of `graphemes` whose summed
/// `cost` is at most `max`.
fn fitting_bytes<'a>(
    graphemes: impl Iterator<Item = &'a str>,
    max: usize,
    cost: impl Fn(&str) -> usize,
) -> usize {
    let mut used = 0;
    let mut bytes = 0;
    for seg in graphemes {
        let seg_cost = cost(seg);
        if used + seg_cost > max {
            break;
        }
        used += seg_cost;
        bytes += seg.len();
    }
    bytes
}

/// Byte end index of the longest prefix of `s` whose summed per-grapheme cost is
/// at most `max`.
fn prefix_boundary(s: &str, max: usize, cost: impl Fn(&str) -> usize) -> usize {
    fitting_bytes(s.graphemes(true), max, cost)
}

/// Byte start index of the longest suffix of `s` whose summed per-grapheme cost
/// is at most `max`.
fn suffix_boundary(s: &str, max: usize, cost: impl Fn(&str) -> usize) -> usize {
    s.len() - fitting_bytes(s.graphemes(true).rev(), max, cost)
}

#[cfg(test)]
mod tests {
    use super::{Budget, EllipsizeExt, Indicator, Pos};
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;
    use rstest::rstest;
    use unicode_width::UnicodeWidthStr;

    fn amount(b: Budget) -> usize {
        match b {
            Budget::Bytes(n) => n,
            Budget::Columns(n) => n,
        }
    }

    fn cost(b: Budget, s: &str) -> usize {
        match b {
            Budget::Bytes(_) => s.len(),
            Budget::Columns(_) => UnicodeWidthStr::width(s),
        }
    }

    #[rstest]
    #[case("hello", Budget::Columns(10), Pos::End, Indicator::ASCII, "hello")]
    #[case("hello", Budget::Columns(5), Pos::End, Indicator::ASCII, "hello")]
    #[case(
        "hello world",
        Budget::Columns(8),
        Pos::End,
        Indicator::ASCII,
        "hello..."
    )]
    #[case(
        "hello world",
        Budget::Columns(8),
        Pos::Start,
        Indicator::ASCII,
        "...world"
    )]
    #[case(
        "hello world",
        Budget::Columns(7),
        Pos::Middle,
        Indicator::ASCII,
        "he...ld"
    )]
    #[case(
        "hello world",
        Budget::Columns(6),
        Pos::End,
        Indicator::UNICODE,
        "hello…"
    )]
    #[case(
        "hello world",
        Budget::Columns(6),
        Pos::Start,
        Indicator::UNICODE,
        "…world"
    )]
    #[case("你好世界", Budget::Columns(5), Pos::End, Indicator::ASCII, "你...")]
    #[case("你好世界", Budget::Columns(4), Pos::End, Indicator::ASCII, "...")]
    #[case("你好世界", Budget::Columns(8), Pos::End, Indicator::ASCII, "你好世界")]
    #[case("🐢🦀🐢🦀", Budget::Columns(5), Pos::End, Indicator::UNICODE, "🐢🦀…")]
    #[case(
        "🐢🦀🐢🦀",
        Budget::Columns(8),
        Pos::End,
        Indicator::UNICODE,
        "🐢🦀🐢🦀"
    )]
    #[case("hello", Budget::Columns(2), Pos::End, Indicator::ASCII, "he")]
    #[case("hello", Budget::Columns(2), Pos::Start, Indicator::ASCII, "lo")]
    #[case("hello", Budget::Columns(0), Pos::End, Indicator::ASCII, "")]
    #[case("", Budget::Columns(5), Pos::End, Indicator::ASCII, "")]
    #[case(
        "hello world",
        Budget::Bytes(8),
        Pos::End,
        Indicator::ASCII,
        "hello..."
    )]
    #[case(
        "hello world",
        Budget::Bytes(8),
        Pos::End,
        Indicator::UNICODE,
        "hello…"
    )]
    #[case("café", Budget::Bytes(4), Pos::End, Indicator::ASCII, "c...")]
    #[case("café", Budget::Bytes(5), Pos::End, Indicator::ASCII, "café")]
    #[case("你好", Budget::Bytes(5), Pos::End, Indicator::ASCII, "...")]
    fn truncates_per_table(
        #[case] input: &str,
        #[case] budget: Budget,
        #[case] side: Pos,
        #[case] ellipsis: Indicator<'static>,
        #[case] expected: &str,
    ) {
        assert_eq!(input.ellipsize(budget, side, ellipsis), *expected);
    }

    fn any_pos() -> impl Strategy<Value = Pos> {
        prop_oneof![Just(Pos::Start), Just(Pos::Middle), Just(Pos::End)]
    }

    fn any_indicator() -> impl Strategy<Value = Indicator<'static>> {
        prop_oneof![Just(Indicator::ASCII), Just(Indicator::UNICODE)]
    }

    fn any_budget() -> impl Strategy<Value = Budget> {
        prop_oneof![
            (0usize..40).prop_map(Budget::Bytes),
            (0usize..40).prop_map(Budget::Columns),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]

        #[test]
        fn never_overflows(
            s in r"(?s).*",
            budget in any_budget(),
            side in any_pos(),
            ellipsis in any_indicator(),
        ) {
            let out = s.ellipsize(budget, side, ellipsis).to_string();
            prop_assert!(cost(budget, out.as_ref()) <= amount(budget));
        }

        #[test]
        fn borrowed_when_it_fits(
            s in r"(?s).*",
            budget in any_budget(),
            side in any_pos(),
            ellipsis in any_indicator(),
        ) {
            if cost(budget, &s) <= amount(budget) {
                let result = s.ellipsize(budget, side, ellipsis);
                prop_assert!(matches!(
                    std::borrow::Cow::from(result),
                    std::borrow::Cow::Borrowed(_)
                ));
                prop_assert!(result == s.as_str());
            }
        }

        #[test]
        fn never_grows(
            s in r"(?s).*",
            budget in any_budget(),
            side in any_pos(),
            ellipsis in any_indicator(),
        ) {
            let out = s.ellipsize(budget, side, ellipsis).to_string();
            prop_assert!(cost(budget, out.as_ref()) <= cost(budget, &s));
        }

        #[test]
        fn valid_byte_cut(
            s in r"(?s).*",
            n in 0usize..40,
            side in any_pos(),
            ellipsis in any_indicator(),
        ) {
            let out = s.ellipsize(Budget::Bytes(n), side, ellipsis).to_string();
            prop_assert!(out.len() <= n);
        }

        #[test]
        fn ellipsis_present_when_needed(
            s in r"(?s).*",
            budget in any_budget(),
            side in any_pos(),
            ellipsis in any_indicator(),
        ) {
            let glyph = ellipsis.as_ref();
            let ellipsis_cost = cost(budget, glyph);
            if cost(budget, &s) > amount(budget) && amount(budget) >= ellipsis_cost {
                let out = s.ellipsize(budget, side, ellipsis).to_string();
                prop_assert!(out.contains(glyph));
            }
        }

        #[test]
        fn source_index_round_trips(
            s in r"(?s).*",
            budget in any_budget(),
            side in any_pos(),
            indicator in any_indicator(),
        ) {
            let e = s.ellipsize(budget, side, indicator);
            let out = e.to_string();
            for (i, ch) in out.char_indices() {
                if let Some(j) = e.source_index(i) {
                    prop_assert_eq!(s[j..].chars().next(), Some(ch));
                }
            }
        }
    }

    #[test]
    fn source_index_middle_maps_head_gap_tail() {
        let e = "hello world".ellipsize(Budget::Columns(7), Pos::Middle, Indicator::ASCII);
        assert_eq!(e.to_string(), "he...ld");
        assert_eq!(e.source_index(0), Some(0));
        assert_eq!(e.source_index(1), Some(1));
        assert_eq!(e.source_index(2), None);
        assert_eq!(e.source_index(4), None);
        assert_eq!(e.source_index(5), Some(9));
        assert_eq!(e.source_index(6), Some(10));
    }

    #[test]
    fn source_index_fits_is_identity() {
        let e = "hi".ellipsize(Budget::Columns(10), Pos::Middle, Indicator::ASCII);
        assert!(matches!(
            std::borrow::Cow::from(e),
            std::borrow::Cow::Borrowed(_)
        ));
        assert_eq!(e.source_index(0), Some(0));
        assert_eq!(e.source_index(1), Some(1));
    }

    #[test]
    fn display_writes_without_allocating_via_cow() {
        let e = "hello world".ellipsize(Budget::Columns(8), Pos::End, Indicator::ASCII);
        assert_eq!(e.to_string(), "hello...");
        assert!(matches!(
            std::borrow::Cow::from(e),
            std::borrow::Cow::Owned(_)
        ));

        let fits = "hi".ellipsize(Budget::Columns(8), Pos::End, Indicator::ASCII);
        assert!(matches!(
            std::borrow::Cow::from(fits),
            std::borrow::Cow::Borrowed(_)
        ));
    }
}
