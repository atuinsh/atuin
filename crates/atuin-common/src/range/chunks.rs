//! Chunk a [`Range`] into fixed-size sub-ranges.
//!
//! [`slice::chunks`] exists for slices but not for [`Range`]s; this fills that gap. A
//! `(start..end).chunks(size)` yields non-overlapping sub-ranges covering the range in order, the
//! last one clamped to `end`.

use std::iter::FusedIterator;
use std::num::NonZero;
use std::ops::Range;

mod sealed {
    pub trait Sealed {}
}

/// An internal-only trait used for advancing chunk iterators.
pub trait ChunkInt: sealed::Sealed + Copy + Ord {
    type NonZero: Copy + std::fmt::Debug;

    /// `self` advanced by a non-zero chunk width, saturating at the type's maximum.
    fn saturating_advance(self, by: Self::NonZero) -> Self;

    /// `size` as a non-zero chunk width, falling back to `1` when it is `0`.
    fn into_nonzero_saturating(size: Self) -> Self::NonZero;
}

macro_rules! impl_chunk_int {
    ($($t:ty),+ $(,)?) => {$(
        impl sealed::Sealed for $t {}

        impl ChunkInt for $t {
            type NonZero = NonZero<$t>;

            fn saturating_advance(self, by: Self::NonZero) -> Self {
                self.saturating_add(by.get())
            }

            fn into_nonzero_saturating(size: Self) -> Self::NonZero {
                NonZero::new(size).unwrap_or(<NonZero<$t>>::MIN)
            }
        }
    )+};
}

impl_chunk_int!(u8, u16, u32, u64, usize);

/// Iterator of fixed-`size` sub-ranges covering a [`Range`], the last clamped to the range end.
///
/// Build one with [`RangeExt::chunks`](super::RangeExt::chunks).
///
/// ```
/// use atuin_common::range::RangeExt;
///
/// // Split 0..10 into width-4 chunks; the final chunk is clamped to the end.
/// let chunks: Vec<_> = (0u64..10).chunks(4).collect();
/// assert_eq!(chunks, vec![0..4, 4..8, 8..10]);
/// ```
#[derive(Debug, Copy, Clone)]
pub struct Chunks<T: ChunkInt> {
    cursor: T,
    end: T,
    size: T::NonZero,
}

impl<T: ChunkInt> Chunks<T> {
    pub(super) fn new(range: Range<T>, size: T) -> Self {
        Self {
            cursor: range.start,
            end: range.end,
            size: T::into_nonzero_saturating(size),
        }
    }

    /// The index the next chunk starts at.
    #[must_use]
    pub fn start(&self) -> T {
        self.cursor
    }

    /// One past the last index the run covers.
    #[must_use]
    pub fn end(&self) -> T {
        self.end
    }

    /// The chunk width, as the [`NonZero`] of the type.
    #[must_use]
    pub fn size(&self) -> T::NonZero {
        self.size
    }
}

impl<T: ChunkInt> Iterator for Chunks<T> {
    type Item = Range<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.end {
            return None;
        }
        let stop = self.cursor.saturating_advance(self.size).min(self.end);
        let chunk = self.cursor..stop;
        self.cursor = stop;
        Some(chunk)
    }
}

impl<T: ChunkInt> FusedIterator for Chunks<T> {}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::range::RangeExt;

    #[rstest]
    #[case::clamped_tail(0..5, 2, vec![0..2, 2..4, 4..5])]
    #[case::exact_multiple(0..4, 2, vec![0..2, 2..4])]
    #[case::size_exceeds_range(0..3, 10, vec![0..3])]
    #[case::offset_start(3..9, 4, vec![3..7, 7..9])]
    #[case::empty_range(5..5, 4, vec![])]
    #[case::zero_width_falls_back_to_one(0..3, 0, vec![0..1, 1..2, 2..3])]
    fn chunks_a_range_with_a_clamped_tail(
        #[case] range: Range<u64>,
        #[case] size: u64,
        #[case] expected: Vec<Range<u64>>,
    ) {
        assert_eq!(range.chunks(size).collect::<Vec<_>>(), expected);
    }

    #[test]
    fn exposes_its_bounds_and_stride() {
        let plan = (3u64..9).chunks(4);
        assert_eq!(plan.start(), 3);
        assert_eq!(plan.end(), 9);
        assert_eq!(plan.size(), NonZero::new(4u64).unwrap());
    }

    #[test]
    fn is_generic_over_the_index_type() {
        let chunks: Vec<_> = (0usize..5).chunks(2).collect();
        assert_eq!(chunks, vec![0..2, 2..4, 4..5]);
    }
}
