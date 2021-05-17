use core::mem;

use ext_slice::ByteSlice;
use search::byte_frequencies::BYTE_FREQUENCIES;

/// PrefilterState tracks state associated with the effectiveness of a
/// prefilter. It is used to track how many bytes, on average, are skipped by
/// the prefilter. If this average dips below a certain threshold over time,
/// then the state renders the prefilter inert and stops using it.
///
/// A prefilter state should be created for each search. (Where creating an
/// iterator via, e.g., `find_iter`, is treated as a single search.)
#[derive(Clone, Debug)]
pub struct PrefilterState {
    /// The number of skips that has been executed.
    skips: usize,
    /// The total number of bytes that have been skipped.
    skipped: usize,
    /// The maximum length of a match. This is used to help determine how many
    /// bytes on average should be skipped in order for a prefilter to be
    /// effective.
    max_match_len: usize,
    /// Once this heuristic has been deemed ineffective, it will be inert
    /// throughout the rest of its lifetime. This serves as a cheap way to
    /// check inertness.
    inert: bool,
}

impl PrefilterState {
    /// The minimum number of skip attempts to try before considering whether
    /// a prefilter is effective or not.
    const MIN_SKIPS: usize = 50;

    /// The minimum amount of bytes that skipping must average.
    ///
    /// This value was chosen based on varying it and checking the bstr/find/
    /// microbenchmarks. In particular, this can impact the
    /// pathological/repeated-{huge,small} benchmarks quite a bit if it's
    /// set too low.
    const MIN_SKIP_BYTES: usize = 8;

    /// Create a fresh prefilter state.
    pub fn new(max_match_len: usize) -> PrefilterState {
        if max_match_len == 0 {
            return PrefilterState::inert();
        }
        PrefilterState { skips: 0, skipped: 0, max_match_len, inert: false }
    }

    /// Create a fresh prefilter state that is always inert.
    fn inert() -> PrefilterState {
        PrefilterState { skips: 0, skipped: 0, max_match_len: 0, inert: true }
    }

    /// Update this state with the number of bytes skipped on the last
    /// invocation of the prefilter.
    #[inline]
    pub fn update(&mut self, skipped: usize) {
        self.skips += 1;
        self.skipped += skipped;
    }

    /// Return true if and only if this state indicates that a prefilter is
    /// still effective.
    #[inline]
    pub fn is_effective(&mut self) -> bool {
        if self.inert {
            return false;
        }
        if self.skips < PrefilterState::MIN_SKIPS {
            return true;
        }
        if self.skipped >= PrefilterState::MIN_SKIP_BYTES * self.skips {
            return true;
        }

        // We're inert.
        self.inert = true;
        false
    }
}

/// A heuristic frequency based prefilter for searching a single needle.
///
/// This prefilter attempts to pick out the byte in a needle that is predicted
/// to occur least frequently, and search for that using fast vectorized
/// routines. If a rare enough byte could not be found, then this prefilter's
/// constructors will return `None`.
///
/// This can be combined with `PrefilterState` to dynamically render this
/// prefilter inert if it proves to ineffective.
#[derive(Clone, Debug)]
pub struct Freqy {
    /// Whether this prefilter should be used or not.
    inert: bool,
    /// The length of the needle we're searching for.
    needle_len: usize,
    /// The rarest byte in the needle, according to pre-computed frequency
    /// analysis.
    rare1: u8,
    /// The leftmost offset of the rarest byte in the needle.
    rare1i: usize,
    /// The second rarest byte in the needle, according to pre-computed
    /// frequency analysis. (This may be equivalent to the rarest byte.)
    ///
    /// The second rarest byte is used as a type of guard for quickly detecting
    /// a mismatch after memchr locates an instance of the rarest byte. This
    /// is a hedge against pathological cases where the pre-computed frequency
    /// analysis may be off. (But of course, does not prevent *all*
    /// pathological cases.)
    rare2: u8,
    /// The leftmost offset of the second rarest byte in the needle.
    rare2i: usize,
}

impl Freqy {
    /// The maximum frequency rank permitted. If the rarest byte in the needle
    /// has a frequency rank above this value, then Freqy is not used.
    const MAX_RANK: usize = 200;

    /// Return a fresh prefilter state that can be used with this prefilter. A
    /// prefilter state is used to track the effectiveness of a prefilter for
    /// speeding up searches. Therefore, the prefilter state should generally
    /// be reused on subsequent searches (such as in an iterator). For searches
    /// on a different haystack, then a new prefilter state should be used.
    pub fn prefilter_state(&self) -> PrefilterState {
        if self.inert {
            PrefilterState::inert()
        } else {
            PrefilterState::new(self.needle_len)
        }
    }

    /// Returns a valid but inert prefilter. This is valid for both the forward
    /// and reverse direction.
    ///
    /// It is never correct to use an inert prefilter. The results of finding
    /// the next (or previous) candidate are unspecified.
    fn inert() -> Freqy {
        Freqy {
            inert: true,
            needle_len: 0,
            rare1: 0,
            rare1i: 0,
            rare2: 0,
            rare2i: 0,
        }
    }

    /// Return search info for the given needle in the forward direction.
    pub fn forward(needle: &[u8]) -> Freqy {
        if needle.is_empty() {
            return Freqy::inert();
        }

        // Find the rarest two bytes. Try to make them distinct (but it's not
        // required).
        let (mut rare1, mut rare1i) = (needle[0], 0);
        let (mut rare2, mut rare2i) = (needle[0], 0);
        if needle.len() >= 2 {
            rare2 = needle[1];
            rare2i = 1;
        }
        if Freqy::rank(rare2) < Freqy::rank(rare1) {
            mem::swap(&mut rare1, &mut rare2);
            mem::swap(&mut rare1i, &mut rare2i);
        }
        for (i, b) in needle.bytes().enumerate().skip(2) {
            if Freqy::rank(b) < Freqy::rank(rare1) {
                rare2 = rare1;
                rare2i = rare1i;
                rare1 = b;
                rare1i = i;
            } else if b != rare1 && Freqy::rank(b) < Freqy::rank(rare2) {
                rare2 = b;
                rare2i = i;
            }
        }
        if Freqy::rank(rare1) > Freqy::MAX_RANK {
            return Freqy::inert();
        }
        let needle_len = needle.len();
        Freqy { inert: false, needle_len, rare1, rare1i, rare2, rare2i }
    }

    /// Return search info for the given needle in the reverse direction.
    pub fn reverse(needle: &[u8]) -> Freqy {
        if needle.is_empty() {
            return Freqy::inert();
        }

        // Find the rarest two bytes. Try to make them distinct (but it's not
        // required). In reverse, the offsets correspond to the number of bytes
        // from the end of the needle. So `0` is the last byte in the needle.
        let (mut rare1i, mut rare2i) = (0, 0);
        if needle.len() >= 2 {
            rare2i += 1;
        }
        let mut rare1 = needle[needle.len() - rare1i - 1];
        let mut rare2 = needle[needle.len() - rare2i - 1];
        if Freqy::rank(rare2) < Freqy::rank(rare1) {
            mem::swap(&mut rare1, &mut rare2);
            mem::swap(&mut rare1i, &mut rare2i);
        }
        for (i, b) in needle.bytes().rev().enumerate().skip(2) {
            if Freqy::rank(b) < Freqy::rank(rare1) {
                rare2 = rare1;
                rare2i = rare1i;
                rare1 = b;
                rare1i = i;
            } else if b != rare1 && Freqy::rank(b) < Freqy::rank(rare2) {
                rare2 = b;
                rare2i = i;
            }
        }
        if Freqy::rank(rare1) > Freqy::MAX_RANK {
            return Freqy::inert();
        }
        let needle_len = needle.len();
        Freqy { inert: false, needle_len, rare1, rare1i, rare2, rare2i }
    }

    /// Look for a possible occurrence of needle. The position returned
    /// corresponds to the beginning of the occurrence, if one exists.
    ///
    /// Callers may assume that this never returns false negatives (i.e., it
    /// never misses an actual occurrence), but must check that the returned
    /// position corresponds to a match. That is, it can return false
    /// positives.
    ///
    /// This should only be used when Freqy is constructed for forward
    /// searching.
    pub fn find_candidate(
        &self,
        prestate: &mut PrefilterState,
        haystack: &[u8],
    ) -> Option<usize> {
        debug_assert!(!self.inert);

        let mut i = 0;
        while prestate.is_effective() {
            // Use a fast vectorized implementation to skip to the next
            // occurrence of the rarest byte (heuristically chosen) in the
            // needle.
            i += match haystack[i..].find_byte(self.rare1) {
                None => return None,
                Some(found) => {
                    prestate.update(found);
                    found
                }
            };

            // If we can't align our first match with the haystack, then a
            // match is impossible.
            if i < self.rare1i {
                i += 1;
                continue;
            }

            // Align our rare2 byte with the haystack. A mismatch means that
            // a match is impossible.
            let aligned_rare2i = i - self.rare1i + self.rare2i;
            if haystack.get(aligned_rare2i) != Some(&self.rare2) {
                i += 1;
                continue;
            }

            // We've done what we can. There might be a match here.
            return Some(i - self.rare1i);
        }
        // The only way we get here is if we believe our skipping heuristic
        // has become ineffective. We're allowed to return false positives,
        // so return the position at which we advanced to, aligned to the
        // haystack.
        Some(i.saturating_sub(self.rare1i))
    }

    /// Look for a possible occurrence of needle, in reverse, starting from the
    /// end of the given haystack. The position returned corresponds to the
    /// position immediately after the end of the occurrence, if one exists.
    ///
    /// Callers may assume that this never returns false negatives (i.e., it
    /// never misses an actual occurrence), but must check that the returned
    /// position corresponds to a match. That is, it can return false
    /// positives.
    ///
    /// This should only be used when Freqy is constructed for reverse
    /// searching.
    pub fn rfind_candidate(
        &self,
        prestate: &mut PrefilterState,
        haystack: &[u8],
    ) -> Option<usize> {
        debug_assert!(!self.inert);

        let mut i = haystack.len();
        while prestate.is_effective() {
            // Use a fast vectorized implementation to skip to the next
            // occurrence of the rarest byte (heuristically chosen) in the
            // needle.
            i = match haystack[..i].rfind_byte(self.rare1) {
                None => return None,
                Some(found) => {
                    prestate.update(i - found);
                    found
                }
            };

            // If we can't align our first match with the haystack, then a
            // match is impossible.
            if i + self.rare1i + 1 > haystack.len() {
                continue;
            }

            // Align our rare2 byte with the haystack. A mismatch means that
            // a match is impossible.
            let aligned = match (i + self.rare1i).checked_sub(self.rare2i) {
                None => continue,
                Some(aligned) => aligned,
            };
            if haystack.get(aligned) != Some(&self.rare2) {
                continue;
            }

            // We've done what we can. There might be a match here.
            return Some(i + self.rare1i + 1);
        }
        // The only way we get here is if we believe our skipping heuristic
        // has become ineffective. We're allowed to return false positives,
        // so return the position at which we advanced to, aligned to the
        // haystack.
        Some(i + self.rare1i + 1)
    }

    /// Return the heuristical frequency rank of the given byte. A lower rank
    /// means the byte is believed to occur less frequently.
    fn rank(b: u8) -> usize {
        BYTE_FREQUENCIES[b as usize] as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ext_slice::B;

    #[test]
    fn freqy_forward() {
        // N.B. We sometimes use uppercase here since that mostly ensures freqy
        // will be constructable. Lowercase letters may be too common for freqy
        // to work.

        let s = Freqy::forward(B("BAR"));
        let mut pre = s.prefilter_state();
        assert_eq!(Some(0), s.find_candidate(&mut pre, B("BARFOO")));

        let s = Freqy::forward(B("BAR"));
        let mut pre = s.prefilter_state();
        assert_eq!(Some(3), s.find_candidate(&mut pre, B("FOOBAR")));

        let s = Freqy::forward(B("zyzy"));
        let mut pre = s.prefilter_state();
        assert_eq!(Some(0), s.find_candidate(&mut pre, B("zyzz")));

        let s = Freqy::forward(B("zyzy"));
        let mut pre = s.prefilter_state();
        assert_eq!(Some(2), s.find_candidate(&mut pre, B("zzzy")));

        let s = Freqy::forward(B("zyzy"));
        let mut pre = s.prefilter_state();
        assert_eq!(None, s.find_candidate(&mut pre, B("zazb")));

        let s = Freqy::forward(B("yzyz"));
        let mut pre = s.prefilter_state();
        assert_eq!(Some(0), s.find_candidate(&mut pre, B("yzyy")));

        let s = Freqy::forward(B("yzyz"));
        let mut pre = s.prefilter_state();
        assert_eq!(Some(2), s.find_candidate(&mut pre, B("yyyz")));

        let s = Freqy::forward(B("yzyz"));
        let mut pre = s.prefilter_state();
        assert_eq!(None, s.find_candidate(&mut pre, B("yayb")));
    }

    #[test]
    fn freqy_reverse() {
        // N.B. We sometimes use uppercase here since that mostly ensures freqy
        // will be constructable. Lowercase letters may be too common for freqy
        // to work.

        let s = Freqy::reverse(B("BAR"));
        let mut pre = s.prefilter_state();
        assert_eq!(Some(3), s.rfind_candidate(&mut pre, B("BARFOO")));

        let s = Freqy::reverse(B("BAR"));
        let mut pre = s.prefilter_state();
        assert_eq!(Some(6), s.rfind_candidate(&mut pre, B("FOOBAR")));

        let s = Freqy::reverse(B("zyzy"));
        let mut pre = s.prefilter_state();
        assert_eq!(Some(2), s.rfind_candidate(&mut pre, B("zyzz")));

        let s = Freqy::reverse(B("zyzy"));
        let mut pre = s.prefilter_state();
        assert_eq!(Some(4), s.rfind_candidate(&mut pre, B("zzzy")));

        let s = Freqy::reverse(B("zyzy"));
        let mut pre = s.prefilter_state();
        assert_eq!(None, s.rfind_candidate(&mut pre, B("zazb")));

        let s = Freqy::reverse(B("yzyz"));
        let mut pre = s.prefilter_state();
        assert_eq!(Some(2), s.rfind_candidate(&mut pre, B("yzyy")));

        let s = Freqy::reverse(B("yzyz"));
        let mut pre = s.prefilter_state();
        assert_eq!(Some(4), s.rfind_candidate(&mut pre, B("yyyz")));

        let s = Freqy::reverse(B("yzyz"));
        let mut pre = s.prefilter_state();
        assert_eq!(None, s.rfind_candidate(&mut pre, B("yayb")));
    }
}
