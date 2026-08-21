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

#[doc(hidden)]
pub trait TileIdx: sealed::Sealed + Copy + Ord {
    type NonZero: Copy + std::fmt::Debug;

    /// `self` advanced by a non-zero tile width, saturating at the type's maximum.
    fn saturating_advance(self, by: Self::NonZero) -> Self;

    /// `size` as a non-zero tile width, falling back to `1` when it is `0`.
    fn width_or_one(size: Self) -> Self::NonZero;
}

macro_rules! impl_tile_idx {
    ($($t:ty => $nz:ty),+ $(,)?) => {$(
        impl TileIdx for $t {
            type NonZero = $nz;

            fn saturating_advance(self, by: Self::NonZero) -> Self {
                self.saturating_add(by.get())
            }

            fn width_or_one(size: Self) -> Self::NonZero {
                <$nz>::new(size).unwrap_or(<$nz>::MIN)
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
///
/// ```
/// use atuin_common::range::RangeTiledExt;
///
/// // Tile 0..10 into width-4 chunks; the final chunk is clamped to the end.
/// let tiles: Vec<_> = (0u64..10).tiled(4).collect();
/// assert_eq!(tiles, vec![0..4, 4..8, 8..10]);
/// ```
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
}

impl<T: TileIdx> Iterator for Tiled<T> {
    type Item = Range<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.end {
            return None;
        }
        let stop = self.cursor.saturating_advance(self.size).min(self.end);
        let tile = self.cursor..stop;
        self.cursor = stop;
        Some(tile)
    }
}

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
        assert_eq!(range.tiled(size).collect::<Vec<_>>(), expected);
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
