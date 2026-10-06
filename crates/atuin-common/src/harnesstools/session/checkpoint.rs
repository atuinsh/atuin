//! Where a session resumes.

use xxhash_rust::xxh3::xxh3_64;

use crate::io::Line;

/// Where a session resumes: just past the item a consumer last took, with a digest of that item so
/// a source rewritten under the position is noticed.
///
/// Two checkpoints are equal when they name the same position and item: which read of the source
/// reached it ([`generation`](Self::generation)) is not part of that.
#[derive(Debug, Clone, Copy)]
pub struct Checkpoint {
    /// The item's position in its harness's own terms: a transcript line's end offset, a row's
    /// `seq`.
    pub at: u64,
    /// xxh3 of the item: a transcript line's bytes, a row's id. Stored checkpoints are compared
    /// against it, so changing the hash reads every session from its start once.
    pub digest: u64,
    /// Which read of the source the item was read in, as a session's own stream counts them: one
    /// more each time it started over from the source's start under the same stream (a
    /// transcript replaced, or cut short: [`Line::generation`]); 0 for the stream's first, and
    /// for a checkpoint not read from a stream (a stored one). Never stored: it tells a consumer
    /// following the stream to start over what it keeps of the session, which positions can't
    /// (the new content's first line may end past where the old content was read to).
    pub generation: u32,
}

impl PartialEq for Checkpoint {
    fn eq(&self, other: &Self) -> bool {
        (self.at, self.digest) == (other.at, other.digest)
    }
}

impl Eq for Checkpoint {}

impl Checkpoint {
    /// A checkpoint just past `item`, found at `at`.
    #[must_use]
    pub fn new(at: u64, item: &[u8]) -> Self {
        Self {
            at,
            digest: xxh3_64(item),
            generation: 0,
        }
    }

    /// A checkpoint just past `line`, in the read it came from.
    #[must_use]
    pub fn after(line: &Line) -> Self {
        Self {
            generation: line.generation,
            ..Self::new(line.end, &line.bytes)
        }
    }

    /// This checkpoint, from a read that started its source over from the start (`over`) rather
    /// than read on from the checkpoint it was handed, which no longer named its item: a read
    /// later ([`generation`](Self::generation)), so a consumer can tell it from a resume whatever
    /// the position.
    #[must_use]
    pub const fn started_over(self, over: bool) -> Self {
        Self {
            generation: self.generation.wrapping_add(over as u32),
            ..self
        }
    }

    /// Whether `item`, found at this checkpoint's position, is the one it was taken after.
    #[must_use]
    pub fn names(&self, item: &[u8]) -> bool {
        xxh3_64(item) == self.digest
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn the_digest_of_an_item_never_changes() {
        assert_eq!(Checkpoint::new(7, br#"{"type":"user"}"#).digest, 0xc1bb_1810_dd7a_177b);
    }
}
