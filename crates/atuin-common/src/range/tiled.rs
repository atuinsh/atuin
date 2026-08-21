//! Tile a [`Range`] into fixed-size sub-ranges.
//!
//! [`slice::chunks`] exists for slices but not for [`Range`]s; this fills that gap. A
//! `(start..end).tiled(size)` yields non-overlapping sub-ranges tiling the range in order, the
//! last one clamped to `end`.

use std::iter::FusedIterator;
use std::num::{NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroUsize};
use std::ops::Range;

mod sealed {
    pub trait Sealed {}
    impl Sealed for u8 {}
    impl Sealed for u16 {}
    impl Sealed for u32 {}
    impl Sealed for u64 {}
    impl Sealed for usize {}
}

/// An unsigned index type a [`Range`] can be tiled over. Sealed: implemented for `u8`, `u16`,
/// `u32`, `u64`, and `usize` only.
pub trait TileIdx: sealed::Sealed + Copy + Ord {
    /// The [`NonZero`](std::num::NonZero) flavour of this index type -- `NonZeroU64` for `u64`,
    /// `NonZeroUsize` for `usize`, and so on. This is the generic `NonZero<Self>` Rust can't yet
    /// spell directly (its bound is unstable), recovered as an associated type.
    type NonZero: Copy + std::fmt::Debug;

    /// The number of indices in `self..end`, or `0` when `end <= self`.
    fn distance_to(self, end: Self) -> u64;

    /// `self` advanced by `by` indices, saturating at the type's maximum.
    fn saturating_advance(self, by: u64) -> Self;

    /// `size` as a non-zero tile width, falling back to `1` when it is `0` -- so callers can pass a
    /// plain index without hand-building a `NonZero`.
    fn width_or_one(size: Self) -> Self::NonZero;

    /// A tile `width` widened to `u64` for the internal tiling arithmetic.
    fn width_u64(width: Self::NonZero) -> u64;
}

macro_rules! impl_tile_idx {
    ($($t:ty => $nz:ty),+ $(,)?) => {$(
        impl TileIdx for $t {
            type NonZero = $nz;

            #[allow(
                clippy::cast_lossless,
                reason = "widening an unsigned index (u8..=u64/usize) to u64 is lossless on every \
                          supported platform; `u64::from` does not cover `usize`"
            )]
            fn distance_to(self, end: Self) -> u64 {
                end.saturating_sub(self) as u64
            }

            fn saturating_advance(self, by: u64) -> Self {
                self.saturating_add(<$t>::try_from(by).unwrap_or(<$t>::MAX))
            }

            fn width_or_one(size: Self) -> Self::NonZero {
                <$nz>::new(size).unwrap_or(<$nz>::MIN)
            }

            #[allow(
                clippy::cast_lossless,
                reason = "widening an unsigned index (u8..=u64/usize) to u64 is lossless on every \
                          supported platform; `u64::from` does not cover `usize`"
            )]
            fn width_u64(width: Self::NonZero) -> u64 {
                width.get() as u64
            }
        }
    )+};
}

impl_tile_idx!(
    u8 => NonZeroU8,
    u16 => NonZeroU16,
    u32 => NonZeroU32,
    u64 => NonZeroU64,
    usize => NonZeroUsize,
);

/// Iterator of fixed-`size` sub-ranges tiling a [`Range`], the last clamped to the range end.
///
/// Build one with [`RangeTiledExt::tiled`].
#[derive(Debug, Copy, Clone)]
pub struct Tiled<T: TileIdx> {
    cursor: T,
    end: T,
    size: T::NonZero,
}

impl<T: TileIdx> Tiled<T> {
    /// The index the next tile starts at.
    #[must_use]
    pub fn start(&self) -> T {
        self.cursor
    }

    /// One past the last index the run covers.
    #[must_use]
    pub fn end(&self) -> T {
        self.end
    }

    /// The tile width, as the [`NonZero`](std::num::NonZero) of the index type.
    #[must_use]
    pub fn size(&self) -> T::NonZero {
        self.size
    }

    /// The number of *indices* still to be covered.
    ///
    /// Note this is not the number of tiles, which is what [`ExactSizeIterator::len`] is.
    #[must_use]
    pub fn index_len(&self) -> u64 {
        self.cursor.distance_to(self.end)
    }
}

impl<T: TileIdx> Iterator for Tiled<T> {
    type Item = Range<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.end {
            return None;
        }
        let stop = self.cursor.saturating_advance(T::width_u64(self.size)).min(self.end);
        let tile = self.cursor..stop;
        self.cursor = stop;
        Some(tile)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = usize::try_from(self.index_len().div_ceil(T::width_u64(self.size)))
            .unwrap_or(usize::MAX);
        (n, Some(n))
    }
}

impl<T: TileIdx> ExactSizeIterator for Tiled<T> {}
impl<T: TileIdx> FusedIterator for Tiled<T> {}

/// Tile a [`Range`] into fixed-size sub-ranges. See [`Tiled`].
pub trait RangeTiledExt<T: TileIdx> {
    /// Tile `self` into `size`-wide sub-ranges, the last clamped to the end. A `size` of `0` falls
    /// back to `1`, so a misconfigured width degrades to one index per tile rather than panicking.
    fn tiled(self, size: T) -> Tiled<T>;
}

impl<T: TileIdx> RangeTiledExt<T> for Range<T> {
    fn tiled(self, size: T) -> Tiled<T> {
        Tiled {
            cursor: self.start,
            end: self.end,
            size: T::width_or_one(size),
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::clamped_tail(0..5, 2, vec![0..2, 2..4, 4..5])]
    #[case::exact_multiple(0..4, 2, vec![0..2, 2..4])]
    #[case::size_exceeds_range(0..3, 10, vec![0..3])]
    #[case::offset_start(3..9, 4, vec![3..7, 7..9])]
    #[case::empty_range(5..5, 4, vec![])]
    #[case::zero_width_falls_back_to_one(0..3, 0, vec![0..1, 1..2, 2..3])]
    fn tiles_a_range_with_a_clamped_tail(
        #[case] range: Range<u64>,
        #[case] size: u64,
        #[case] expected: Vec<Range<u64>>,
    ) {
        let plan = range.tiled(size);
        // `len()` (from `size_hint`) must agree with the tiles actually produced.
        assert_eq!(plan.len(), expected.len());
        assert_eq!(plan.collect::<Vec<_>>(), expected);
    }

    #[rstest]
    #[case(3..9, 6)]
    #[case(0..10, 10)]
    #[case(5..5, 0)]
    fn index_len_counts_indices_not_tiles(#[case] range: Range<u64>, #[case] expected: u64) {
        assert_eq!(range.tiled(4).index_len(), expected);
    }

    #[test]
    fn exposes_its_bounds_and_stride() {
        let plan = (3u64..9).tiled(4);
        assert_eq!(plan.start(), 3);
        assert_eq!(plan.end(), 9);
        assert_eq!(plan.size(), NonZeroU64::new(4).unwrap());
    }

    #[test]
    fn is_generic_over_the_index_type() {
        let tiles: Vec<_> = (0usize..5).tiled(2).collect();
        assert_eq!(tiles, vec![0..2, 2..4, 4..5]);
    }
}
