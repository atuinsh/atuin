//! Utilities for working with [`Range`]s.

use std::ops::Range;

mod chunks;
mod clamped;
mod numbering;
pub mod py_style;
mod windows;

pub use chunks::{ChunkInt, Chunks};
pub use clamped::{ClampInt, Clamped};
pub use numbering::KeptEnds;
pub use py_style::PyStyleIdxRange;
pub use windows::{context_windows, merge_ranges};

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
