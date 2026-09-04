//! Utilities for working with [`Range`]s.

use std::ops::Range;

mod chunks;
mod py_style;

pub use chunks::{ChunkInt, Chunks};
pub use py_style::PyStyleIdxRange;

/// Chunk a [`Range`] into fixed-size sub-ranges. See [`Chunks`].
pub trait RangeExt<T: ChunkInt> {
    /// Chunk `self` into `size`-wide sub-ranges, the last clamped to the end. A `size` of `0` falls
    /// back to `1`, so a misconfigured width degrades to one per chunk rather than panicking.
    fn chunks(self, size: T) -> Chunks<T>;
}

impl<T: ChunkInt> RangeExt<T> for Range<T> {
    fn chunks(self, size: T) -> Chunks<T> {
        Chunks::new(self, size)
    }
}
