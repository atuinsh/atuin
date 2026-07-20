//! Extension trait for padding a string to a budget with an alignment.

use std::borrow::Cow;

use super::Measure;

/// Which side to pad toward when the string is shorter than the budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    /// Content flush to the start, padding on the end (left-aligned).
    Start,
    /// Content centered, padding split across both sides.
    Center,
    /// Content flush to the end, padding on the start (right-aligned).
    End,
}

pub trait AlignExt: AsRef<str> {
    /// Pad `self` with spaces to fill `budget`, distributing the padding per `align`.
    ///
    /// Does not truncate.
    fn pad_to(&self, budget: Measure, align: Alignment) -> Cow<'_, str> {
        let s = self.as_ref();
        let pad = budget.amount().saturating_sub(budget.cost(s));
        if pad == 0 {
            return Cow::Borrowed(s);
        }
        // Split the padding across the two sides per `align`.
        let (left, right) = match align {
            Alignment::Start => (0, pad),
            Alignment::End => (pad, 0),
            Alignment::Center => (pad / 2, pad - pad / 2),
        };
        let mut out = String::with_capacity(s.len() + pad);
        for _ in 0..left {
            out.push(' ');
        }
        out.push_str(s);
        for _ in 0..right {
            out.push(' ');
        }
        Cow::Owned(out)
    }
}

impl<T: AsRef<str>> AlignExt for T {}

#[cfg(test)]
mod tests {
    use super::{AlignExt, Alignment};
    use crate::string::Measure;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    #[rstest]
    #[case::start_pads_on_the_end("hi", Measure::Columns(5), Alignment::Start, "hi   ")]
    #[case::end_pads_on_the_start("hi", Measure::Columns(5), Alignment::End, "   hi")]
    #[case::center_splits_evenly("hi", Measure::Columns(6), Alignment::Center, "  hi  ")]
    #[case::center_odd_extra_on_right("hi", Measure::Columns(5), Alignment::Center, " hi  ")]
    #[case::exact_fit_unchanged("hello", Measure::Columns(5), Alignment::Start, "hello")]
    #[case::empty_pads("", Measure::Columns(3), Alignment::End, "   ")]
    #[case::wide_glyph_pads_by_display_columns("世", Measure::Columns(3), Alignment::Start, "世 ")]
    #[case::pads_by_bytes_under_byte_budget("世", Measure::Bytes(4), Alignment::Start, "世 ")]
    #[case::too_wide_is_never_truncated(
        "hello world",
        Measure::Columns(3),
        Alignment::Start,
        "hello world"
    )]
    fn pads_per_table(
        #[case] input: &str,
        #[case] budget: Measure,
        #[case] align: Alignment,
        #[case] expected: &str,
    ) {
        assert_eq!(input.pad_to(budget, align).as_ref(), expected);
    }

    #[test]
    fn borrows_only_when_no_padding_needed() {
        // Exact fit: no padding -> borrowed.
        assert!(matches!(
            "hello".pad_to(Measure::Columns(5), Alignment::Start),
            std::borrow::Cow::Borrowed(_)
        ));
        // Too wide: not truncated, no padding -> borrowed.
        assert!(matches!(
            "hello world".pad_to(Measure::Columns(3), Alignment::Start),
            std::borrow::Cow::Borrowed(_)
        ));
        // Padding needed -> owned.
        assert!(matches!(
            "hi".pad_to(Measure::Columns(5), Alignment::Start),
            std::borrow::Cow::Owned(_)
        ));
    }
}
