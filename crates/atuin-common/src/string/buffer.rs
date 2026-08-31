use std::fmt;

/// A string buffer limited to a certain length.
#[derive(Clone)]
pub struct BoundedBuffer {
    inner: String,
    limit: usize,
    truncated: bool,
}

impl BoundedBuffer {
    /// Create a new bounded buffer.
    ///
    /// `limit` is the maximum length, in bytes, of the buffer.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            inner: String::new(),
            limit,
            truncated: false,
        }
    }

    /// Get the buffer's maximum length.
    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Inspect the data in the buffer so far.
    #[must_use]
    pub fn data(&self) -> &str {
        &self.inner
    }

    /// Consume the [`BoundedBuffer`] and return the underlying [`String`].
    #[must_use]
    pub fn into_data(self) -> String {
        self.inner
    }

    /// Return whether the buffer has been truncated.
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Replace this buffer with an empty one, and return the old full buffer.
    ///
    /// The buffer's limit is maintained.
    #[must_use]
    pub fn take(&mut self) -> Self {
        Self {
            inner: std::mem::take(&mut self.inner),
            truncated: std::mem::take(&mut self.truncated),
            limit: self.limit,
        }
    }

    /// Clear the buffer.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.truncated = false;
    }
}

/// Implementation of [`fmt::Write`] for [`BoundedBuffer`].
///
/// [`BoundedBuffer`] will return [`fmt::Error`] only when the buffer is full and cannot accept any
/// more data.
impl fmt::Write for BoundedBuffer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.truncated {
            return Err(fmt::Error);
        }

        let available = self.limit - self.inner.len();
        if available >= s.len() {
            self.inner.push_str(s);
            return Ok(());
        }

        self.inner.push_str(&s[..s.floor_char_boundary(available)]);
        self.truncated = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use proptest::prelude::*;
    use rstest::{fixture, rstest};

    use super::BoundedBuffer;

    const LIMIT: usize = 8;

    #[fixture]
    fn buffer(#[default(LIMIT)] limit: usize) -> BoundedBuffer {
        BoundedBuffer::new(limit)
    }

    #[rstest]
    #[case::empty(&[], "")]
    #[case::one_write(&["abc"], "abc")]
    #[case::several_writes(&["ab", "cd", "ef"], "abcdef")]
    #[case::exactly_the_limit(&["abcdefgh"], "abcdefgh")]
    #[case::exactly_the_limit_in_pieces(&["abcd", "efgh"], "abcdefgh")]
    fn keeps_everything_that_fits(
        mut buffer: BoundedBuffer,
        #[case] writes: &[&str],
        #[case] expected: &str,
    ) {
        for write in writes {
            assert!(buffer.write_str(write).is_ok());
        }

        assert_eq!(buffer.data(), expected);
        assert!(!buffer.is_truncated(), "nothing was dropped, so nothing to report");
    }

    #[rstest]
    // The write that overflows keeps the part that fits, and reports the truncation.
    #[case::one_oversized_write(&["abcdefghij"], "abcdefgh")]
    #[case::overflows_part_way(&["abcdef", "ghij"], "abcdefgh")]
    // A write that lands exactly on the limit is not itself a truncation, but the next one is.
    #[case::full_then_more(&["abcdefgh", "i"], "abcdefgh")]
    fn reports_truncation(
        mut buffer: BoundedBuffer,
        #[case] writes: &[&str],
        #[case] expected: &str,
    ) {
        for write in writes {
            let _ = buffer.write_str(write);
        }

        assert_eq!(buffer.data(), expected);
        assert!(buffer.is_truncated());
        assert!(buffer.data().len() <= buffer.limit());
    }

    #[rstest]
    fn a_full_buffer_rejects_further_writes(mut buffer: BoundedBuffer) {
        assert!(buffer.write_str("abcdefghij").is_ok(), "the overflowing write itself succeeds");
        assert!(buffer.write_str("more").is_err());
        assert_eq!(buffer.data(), "abcdefgh");
    }

    #[rstest]
    #[case::multibyte_split(&["ab", "cdef🦀"], "abcdef")]
    #[case::multibyte_does_not_fit_at_all(&["abcdefg", "🦀"], "abcdefg")]
    #[case::multibyte_fits_exactly(&["abcd", "🦀"], "abcd🦀")]
    fn never_splits_a_character(
        mut buffer: BoundedBuffer,
        #[case] writes: &[&str],
        #[case] expected: &str,
    ) {
        for write in writes {
            let _ = buffer.write_str(write);
        }

        // `data()` returning a `&str` at all is the real assertion: a byte-wise cut would
        // have made the buffer invalid UTF-8.
        assert_eq!(buffer.data(), expected);
    }

    // Sized per case rather than via the fixture, which only takes literal arguments.
    #[rstest]
    #[case::zero(0)]
    #[case::one(1)]
    fn tiny_limits(#[case] limit: usize) {
        let mut buffer = BoundedBuffer::new(limit);
        let _ = buffer.write_str("hello");

        assert_eq!(buffer.data().len(), limit);
        assert!(buffer.is_truncated());
    }

    #[rstest]
    fn take_hands_over_the_contents_and_resets(mut buffer: BoundedBuffer) {
        let _ = buffer.write_str("abcdefghij");

        let taken = buffer.take();
        assert_eq!(taken.data(), "abcdefgh");
        assert!(taken.is_truncated());

        // The original is empty again, keeps its limit, and accepts writes once more.
        assert_eq!(buffer.data(), "");
        assert!(!buffer.is_truncated());
        assert_eq!(buffer.limit(), LIMIT);
        assert!(buffer.write_str("xy").is_ok());
        assert_eq!(buffer.data(), "xy");
    }

    #[rstest]
    fn clear_resets_the_truncation_flag(mut buffer: BoundedBuffer) {
        let _ = buffer.write_str("abcdefghij");
        buffer.clear();

        assert_eq!(buffer.data(), "");
        assert!(!buffer.is_truncated());
        assert!(buffer.write_str("xy").is_ok());
    }

    #[rstest]
    fn into_data_returns_what_was_kept(mut buffer: BoundedBuffer) {
        let _ = buffer.write_str("abcdefghij");
        assert_eq!(buffer.into_data(), "abcdefgh");
    }

    /// Feed `writes` into a fresh buffer of `limit` bytes, ignoring the write results.
    fn filled(limit: usize, writes: &[String]) -> BoundedBuffer {
        let mut buffer = BoundedBuffer::new(limit);
        for write in writes {
            let _ = buffer.write_str(write);
        }
        buffer
    }

    prop_compose! {
        /// A limit small enough that the writes below regularly overflow it.
        fn limit_and_writes()(
            limit in 0usize..24,
            writes in prop::collection::vec("(?s).{0,8}", 0..8),
        ) -> (usize, Vec<String>) {
            (limit, writes)
        }
    }

    proptest! {
        /// The whole point of the type: whatever it is fed, it never grows past its limit.
        #[test]
        fn never_exceeds_the_limit((limit, writes) in limit_and_writes()) {
            let buffer = filled(limit, &writes);
            prop_assert!(buffer.data().len() <= limit);
            prop_assert_eq!(buffer.limit(), limit);
        }

        /// What is kept is exactly the longest prefix of everything written that fits, cut on a
        /// character boundary. `data()` being a `&str` at all also proves the cut kept it UTF-8.
        #[test]
        fn keeps_the_longest_prefix_that_fits((limit, writes) in limit_and_writes()) {
            let all = writes.concat();
            let buffer = filled(limit, &writes);
            prop_assert_eq!(buffer.data(), &all[..all.floor_char_boundary(limit)]);
        }

        /// How the data is split across writes cannot change the result.
        #[test]
        fn chunking_does_not_matter((limit, writes) in limit_and_writes()) {
            let chunked = filled(limit, &writes);
            let at_once = filled(limit, &[writes.concat()]);

            prop_assert_eq!(chunked.data(), at_once.data());
            prop_assert_eq!(chunked.is_truncated(), at_once.is_truncated());
        }

        /// Truncation is reported if and only if something was actually dropped.
        #[test]
        fn reports_truncation_exactly_when_data_was_dropped((limit, writes) in limit_and_writes()) {
            let all = writes.concat();
            let buffer = filled(limit, &writes);
            prop_assert_eq!(buffer.is_truncated(), all.len() > limit);
        }

        /// A write fails only once the buffer has given up, and from then on every write fails.
        #[test]
        fn writes_fail_only_after_truncation((limit, writes) in limit_and_writes()) {
            let mut buffer = BoundedBuffer::new(limit);
            for write in &writes {
                let was_truncated = buffer.is_truncated();
                prop_assert_eq!(buffer.write_str(write).is_err(), was_truncated);
            }
        }

        /// `take` hands the whole state over and leaves a buffer indistinguishable from a new one.
        #[test]
        fn take_moves_the_state_out((limit, writes) in limit_and_writes()) {
            let mut buffer = filled(limit, &writes);
            let before = buffer.data().to_string();
            let was_truncated = buffer.is_truncated();

            let taken = buffer.take();
            prop_assert_eq!(taken.data(), before);
            prop_assert_eq!(taken.is_truncated(), was_truncated);
            prop_assert_eq!(taken.limit(), limit);

            prop_assert_eq!(buffer.data(), "");
            prop_assert!(!buffer.is_truncated());
            prop_assert_eq!(buffer.limit(), limit);
        }

        /// So does `clear`, minus the handing over.
        #[test]
        fn clear_is_as_good_as_a_new_buffer((limit, writes) in limit_and_writes()) {
            let mut buffer = filled(limit, &writes);
            buffer.clear();

            prop_assert_eq!(buffer.data(), "");
            prop_assert!(!buffer.is_truncated());
            prop_assert_eq!(buffer.limit(), limit);

            // And it takes writes again just like a fresh one would.
            for write in &writes {
                let _ = buffer.write_str(write);
            }
            let fresh = filled(limit, &writes);
            prop_assert_eq!(buffer.data(), fresh.data());
        }

        /// Consuming the buffer returns what inspecting it showed.
        #[test]
        fn into_data_matches_data((limit, writes) in limit_and_writes()) {
            let buffer = filled(limit, &writes);
            let seen = buffer.data().to_string();
            prop_assert_eq!(buffer.into_data(), seen);
        }
    }
}
