//! Utilities for working with index [`Range`](std::ops::Range)s.

mod chunks;

pub use chunks::{ChunkIdx, Chunks, RangeChunksExt};
