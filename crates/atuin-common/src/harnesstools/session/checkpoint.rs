//! Where a session resumes.

use xxhash_rust::xxh3::xxh3_64;

/// Where a session resumes: just past the item a consumer last took, with a digest of that item so
/// a source rewritten under the position is noticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checkpoint {
    /// The item's position in its harness's own terms: a transcript line's end offset, a row's
    /// `seq`.
    pub at: u64,
    /// xxh3 of the item: a transcript line's bytes, a row's id. Stored checkpoints are compared
    /// against it, so changing the hash reads every session from its start once.
    pub digest: u64,
}

impl Checkpoint {
    /// A checkpoint just past `item`, found at `at`.
    #[must_use]
    pub fn new(at: u64, item: &[u8]) -> Self {
        Self {
            at,
            digest: xxh3_64(item),
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
