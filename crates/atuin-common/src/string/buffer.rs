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
}
