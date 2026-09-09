use std::collections::VecDeque;
use std::fmt;

/// Controls how many bytes a [`BoundedBuffer`] can store.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Limit {
    /// How many bytes from the start of the buffer to keep.
    pub start: usize,
    /// How many bytes from the end of the buffer to keep.
    pub end: usize,
}

/// A string buffer limited to a certain length.
#[derive(Clone)]
pub struct BoundedBuffer {
    start: String,
    /// Invariant: `end` is valid UTF-8.
    end: VecDeque<u8>,
    truncated: bool,
    limit: Limit,
}

/// The finished contents of a [`BoundedBuffer`].
///
/// Returned by [`BoundedBuffer::take`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BufferContents {
    /// The start portion of the buffer's contents.
    ///
    /// If `end` is [`None`], this contains the entire contents of the buffer.
    pub start: String,

    /// The end portion of the buffer's contents.
    ///
    /// If this is [`None`], the buffer didn't need any truncation, so the entire contents are in
    /// `start`. If this is [`Some`], there is a middle portion that was discarded.
    pub end: Option<String>,
}

impl BoundedBuffer {
    /// Create a new [`BoundedBuffer`] with the given size limit.
    #[must_use]
    pub fn new(limit: Limit) -> Self {
        Self {
            start: String::new(),
            end: VecDeque::new(),
            truncated: false,
            limit,
        }
    }

    /// Return the buffer's contents and reset the buffer back to the initial state.
    pub fn take(&mut self) -> BufferContents {
        let start = std::mem::take(&mut self.start);

        if std::mem::take(&mut self.truncated) {
            let end = Vec::from(std::mem::take(&mut self.end));
            if cfg!(debug_assertions) {
                std::str::from_utf8(&end).expect("invalid utf-8: this is UB!");
            }

            #[allow(
                unsafe_code,
                reason = "unchecked conversion is much faster (O(1) vs O(n)) and is guaranteed by \
                          this type's invariants; every place where `self.end` is modified is \
                          accompanied by a justification for why the invariant is necessarily \
                          upheld, and we perform an explicit check in debug mode for extra \
                          assurance"
            )]
            // SAFETY: Guaranteed by invariant on `self.end`.
            let end = unsafe { String::from_utf8_unchecked(end) };
            BufferContents {
                start,
                end: Some(end),
            }
        } else {
            let mut start = start.into_bytes();
            start.extend(&self.end);
            self.end.clear();

            if cfg!(debug_assertions) {
                std::str::from_utf8(&start).expect("invalid utf-8: this is UB!");
            }

            #[allow(
                unsafe_code,
                reason = "unchecked conversion is much faster (O(1) vs O(n)) and is guaranteed by \
                          this type's invariants; every place where `self.end` is modified is \
                          accompanied by a justification for why the invariant is necessarily \
                          upheld, and we perform an explicit check in debug mode for extra \
                          assurance"
            )]
            // SAFETY: `self.end` is valid UTF-8 by its invariant. `start` is the concatenation of
            // valid UTF-8 (guaranteed because it came from a `String`) with `self.end`, which
            // necessarily produces valid UTF-8.
            let start = unsafe { String::from_utf8_unchecked(start) };
            BufferContents { start, end: None }
        }
    }

    pub fn clear(&mut self) {
        self.start.clear();
        self.end.clear();
        self.truncated = false;
    }
}

impl fmt::Write for BoundedBuffer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let s = if self.truncated || !self.end.is_empty() {
            s
        } else {
            let available = self.limit.start - self.start.len();
            if available >= s.len() {
                self.start.push_str(s);
                return Ok(());
            }
            let (start, end) = s.split_at(s.floor_char_boundary(available));
            self.start.push_str(start);
            end
        };

        if s.len() > self.limit.end {
            self.truncated = true;
            // Maintains the invariant because an empty sequence of bytes is valid UTF-8.
            self.end.clear();
            let tail = &s[s.ceil_char_boundary(s.len() - self.limit.end)..];
            // Maintains the UTF-8 invariant because we're pushing a UTF-8 string slice.
            self.end.extend(tail.as_bytes());
            return Ok(());
        }

        let keep = self.limit.end - s.len();
        if self.end.len() > keep {
            self.truncated = true;
            let keep_start = self.end.len() - keep;

            // Find the nearest char boundary at or after `keep_start`.
            let offset = self.end.iter().skip(keep_start).take(char::MAX_LEN_UTF8).position(|&b| {
                // There are three possibilities for the return value of `from_utf8`:
                //
                // * `Ok(())`: `b` is ASCII and thus a valid char boundary.
                // * `Err(e)` where `e.error_len()` is `None`: `b` is the start of a multibyte UTF-8
                //   sequence and thus a valid char boundary.
                // * `Err(e)` where `e.error_len()` is `Some`: `b` is the middle of a multibyte
                //   UTF-8 sequence and thus *not* a valid char boundary.
                !matches!(std::str::from_utf8(&[b]), Err(e) if e.error_len().is_some())
            });

            // Round `keep_start` up to the nearest char boundary.
            let keep_start = if let Some(offset) = offset {
                keep_start + offset
            } else {
                self.end.len()
            };

            // Maintains the invariant because we calculated `keep_start` to be a valid char
            // boundary.
            self.end.drain(..keep_start);
        }

        // Maintains the invariant because `s` is a valid UTF-8 string slice, and appending valid
        // UTF-8 to other valid UTF-8 (which `self.end` is due to the invariant) always results in
        // valid UTF-8.
        self.end.extend(s.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use proptest::prelude::*;
    use rstest::{fixture, rstest};

    use super::{BoundedBuffer, BufferContents, Limit};

    const START: usize = 5;
    const END: usize = 3;
    const LIMIT: Limit = Limit {
        start: START,
        end: END,
    };

    /// A buffer that keeps five bytes from the start and three from the end, so eight bytes fit
    /// whole and anything past that loses its middle.
    #[fixture]
    fn buffer(#[default(START)] start: usize, #[default(END)] end: usize) -> BoundedBuffer {
        BoundedBuffer::new(Limit { start, end })
    }

    /// Feed `writes` into a fresh buffer with the given limit and take its contents.
    fn filled(limit: Limit, writes: &[impl AsRef<str>]) -> BufferContents {
        let mut buffer = BoundedBuffer::new(limit);
        for write in writes {
            buffer.write_str(write.as_ref()).expect("writing never fails");
        }
        buffer.take()
    }

    /// The contents as a caller sees them: the kept text, and whether a middle was dropped.
    fn parts(contents: &BufferContents) -> (&str, Option<&str>) {
        (&contents.start, contents.end.as_deref())
    }

    // -- Nothing dropped ------------------------------------------------------

    #[rstest]
    #[case::empty(&[], "")]
    #[case::one_write(&["abc"], "abc")]
    #[case::several_writes(&["ab", "cd"], "abcd")]
    // The two halves of the limit are one budget: eight bytes fit even though `start` is five.
    #[case::exactly_the_limit(&["abcdefgh"], "abcdefgh")]
    #[case::exactly_the_limit_in_pieces(&["abcd", "efgh"], "abcdefgh")]
    // Spilling into the end portion is not truncation on its own -- nothing was dropped yet.
    #[case::spills_into_the_end_portion(&["abcdefg"], "abcdefg")]
    fn keeps_everything_that_fits(#[case] writes: &[&str], #[case] expected: &str) {
        let contents = filled(LIMIT, writes);
        assert_eq!(
            parts(&contents),
            (expected, None),
            "nothing was dropped, so it all belongs in `start`",
        );
    }

    // -- The middle goes ------------------------------------------------------

    #[rstest]
    // One byte over: the start fills, the end keeps the last three, and `e` is what goes.
    #[case::one_byte_over(&["abcdefghi"], "abcde", "ghi")]
    #[case::far_over(&["abcdefghijklmnop"], "abcde", "nop")]
    // The same content, however it is chopped up, yields the same answer.
    #[case::overflows_part_way(&["abcdef", "ghi"], "abcde", "ghi")]
    #[case::byte_at_a_time(&["a", "b", "c", "d", "e", "f", "g", "h", "i"], "abcde", "ghi")]
    #[case::full_then_more(&["abcdefgh", "i"], "abcde", "ghi")]
    fn keeps_the_start_and_the_end(
        #[case] writes: &[&str],
        #[case] start: &str,
        #[case] end: &str,
    ) {
        let contents = filled(LIMIT, writes);
        assert_eq!(parts(&contents), (start, Some(end)));
    }

    /// The regression this type exists for: one huge write must be capped just like many small
    /// ones. Keeping `s[limit.end..]` instead of the last `limit.end` bytes let a single write
    /// through almost whole.
    #[rstest]
    fn one_huge_write_is_capped_like_many_small_ones() {
        let all = "x".repeat(10_000);

        let at_once = filled(LIMIT, std::slice::from_ref(&all));
        let in_pieces = filled(
            LIMIT,
            &all.as_bytes()
                .chunks(7)
                .map(|c| std::str::from_utf8(c).expect("ascii").to_string())
                .collect::<Vec<_>>(),
        );

        assert_eq!(at_once, in_pieces);
        assert_eq!(at_once.start.len(), START);
        assert_eq!(at_once.end.as_deref().map(str::len), Some(END));
    }

    #[rstest]
    fn the_end_portion_is_a_sliding_window(mut buffer: BoundedBuffer) {
        // Once the start is full, every further write scrolls the end window along, always
        // leaving the three most recent bytes. The first spill drops nothing, so it is not yet a
        // split capture -- `take` joins the halves back up.
        buffer.write_str("abcde").expect("writing never fails");
        let expected = [
            ("fgh", ("abcdefgh", None)),
            ("ij", ("abcde", Some("hij"))),
            ("klmn", ("abcde", Some("lmn"))),
        ];
        for (write, expected) in expected {
            buffer.write_str(write).expect("writing never fails");
            let contents = buffer.clone().take();
            assert_eq!(parts(&contents), expected, "after writing {write:?}");
        }
    }

    // -- Degenerate limits ----------------------------------------------------

    #[rstest]
    #[case::no_end_kept(Limit { start: 4, end: 0 }, "abcd", Some(""))]
    #[case::no_start_kept(Limit { start: 0, end: 4 }, "", Some("wxyz"))]
    #[case::nothing_kept(Limit { start: 0, end: 0 }, "", Some(""))]
    fn zero_sided_limits(#[case] limit: Limit, #[case] start: &str, #[case] end: Option<&str>) {
        let contents = filled(limit, &["abcdefghijklmnopqrstuvwxyz"]);
        assert_eq!(parts(&contents), (start, end));
    }

    #[rstest]
    fn a_zero_limit_keeps_nothing_but_still_reports_the_cut() {
        let limit = Limit { start: 0, end: 0 };
        assert_eq!(filled(limit, &[""]), BufferContents {
            start: String::new(),
            end: None
        });
        assert_eq!(filled(limit, &["a"]), BufferContents {
            start: String::new(),
            end: Some(String::new()),
        });
    }

    // -- Characters stay whole ------------------------------------------------

    #[rstest]
    // A crab is four bytes, so it cannot fit in the two bytes left in `start`; it moves along
    // rather than being cut in half.
    #[case::does_not_fit_in_the_start(Limit { start: 6, end: 8 }, &["abcd", "🦀"], "abcd🦀", None)]
    // Nor can half of one be kept at the front of the end window.
    #[case::does_not_fit_in_the_end(Limit { start: 2, end: 3 }, &["ab", "🦀z"], "ab", Some("z"))]
    #[case::fits_the_end_exactly(Limit { start: 2, end: 4 }, &["ab", "cd🦀"], "ab", Some("🦀"))]
    fn never_splits_a_character(
        #[case] limit: Limit,
        #[case] writes: &[&str],
        #[case] start: &str,
        #[case] end: Option<&str>,
    ) {
        // That `start` and `end` are `String`s at all is half the assertion: a byte-wise cut
        // would have made them invalid UTF-8, which the debug check inside `take` catches.
        let contents = filled(limit, writes);
        assert_eq!(parts(&contents), (start, end));
    }

    // -- take / clear ---------------------------------------------------------

    #[rstest]
    fn take_hands_over_the_contents_and_resets(mut buffer: BoundedBuffer) {
        buffer.write_str("abcdefghi").expect("writing never fails");

        let taken = buffer.take();
        assert_eq!(parts(&taken), ("abcde", Some("ghi")));

        // The original is empty again, keeps its limit, and accepts writes once more.
        assert_eq!(buffer.take(), BufferContents {
            start: String::new(),
            end: None
        });
        buffer.write_str("xy").expect("writing never fails");
        assert_eq!(parts(&buffer.take()), ("xy", None));
    }

    #[rstest]
    fn clear_forgets_the_dropped_middle(mut buffer: BoundedBuffer) {
        buffer.write_str("abcdefghi").expect("writing never fails");
        buffer.clear();

        buffer.write_str("xy").expect("writing never fails");
        assert_eq!(
            parts(&buffer.take()),
            ("xy", None),
            "a cleared buffer must not still claim its middle was dropped",
        );
    }

    #[rstest]
    fn writes_always_succeed(mut buffer: BoundedBuffer) {
        // Unlike the old first-1-MiB-only buffer, this one never refuses input: it keeps
        // consuming so that the *end* of a long-running command is still there at the finish.
        for write in ["abcdefghij", "more", "and more"] {
            assert!(buffer.write_str(write).is_ok(), "{write:?} was refused");
        }
        assert_eq!(parts(&buffer.take()), ("abcde", Some("ore")));
    }

    // -- Properties -----------------------------------------------------------

    prop_compose! {
        /// Limits small enough that the writes below regularly overflow them.
        fn limit_and_writes()(
            start in 0usize..12,
            end in 0usize..12,
            writes in prop::collection::vec("(?s).{0,8}", 0..8),
        ) -> (Limit, Vec<String>) {
            (Limit { start, end }, writes)
        }
    }

    proptest! {
        /// The whole point of the type: whatever it is fed, it never outgrows its limit. Once a
        /// middle has been dropped each half is bounded on its own; until then the two budgets
        /// are one, because everything kept comes back joined in `start`.
        #[test]
        fn never_exceeds_the_limit((limit, writes) in limit_and_writes()) {
            let contents = filled(limit, &writes);
            if let Some(end) = &contents.end {
                prop_assert!(contents.start.len() <= limit.start);
                prop_assert!(end.len() <= limit.end);
            } else {
                prop_assert!(contents.start.len() <= limit.start + limit.end);
            }
        }

        /// A middle is reported as dropped if and only if bytes really went missing. `end` being
        /// `Some` is the only signal callers get, so it must never cry wolf and never stay quiet.
        #[test]
        fn reports_a_dropped_middle_exactly_when_data_was_dropped(
            (limit, writes) in limit_and_writes(),
        ) {
            let all = writes.concat();
            let contents = filled(limit, &writes);
            let kept = contents.start.len() + contents.end.as_ref().map_or(0, String::len);
            prop_assert_eq!(contents.end.is_some(), kept < all.len());
        }

        /// The bounds either side of that: what fits in the start half alone is never touched,
        /// and what outgrows both halves together always loses something.
        #[test]
        fn the_limits_decide_whether_anything_is_dropped((limit, writes) in limit_and_writes()) {
            let all = writes.concat();
            let contents = filled(limit, &writes);
            if all.len() <= limit.start {
                prop_assert_eq!(contents.end, None);
            } else if all.len() > limit.start + limit.end {
                prop_assert!(contents.end.is_some());
            }
        }

        /// What is kept really is the start and the end of what was written, cut on character
        /// boundaries: prefix ++ suffix, with only the middle missing.
        #[test]
        fn keeps_a_prefix_and_a_suffix((limit, writes) in limit_and_writes()) {
            let all = writes.concat();
            let contents = filled(limit, &writes);
            prop_assert!(all.starts_with(&contents.start));
            if let Some(end) = &contents.end {
                prop_assert!(all.ends_with(end.as_str()));
                // The two halves never overlap, so what they show was really in the input.
                prop_assert!(contents.start.len() + end.len() <= all.len());
            } else {
                prop_assert_eq!(&contents.start, &all);
            }
        }

        /// Each half is as long as its limit allows, given that characters stay whole. A cut can
        /// only be pulled back by less than one character's worth of bytes.
        #[test]
        fn keeps_as_much_as_it_can((limit, writes) in limit_and_writes()) {
            let all = writes.concat();
            let contents = filled(limit, &writes);
            if let Some(end) = &contents.end {
                prop_assert!(contents.start.len() + char::MAX_LEN_UTF8 > limit.start.min(all.len()));
                prop_assert!(end.len() + char::MAX_LEN_UTF8 > limit.end.min(all.len()));
            }
        }

        /// How the data is split across writes cannot change the result.
        #[test]
        fn chunking_does_not_matter((limit, writes) in limit_and_writes()) {
            prop_assert_eq!(filled(limit, &writes), filled(limit, &[writes.concat()]));
        }

        /// Writing never fails, however much is written.
        #[test]
        fn writes_never_fail((limit, writes) in limit_and_writes()) {
            let mut buffer = BoundedBuffer::new(limit);
            for write in &writes {
                prop_assert!(buffer.write_str(write).is_ok());
            }
        }

        /// `take` leaves a buffer indistinguishable from a new one.
        #[test]
        fn take_resets_the_buffer((limit, writes) in limit_and_writes()) {
            let mut buffer = BoundedBuffer::new(limit);
            for write in &writes {
                let _ = buffer.write_str(write);
            }
            let _ = buffer.take();

            for write in &writes {
                let _ = buffer.write_str(write);
            }
            prop_assert_eq!(buffer.take(), filled(limit, &writes));
        }

        /// So does `clear`.
        #[test]
        fn clear_resets_the_buffer((limit, writes) in limit_and_writes()) {
            let mut buffer = BoundedBuffer::new(limit);
            for write in &writes {
                let _ = buffer.write_str(write);
            }
            buffer.clear();

            for write in &writes {
                let _ = buffer.write_str(write);
            }
            prop_assert_eq!(buffer.take(), filled(limit, &writes));
        }
    }
}
