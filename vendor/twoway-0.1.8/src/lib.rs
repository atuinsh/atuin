#![cfg_attr(not(test), no_std)]
#![cfg_attr(feature = "pattern", feature(pattern))]
#![cfg_attr(feature = "pcmp", feature(asm))]

#[cfg(not(test))]
extern crate core as std;

use std::cmp;
use std::usize;

extern crate memchr;

mod tw;
#[cfg(feature = "pcmp")]
pub mod pcmp;
pub mod bmh;
#[cfg(feature = "test-set")]
pub mod set;
mod util;

#[cfg(feature = "pattern")]
use std::str::pattern::{
    Pattern,
    Searcher,
    ReverseSearcher,
    SearchStep,
};

/// `find_str` finds the first ocurrence of `pattern` in the `text`.
///
/// Uses the SSE42 version if it is compiled in.
#[inline]
pub fn find_str(text: &str, pattern: &str) -> Option<usize> {
    find_bytes(text.as_bytes(), pattern.as_bytes())
}

/// `find_bytes` finds the first ocurrence of `pattern` in the `text`.
///
/// Uses the SSE42 version if it is compiled in.
#[cfg(feature = "pcmp")]
#[inline]
pub fn find_bytes(text: &[u8], pattern: &[u8]) -> Option<usize> {
    pcmp::find(text, pattern)
}

/// `find_bytes` finds the first ocurrence of `pattern` in the `text`.
///
/// Uses the SSE42 version if it is compiled in.
#[cfg(not(feature = "pcmp"))]
pub fn find_bytes(text: &[u8], pattern: &[u8]) -> Option<usize> {
    if pattern.is_empty() {
        Some(0)
    } else if pattern.len() == 1 {
        memchr::memchr(pattern[0], text)
    } else {
        let mut searcher = TwoWaySearcher::new(pattern, text.len());
        let is_long = searcher.memory == usize::MAX;
        // write out `true` and `false` cases to encourage the compiler
        // to specialize the two cases separately.
        if is_long {
            searcher.next::<MatchOnly>(text, pattern, true).map(|t| t.0)
        } else {
            searcher.next::<MatchOnly>(text, pattern, false).map(|t| t.0)
        }
    }
}

/// `rfind_str` finds the last ocurrence of `pattern` in the `text`
/// and returns the index of the start of the match.
///
/// As of this writing, this function uses the two way algorithm
/// in pure rust (with no SSE4.2 support).
#[inline]
pub fn rfind_str(text: &str, pattern: &str) -> Option<usize> {
    rfind_bytes(text.as_bytes(), pattern.as_bytes())
}

/// `rfind_bytes` finds the last ocurrence of `pattern` in the `text`,
/// and returns the index of the start of the match.
///
/// As of this writing, this function uses the two way algorithm
/// in pure rust (with no SSE4.2 support).
pub fn rfind_bytes(text: &[u8], pattern: &[u8]) -> Option<usize> {
    if pattern.is_empty() {
        Some(text.len())
    } else if pattern.len() == 1 {
        memchr::memrchr(pattern[0], text)
    } else {
        let mut searcher = TwoWaySearcher::new(pattern, text.len());
        let is_long = searcher.memory == usize::MAX;
        // write out `true` and `false` cases to encourage the compiler
        // to specialize the two cases separately.
        if is_long {
            searcher.next_back::<MatchOnly>(text, pattern, true).map(|t| t.0)
        } else {
            searcher.next_back::<MatchOnly>(text, pattern, false).map(|t| t.0)
        }
    }
}


/// Dummy wrapper for &str
#[doc(hidden)]
pub struct Str<'a>(pub &'a str);

#[cfg(feature = "pattern")]
/// Non-allocating substring search.
///
/// Will handle the pattern `""` as returning empty matches at each character
/// boundary.
impl<'a, 'b> Pattern<'a> for Str<'b> {
    type Searcher = StrSearcher<'a, 'b>;

    #[inline]
    fn into_searcher(self, haystack: &'a str) -> StrSearcher<'a, 'b> {
        StrSearcher::new(haystack, self.0)
    }

    /// Checks whether the pattern matches at the front of the haystack
    #[inline]
    fn is_prefix_of(self, haystack: &'a str) -> bool {
        let self_ = self.0;
        haystack.is_char_boundary(self_.len()) &&
            self_ == &haystack[..self_.len()]
    }

    /// Checks whether the pattern matches at the back of the haystack
    #[inline]
    fn is_suffix_of(self, haystack: &'a str) -> bool {
        let self_ = self.0;
        self_.len() <= haystack.len() &&
            haystack.is_char_boundary(haystack.len() - self_.len()) &&
            self_ == &haystack[haystack.len() - self_.len()..]
    }

}

#[derive(Clone, Debug)]
#[doc(hidden)]
/// Associated type for `<&str as Pattern<'a>>::Searcher`.
pub struct StrSearcher<'a, 'b> {
    haystack: &'a str,
    needle: &'b str,

    searcher: StrSearcherImpl,
}

#[derive(Clone, Debug)]
enum StrSearcherImpl {
    Empty(EmptyNeedle),
    TwoWay(TwoWaySearcher),
}

#[derive(Clone, Debug)]
struct EmptyNeedle {
    position: usize,
    end: usize,
    is_match_fw: bool,
    is_match_bw: bool,
}

impl<'a, 'b> StrSearcher<'a, 'b> {
    pub fn new(haystack: &'a str, needle: &'b str) -> StrSearcher<'a, 'b> {
        if needle.is_empty() {
            StrSearcher {
                haystack: haystack,
                needle: needle,
                searcher: StrSearcherImpl::Empty(EmptyNeedle {
                    position: 0,
                    end: haystack.len(),
                    is_match_fw: true,
                    is_match_bw: true,
                }),
            }
        } else {
            StrSearcher {
                haystack: haystack,
                needle: needle,
                searcher: StrSearcherImpl::TwoWay(
                    TwoWaySearcher::new(needle.as_bytes(), haystack.len())
                ),
            }
        }
    }
}

#[cfg(feature = "pattern")]
unsafe impl<'a, 'b> Searcher<'a> for StrSearcher<'a, 'b> {
    fn haystack(&self) -> &'a str { self.haystack }

    #[inline]
    fn next(&mut self) -> SearchStep {
        match self.searcher {
            StrSearcherImpl::Empty(ref mut searcher) => {
                // empty needle rejects every char and matches every empty string between them
                let is_match = searcher.is_match_fw;
                searcher.is_match_fw = !searcher.is_match_fw;
                let pos = searcher.position;
                match self.haystack[pos..].chars().next() {
                    _ if is_match => SearchStep::Match(pos, pos),
                    None => SearchStep::Done,
                    Some(ch) => {
                        searcher.position += ch.len_utf8();
                        SearchStep::Reject(pos, searcher.position)
                    }
                }
            }
            StrSearcherImpl::TwoWay(ref mut searcher) => {
                // TwoWaySearcher produces valid *Match* indices that split at char boundaries
                // as long as it does correct matching and that haystack and needle are
                // valid UTF-8
                // *Rejects* from the algorithm can fall on any indices, but we will walk them
                // manually to the next character boundary, so that they are utf-8 safe.
                if searcher.position == self.haystack.len() {
                    return SearchStep::Done;
                }
                let is_long = searcher.memory == usize::MAX;
                match searcher.next::<RejectAndMatch>(self.haystack.as_bytes(),
                                                      self.needle.as_bytes(),
                                                      is_long)
                {
                    SearchStep::Reject(a, mut b) => {
                        // skip to next char boundary
                        while !self.haystack.is_char_boundary(b) {
                            b += 1;
                        }
                        searcher.position = cmp::max(b, searcher.position);
                        SearchStep::Reject(a, b)
                    }
                    otherwise => otherwise,
                }
            }
        }
    }

    #[inline(always)]
    fn next_match(&mut self) -> Option<(usize, usize)> {
        match self.searcher {
            StrSearcherImpl::Empty(..) => {
                loop {
                    match self.next() {
                        SearchStep::Match(a, b) => return Some((a, b)),
                        SearchStep::Done => return None,
                        SearchStep::Reject(..) => { }
                    }
                }
            }

            StrSearcherImpl::TwoWay(ref mut searcher) => {
                let is_long = searcher.memory == usize::MAX;
                // write out `true` and `false` cases to encourage the compiler
                // to specialize the two cases separately.
                if is_long {
                    searcher.next::<MatchOnly>(self.haystack.as_bytes(),
                                               self.needle.as_bytes(),
                                               true)
                } else {
                    searcher.next::<MatchOnly>(self.haystack.as_bytes(),
                                               self.needle.as_bytes(),
                                               false)
                }
            }
        }
    }
}

#[cfg(feature = "pattern")]
unsafe impl<'a, 'b> ReverseSearcher<'a> for StrSearcher<'a, 'b> {
    #[inline]
    fn next_back(&mut self) -> SearchStep {
        match self.searcher {
            StrSearcherImpl::Empty(ref mut searcher) => {
                let is_match = searcher.is_match_bw;
                searcher.is_match_bw = !searcher.is_match_bw;
                let end = searcher.end;
                match self.haystack[..end].chars().next_back() {
                    _ if is_match => SearchStep::Match(end, end),
                    None => SearchStep::Done,
                    Some(ch) => {
                        searcher.end -= ch.len_utf8();
                        SearchStep::Reject(searcher.end, end)
                    }
                }
            }
            StrSearcherImpl::TwoWay(ref mut searcher) => {
                if searcher.end == 0 {
                    return SearchStep::Done;
                }
                let is_long = searcher.memory == usize::MAX;
                match searcher.next_back::<RejectAndMatch>(self.haystack.as_bytes(),
                                                           self.needle.as_bytes(),
                                                           is_long)
                {
                    SearchStep::Reject(mut a, b) => {
                        // skip to next char boundary
                        while !self.haystack.is_char_boundary(a) {
                            a -= 1;
                        }
                        searcher.end = cmp::min(a, searcher.end);
                        SearchStep::Reject(a, b)
                    }
                    otherwise => otherwise,
                }
            }
        }
    }

    #[inline]
    fn next_match_back(&mut self) -> Option<(usize, usize)> {
        match self.searcher {
            StrSearcherImpl::Empty(..) => {
                loop {
                    match self.next_back() {
                        SearchStep::Match(a, b) => return Some((a, b)),
                        SearchStep::Done => return None,
                        SearchStep::Reject(..) => { }
                    }
                }
            }
            StrSearcherImpl::TwoWay(ref mut searcher) => {
                let is_long = searcher.memory == usize::MAX;
                // write out `true` and `false`, like `next_match`
                if is_long {
                    searcher.next_back::<MatchOnly>(self.haystack.as_bytes(),
                                                    self.needle.as_bytes(),
                                                    true)
                } else {
                    searcher.next_back::<MatchOnly>(self.haystack.as_bytes(),
                                                    self.needle.as_bytes(),
                                                    false)
                }
            }
        }
    }
}

/// The internal state of the two-way substring search algorithm.
#[derive(Clone, Debug)]
#[doc(hidden)]
pub struct TwoWaySearcher {
    // constants
    /// critical factorization index
    crit_pos: usize,
    /// critical factorization index for reversed needle
    crit_pos_back: usize,
    period: usize,
    /// `byteset` is an extension (not part of the two way algorithm);
    /// it's a 64-bit "fingerprint" where each set bit `j` corresponds
    /// to a (byte & 63) == j present in the needle.
    byteset: u64,

    // variables
    position: usize,
    end: usize,
    /// index into needle before which we have already matched
    memory: usize,
    /// index into needle after which we have already matched
    memory_back: usize,
}

/*
    This is the Two-Way search algorithm, which was introduced in the paper:
    Crochemore, M., Perrin, D., 1991, Two-way string-matching, Journal of the ACM 38(3):651-675.

    Here's some background information.

    A *word* is a string of symbols. The *length* of a word should be a familiar
    notion, and here we denote it for any word x by |x|.
    (We also allow for the possibility of the *empty word*, a word of length zero).

    If x is any non-empty word, then an integer p with 0 < p <= |x| is said to be a
    *period* for x iff for all i with 0 <= i <= |x| - p - 1, we have x[i] == x[i+p].
    For example, both 1 and 2 are periods for the string "aa". As another example,
    the only period of the string "abcd" is 4.

    We denote by period(x) the *smallest* period of x (provided that x is non-empty).
    This is always well-defined since every non-empty word x has at least one period,
    |x|. We sometimes call this *the period* of x.

    If u, v and x are words such that x = uv, where uv is the concatenation of u and
    v, then we say that (u, v) is a *factorization* of x.

    Let (u, v) be a factorization for a word x. Then if w is a non-empty word such
    that both of the following hold

      - either w is a suffix of u or u is a suffix of w
      - either w is a prefix of v or v is a prefix of w

    then w is said to be a *repetition* for the factorization (u, v).

    Just to unpack this, there are four possibilities here. Let w = "abc". Then we
    might have:

      - w is a suffix of u and w is a prefix of v. ex: ("lolabc", "abcde")
      - w is a suffix of u and v is a prefix of w. ex: ("lolabc", "ab")
      - u is a suffix of w and w is a prefix of v. ex: ("bc", "abchi")
      - u is a suffix of w and v is a prefix of w. ex: ("bc", "a")

    Note that the word vu is a repetition for any factorization (u,v) of x = uv,
    so every factorization has at least one repetition.

    If x is a string and (u, v) is a factorization for x, then a *local period* for
    (u, v) is an integer r such that there is some word w such that |w| = r and w is
    a repetition for (u, v).

    We denote by local_period(u, v) the smallest local period of (u, v). We sometimes
    call this *the local period* of (u, v). Provided that x = uv is non-empty, this
    is well-defined (because each non-empty word has at least one factorization, as
    noted above).

    It can be proven that the following is an equivalent definition of a local period
    for a factorization (u, v): any positive integer r such that x[i] == x[i+r] for
    all i such that |u| - r <= i <= |u| - 1 and such that both x[i] and x[i+r] are
    defined. (i.e. i > 0 and i + r < |x|).

    Using the above reformulation, it is easy to prove that

        1 <= local_period(u, v) <= period(uv)

    A factorization (u, v) of x such that local_period(u,v) = period(x) is called a
    *critical factorization*.

    The algorithm hinges on the following theorem, which is stated without proof:

    **Critical Factorization Theorem** Any word x has at least one critical
    factorization (u, v) such that |u| < period(x).

    The purpose of maximal_suffix is to find such a critical factorization.

    If the period is short, compute another factorization x = u' v' to use
    for reverse search, chosen instead so that |v'| < period(x).

*/
impl TwoWaySearcher {
    pub fn new(needle: &[u8], end: usize) -> TwoWaySearcher {
        let (crit_pos_false, period_false) = TwoWaySearcher::maximal_suffix(needle, false);
        let (crit_pos_true, period_true) = TwoWaySearcher::maximal_suffix(needle, true);

        let (crit_pos, period) =
            if crit_pos_false > crit_pos_true {
                (crit_pos_false, period_false)
            } else {
                (crit_pos_true, period_true)
            };

        // A particularly readable explanation of what's going on here can be found
        // in Crochemore and Rytter's book "Text Algorithms", ch 13. Specifically
        // see the code for "Algorithm CP" on p. 323.
        //
        // What's going on is we have some critical factorization (u, v) of the
        // needle, and we want to determine whether u is a suffix of
        // &v[..period]. If it is, we use "Algorithm CP1". Otherwise we use
        // "Algorithm CP2", which is optimized for when the period of the needle
        // is large.
        if &needle[..crit_pos] == &needle[period.. period + crit_pos] {
            // short period case -- the period is exact
            // compute a separate critical factorization for the reversed needle
            // x = u' v' where |v'| < period(x).
            //
            // This is sped up by the period being known already.
            // Note that a case like x = "acba" may be factored exactly forwards
            // (crit_pos = 1, period = 3) while being factored with approximate
            // period in reverse (crit_pos = 2, period = 2). We use the given
            // reverse factorization but keep the exact period.
            let crit_pos_back = needle.len() - cmp::max(
                TwoWaySearcher::reverse_maximal_suffix(needle, period, false),
                TwoWaySearcher::reverse_maximal_suffix(needle, period, true));

            TwoWaySearcher {
                crit_pos: crit_pos,
                crit_pos_back: crit_pos_back,
                period: period,
                byteset: Self::byteset_create(&needle[..period]),

                position: 0,
                end: end,
                memory: 0,
                memory_back: needle.len(),
            }
        } else {
            // long period case -- we have an approximation to the actual period,
            // and don't use memorization.
            //
            // Approximate the period by lower bound max(|u|, |v|) + 1.
            // The critical factorization is efficient to use for both forward and
            // reverse search.

            TwoWaySearcher {
                crit_pos: crit_pos,
                crit_pos_back: crit_pos,
                period: cmp::max(crit_pos, needle.len() - crit_pos) + 1,
                byteset: Self::byteset_create(needle),

                position: 0,
                end: end,
                memory: usize::MAX, // Dummy value to signify that the period is long
                memory_back: usize::MAX,
            }
        }
    }

    #[inline]
    fn byteset_create(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0, |a, &b| (1 << (b & 0x3f)) | a)
    }

    #[inline(always)]
    fn byteset_contains(&self, byte: u8) -> bool {
        (self.byteset >> ((byte & 0x3f) as usize)) & 1 != 0
    }

    // One of the main ideas of Two-Way is that we factorize the needle into
    // two halves, (u, v), and begin trying to find v in the haystack by scanning
    // left to right. If v matches, we try to match u by scanning right to left.
    // How far we can jump when we encounter a mismatch is all based on the fact
    // that (u, v) is a critical factorization for the needle.
    #[inline(always)]
    fn next<S>(&mut self, haystack: &[u8], needle: &[u8], long_period: bool)
        -> S::Output
        where S: TwoWayStrategy
    {
        // `next()` uses `self.position` as its cursor
        let old_pos = self.position;
        let needle_last = needle.len() - 1;
        'search: loop {
            // Check that we have room to search in
            // position + needle_last can not overflow if we assume slices
            // are bounded by isize's range.
            let tail_byte = match haystack.get(self.position + needle_last) {
                Some(&b) => b,
                None => {
                    self.position = haystack.len();
                    return S::rejecting(old_pos, self.position);
                }
            };

            if S::use_early_reject() && old_pos != self.position {
                return S::rejecting(old_pos, self.position);
            }

            // Quickly skip by large portions unrelated to our substring
            if !self.byteset_contains(tail_byte) {
                self.position += needle.len();
                if !long_period {
                    self.memory = 0;
                }
                continue 'search;
            }

            // See if the right part of the needle matches
            let start = if long_period { self.crit_pos }
                        else { cmp::max(self.crit_pos, self.memory) };
            for i in start..needle.len() {
                if needle[i] != haystack[self.position + i] {
                    self.position += i - self.crit_pos + 1;
                    if !long_period {
                        self.memory = 0;
                    }
                    continue 'search;
                }
            }

            // See if the left part of the needle matches
            let start = if long_period { 0 } else { self.memory };
            for i in (start..self.crit_pos).rev() {
                if needle[i] != haystack[self.position + i] {
                    self.position += self.period;
                    if !long_period {
                        self.memory = needle.len() - self.period;
                    }
                    continue 'search;
                }
            }

            // We have found a match!
            let match_pos = self.position;

            // Note: add self.period instead of needle.len() to have overlapping matches
            self.position += needle.len();
            if !long_period {
                self.memory = 0; // set to needle.len() - self.period for overlapping matches
            }

            return S::matching(match_pos, match_pos + needle.len());
        }
    }

    // Follows the ideas in `next()`.
    //
    // The definitions are symmetrical, with period(x) = period(reverse(x))
    // and local_period(u, v) = local_period(reverse(v), reverse(u)), so if (u, v)
    // is a critical factorization, so is (reverse(v), reverse(u)).
    //
    // For the reverse case we have computed a critical factorization x = u' v'
    // (field `crit_pos_back`). We need |u| < period(x) for the forward case and
    // thus |v'| < period(x) for the reverse.
    //
    // To search in reverse through the haystack, we search forward through
    // a reversed haystack with a reversed needle, matching first u' and then v'.
    #[inline]
    fn next_back<S>(&mut self, haystack: &[u8], needle: &[u8], long_period: bool)
        -> S::Output
        where S: TwoWayStrategy
    {
        // `next_back()` uses `self.end` as its cursor -- so that `next()` and `next_back()`
        // are independent.
        let old_end = self.end;
        'search: loop {
            // Check that we have room to search in
            // end - needle.len() will wrap around when there is no more room,
            // but due to slice length limits it can never wrap all the way back
            // into the length of haystack.
            let front_byte = match haystack.get(self.end.wrapping_sub(needle.len())) {
                Some(&b) => b,
                None => {
                    self.end = 0;
                    return S::rejecting(0, old_end);
                }
            };

            if S::use_early_reject() && old_end != self.end {
                return S::rejecting(self.end, old_end);
            }

            // Quickly skip by large portions unrelated to our substring
            if !self.byteset_contains(front_byte) {
                self.end -= needle.len();
                if !long_period {
                    self.memory_back = needle.len();
                }
                continue 'search;
            }

            // See if the left part of the needle matches
            let crit = if long_period { self.crit_pos_back }
                       else { cmp::min(self.crit_pos_back, self.memory_back) };
            for i in (0..crit).rev() {
                if needle[i] != haystack[self.end - needle.len() + i] {
                    self.end -= self.crit_pos_back - i;
                    if !long_period {
                        self.memory_back = needle.len();
                    }
                    continue 'search;
                }
            }

            // See if the right part of the needle matches
            let needle_end = if long_period { needle.len() }
                             else { self.memory_back };
            for i in self.crit_pos_back..needle_end {
                if needle[i] != haystack[self.end - needle.len() + i] {
                    self.end -= self.period;
                    if !long_period {
                        self.memory_back = self.period;
                    }
                    continue 'search;
                }
            }

            // We have found a match!
            let match_pos = self.end - needle.len();
            // Note: sub self.period instead of needle.len() to have overlapping matches
            self.end -= needle.len();
            if !long_period {
                self.memory_back = needle.len();
            }

            return S::matching(match_pos, match_pos + needle.len());
        }
    }

    // Compute the maximal suffix of `arr`.
    //
    // The maximal suffix is a possible critical factorization (u, v) of `arr`.
    //
    // Returns (`i`, `p`) where `i` is the starting index of v and `p` is the
    // period of v.
    //
    // `order_greater` determines if lexical order is `<` or `>`. Both
    // orders must be computed -- the ordering with the largest `i` gives
    // a critical factorization.
    //
    // For long period cases, the resulting period is not exact (it is too short).
    #[inline]
    pub fn maximal_suffix(arr: &[u8], order_greater: bool) -> (usize, usize) {
        let mut left = 0; // Corresponds to i in the paper
        let mut right = 1; // Corresponds to j in the paper
        let mut offset = 0; // Corresponds to k in the paper, but starting at 0
                            // to match 0-based indexing.
        let mut period = 1; // Corresponds to p in the paper

        while let Some(&a) = arr.get(right + offset) {
            // `left` will be inbounds when `right` is.
            let b = arr[left + offset];
            if (a < b && !order_greater) || (a > b && order_greater) {
                // Suffix is smaller, period is entire prefix so far.
                right += offset + 1;
                offset = 0;
                period = right - left;
            } else if a == b {
                // Advance through repetition of the current period.
                if offset + 1 == period {
                    right += offset + 1;
                    offset = 0;
                } else {
                    offset += 1;
                }
            } else {
                // Suffix is larger, start over from current location.
                left = right;
                right += 1;
                offset = 0;
                period = 1;
            }
        }
        (left, period)
    }

    // Compute the maximal suffix of the reverse of `arr`.
    //
    // The maximal suffix is a possible critical factorization (u', v') of `arr`.
    //
    // Returns `i` where `i` is the starting index of v', from the back;
    // returns immedately when a period of `known_period` is reached.
    //
    // `order_greater` determines if lexical order is `<` or `>`. Both
    // orders must be computed -- the ordering with the largest `i` gives
    // a critical factorization.
    //
    // For long period cases, the resulting period is not exact (it is too short).
    pub fn reverse_maximal_suffix(arr: &[u8], known_period: usize,
                                  order_greater: bool) -> usize
    {
        let mut left = 0; // Corresponds to i in the paper
        let mut right = 1; // Corresponds to j in the paper
        let mut offset = 0; // Corresponds to k in the paper, but starting at 0
                            // to match 0-based indexing.
        let mut period = 1; // Corresponds to p in the paper
        let n = arr.len();

        while right + offset < n {
            let a = arr[n - (1 + right + offset)];
            let b = arr[n - (1 + left + offset)];
            if (a < b && !order_greater) || (a > b && order_greater) {
                // Suffix is smaller, period is entire prefix so far.
                right += offset + 1;
                offset = 0;
                period = right - left;
            } else if a == b {
                // Advance through repetition of the current period.
                if offset + 1 == period {
                    right += offset + 1;
                    offset = 0;
                } else {
                    offset += 1;
                }
            } else {
                // Suffix is larger, start over from current location.
                left = right;
                right += 1;
                offset = 0;
                period = 1;
            }
            if period == known_period {
                break;
            }
        }
        debug_assert!(period <= known_period);
        left
    }
}

// TwoWayStrategy allows the algorithm to either skip non-matches as quickly
// as possible, or to work in a mode where it emits Rejects relatively quickly.
trait TwoWayStrategy {
    type Output;
    fn use_early_reject() -> bool;
    fn rejecting(usize, usize) -> Self::Output;
    fn matching(usize, usize) -> Self::Output;
}

/// Skip to match intervals as quickly as possible
enum MatchOnly { }

impl TwoWayStrategy for MatchOnly {
    type Output = Option<(usize, usize)>;

    #[inline]
    fn use_early_reject() -> bool { false }
    #[inline]
    fn rejecting(_a: usize, _b: usize) -> Self::Output { None }
    #[inline]
    fn matching(a: usize, b: usize) -> Self::Output { Some((a, b)) }
}

#[cfg(feature = "pattern")]
/// Emit Rejects regularly
enum RejectAndMatch { }

#[cfg(feature = "pattern")]
impl TwoWayStrategy for RejectAndMatch {
    type Output = SearchStep;

    #[inline]
    fn use_early_reject() -> bool { true }
    #[inline]
    fn rejecting(a: usize, b: usize) -> Self::Output { SearchStep::Reject(a, b) }
    #[inline]
    fn matching(a: usize, b: usize) -> Self::Output { SearchStep::Match(a, b) }
}


#[cfg(feature = "pattern")]
#[cfg(test)]
impl<'a, 'b> StrSearcher<'a, 'b> {
    fn twoway(&self) -> &TwoWaySearcher {
        match self.searcher {
            StrSearcherImpl::TwoWay(ref inner) => inner,
            _ => panic!("Not a TwoWaySearcher"),
        }
    }
}

#[cfg(feature = "pattern")]
#[test]
fn test_basic() {
    let t = StrSearcher::new("", "aab");
    println!("{:?}", t);
    let t = StrSearcher::new("", "abaaaba");
    println!("{:?}", t);
    let mut t = StrSearcher::new("GCATCGCAGAGAGTATACAGTACG", "GCAGAGAG");
    println!("{:?}", t);

    loop {
        match t.next() {
            SearchStep::Done => break,
            m => println!("{:?}", m),
        }
    }

    let mut t = StrSearcher::new("GCATCGCAGAGAGTATACAGTACG", "GCAGAGAG");
    println!("{:?}", t);

    loop {
        match t.next_back() {
            SearchStep::Done => break,
            m => println!("{:?}", m),
        }
    }

    let mut t = StrSearcher::new("banana", "nana");
    println!("{:?}", t);

    loop {
        match t.next() {
            SearchStep::Done => break,
            m => println!("{:?}", m),
        }
    }
}

#[cfg(feature = "pattern")]
#[cfg(test)]
fn contains(hay: &str, n: &str) -> bool {
    let mut tws = StrSearcher::new(hay, n);
    loop {
        match tws.next() {
            SearchStep::Done => return false,
            SearchStep::Match(..) => return true,
            _ => { }
        }
    }
}

#[cfg(feature = "pattern")]
#[cfg(test)]
fn contains_rev(hay: &str, n: &str) -> bool {
    let mut tws = StrSearcher::new(hay, n);
    loop {
        match tws.next_back() {
            SearchStep::Done => return false,
            SearchStep::Match(..) => return true,
            rej => { println!("{:?}", rej); }
        }
    }
}


#[cfg(feature = "pattern")]
#[test]
fn test_contains() {
    let h = "";
    let n = "";
    assert!(contains(h, n));
    assert!(contains_rev(h, n));

    let h = "BDC\0\0\0";
    let n = "BDC\u{0}";
    assert!(contains(h, n));
    assert!(contains_rev(h, n));


    let h = "ADA\0";
    let n = "ADA";
    assert!(contains(h, n));
    assert!(contains_rev(h, n));

    let h = "\u{0}\u{0}\u{0}\u{0}"; 
    let n = "\u{0}";
    assert!(contains(h, n));
    assert!(contains_rev(h, n));
}

#[cfg(feature = "pattern")]
#[test]
fn test_rev_2() {
    let h = "BDC\0\0\0";
    let n = "BDC\u{0}";
    let mut t = StrSearcher::new(h, n);
    println!("{:?}", t);
    println!("{:?}", h.contains(&n));

    loop {
        match t.next_back() {
            SearchStep::Done => break,
            m => println!("{:?}", m),
        }
    }

    let h = "aabaabx";
    let n = "aabaab";
    let mut t = StrSearcher::new(h, n);
    println!("{:?}", t);
    assert_eq!(t.twoway().crit_pos, 2);
    assert_eq!(t.twoway().crit_pos_back, 5);

    loop {
        match t.next_back() {
            SearchStep::Done => break,
            m => println!("{:?}", m),
        }
    }

    let h = "abababac";
    let n = "ababab";
    let mut t = StrSearcher::new(h, n);
    println!("{:?}", t);
    assert_eq!(t.twoway().crit_pos, 1);
    assert_eq!(t.twoway().crit_pos_back, 5);

    loop {
        match t.next_back() {
            SearchStep::Done => break,
            m => println!("{:?}", m),
        }
    }

    let h = "abababac";
    let n = "abab";
    let mut t = StrSearcher::new(h, n);
    println!("{:?}", t);

    loop {
        match t.next_back() {
            SearchStep::Done => break,
            m => println!("{:?}", m),
        }
    }

    let h = "baabbbaabc";
    let n = "baabb";
    let t = StrSearcher::new(h, n);
    println!("{:?}", t);
    assert_eq!(t.twoway().crit_pos, 3);
    assert_eq!(t.twoway().crit_pos_back, 3);

    let h = "aabaaaabaabxx";
    let n = "aabaaaabaa";
    let mut t = StrSearcher::new(h, n);
    println!("{:?}", t);

    loop {
        match t.next_back() {
            SearchStep::Done => break,
            m => println!("{:?}", m),
        }
    }

    let h = "babbabax";
    let n = "babbab";
    let mut t = StrSearcher::new(h, n);
    println!("{:?}", t);
    assert_eq!(t.twoway().crit_pos, 2);
    assert_eq!(t.twoway().crit_pos_back, 4);

    loop {
        match t.next_back() {
            SearchStep::Done => break,
            m => println!("{:?}", m),
        }
    }

    let h = "xacbaabcax";
    let n = "abca";
    let mut t = StrSearcher::new(h, n);
    assert_eq!(t.next_match_back(), Some((5, 9)));

    let h = "xacbaacbxxcba";
    let m = "acba";
    let mut s = StrSearcher::new(h, m);
    assert_eq!(s.next_match_back(), Some((1, 5)));
    assert_eq!(s.twoway().crit_pos, 1);
    assert_eq!(s.twoway().crit_pos_back, 2);
}

#[cfg(feature = "pattern")]
#[test]
fn test_rev_unicode() {
    let h = "ααααααβ";
    let n = "αβ";
    let mut t = StrSearcher::new(h, n);
    println!("{:?}", t);

    loop {
        match t.next() {
            SearchStep::Done => break,
            m => println!("{:?}", m),
        }
    }

    let mut t = StrSearcher::new(h, n);
    loop {
        match t.next_back() {
            SearchStep::Done => break,
            m => println!("{:?}", m),
        }
    }
}

#[test]
fn maximal_suffix() {
    assert_eq!((2, 1), TwoWaySearcher::maximal_suffix(b"aab", false));
    assert_eq!((0, 3), TwoWaySearcher::maximal_suffix(b"aab", true));

    assert_eq!((0, 3), TwoWaySearcher::maximal_suffix(b"aabaa", true));
    assert_eq!((2, 3), TwoWaySearcher::maximal_suffix(b"aabaa", false));

    assert_eq!((0, 7), TwoWaySearcher::maximal_suffix(b"gcagagag", false));
    assert_eq!((2, 2), TwoWaySearcher::maximal_suffix(b"gcagagag", true));

    // both of these factorizations are critial factorizations
    assert_eq!((2, 2), TwoWaySearcher::maximal_suffix(b"banana", false));
    assert_eq!((1, 2), TwoWaySearcher::maximal_suffix(b"banana", true));
    assert_eq!((0, 6), TwoWaySearcher::maximal_suffix(b"zanana", false));
    assert_eq!((1, 2), TwoWaySearcher::maximal_suffix(b"zanana", true));
}

#[test]
fn maximal_suffix_verbose() {
    fn maximal_suffix(arr: &[u8], order_greater: bool) -> (usize, usize) {
        let mut left: usize = 0; // Corresponds to i in the paper
        let mut right = 1; // Corresponds to j in the paper
        let mut offset = 0; // Corresponds to k in the paper
        let mut period = 1; // Corresponds to p in the paper

        macro_rules! asstr {
            ($e:expr) => (::std::str::from_utf8($e).unwrap())
        }

        while let Some(&a) = arr.get(right + offset) {
            // `left` will be inbounds when `right` is.
            debug_assert!(left <= right);
            let b = unsafe { *arr.get_unchecked(left + offset) };
            println!("str={}, l={}, r={}, offset={}, p={}", asstr!(arr), left, right, offset, period);
            if (a < b && !order_greater) || (a > b && order_greater) {
                // Suffix is smaller, period is entire prefix so far.
                right += offset + 1;
                offset = 0;
                period = right - left;
            } else if a == b {
                // Advance through repetition of the current period.
                if offset + 1 == period {
                    right += offset + 1;
                    offset = 0;
                } else {
                    offset += 1;
                }
            } else {
                // Suffix is larger, start over from current location.
                left = right;
                right += 1;
                offset = 0;
                period = 1;
            }
        }
        println!("str={}, l={}, r={}, offset={}, p={} ==END==", asstr!(arr), left, right, offset, period);
        (left, period)
    }

    fn reverse_maximal_suffix(arr: &[u8], known_period: usize, order_greater: bool) -> usize {
        let n = arr.len();
        let mut left: usize = 0; // Corresponds to i in the paper
        let mut right = 1; // Corresponds to j in the paper
        let mut offset = 0; // Corresponds to k in the paper
        let mut period = 1; // Corresponds to p in the paper

        macro_rules! asstr {
            ($e:expr) => (::std::str::from_utf8($e).unwrap())
        }

        while right + offset < n {
            // `left` will be inbounds when `right` is.
            debug_assert!(left <= right);
            let a = unsafe { *arr.get_unchecked(n - (1 + right + offset)) };
            let b = unsafe { *arr.get_unchecked(n - (1 + left + offset)) };
            println!("str={}, l={}, r={}, offset={}, p={}", asstr!(arr), left, right, offset, period);
            if (a < b && !order_greater) || (a > b && order_greater) {
                // Suffix is smaller, period is entire prefix so far.
                right += offset + 1;
                offset = 0;
                period = right - left;
                if period == known_period {
                    break;
                }
            } else if a == b {
                // Advance through repetition of the current period.
                if offset + 1 == period {
                    right += offset + 1;
                    offset = 0;
                } else {
                    offset += 1;
                }
            } else {
                // Suffix is larger, start over from current location.
                left = right;
                right += 1;
                offset = 0;
                period = 1;
            }
        }
        println!("str={}, l={}, r={}, offset={}, p={} ==END==", asstr!(arr), left, right, offset, period);
        debug_assert!(period == known_period);
        left
    }

    assert_eq!((2, 2), maximal_suffix(b"banana", false));
    assert_eq!((1, 2), maximal_suffix(b"banana", true));
    assert_eq!((0, 7), maximal_suffix(b"gcagagag", false));
    assert_eq!((2, 2), maximal_suffix(b"gcagagag", true));
    assert_eq!((2, 1), maximal_suffix(b"bac", false));
    assert_eq!((1, 2), maximal_suffix(b"bac", true));
    assert_eq!((0, 9), maximal_suffix(b"baaaaaaaa", false));
    assert_eq!((1, 1), maximal_suffix(b"baaaaaaaa", true));

    assert_eq!((2, 3), maximal_suffix(b"babbabbab", false));
    assert_eq!((1, 3), maximal_suffix(b"babbabbab", true));

    assert_eq!(2, reverse_maximal_suffix(b"babbabbab", 3, false));
    assert_eq!(1, reverse_maximal_suffix(b"babbabbab", 3, true));

    assert_eq!((0, 2), maximal_suffix(b"bababa", false));
    assert_eq!((1, 2), maximal_suffix(b"bababa", true));

    assert_eq!(1, reverse_maximal_suffix(b"bababa", 2, false));
    assert_eq!(0, reverse_maximal_suffix(b"bababa", 2, true));

    // NOTE: returns "long period" case per = 2, which is an approximation
    assert_eq!((2, 2), maximal_suffix(b"abca", false));
    assert_eq!((0, 3), maximal_suffix(b"abca", true));

    assert_eq!((3, 2), maximal_suffix(b"abcda", false));
    assert_eq!((0, 4), maximal_suffix(b"abcda", true));

    // "aöa"
    assert_eq!((1, 3), maximal_suffix(b"acba", false));
    assert_eq!((0, 3), maximal_suffix(b"acba", true));
    //assert_eq!(2, reverse_maximal_suffix(b"acba", 3, false));
    //assert_eq!(0, reverse_maximal_suffix(b"acba", 3, true));
}

#[cfg(feature = "pattern")]
#[test]
fn test_find_rfind() {
    fn find(hay: &str, pat: &str) -> Option<usize> {
        let mut t = pat.into_searcher(hay);
        t.next_match().map(|(x, _)| x)
    }

    fn rfind(hay: &str, pat: &str) -> Option<usize> {
        let mut t = pat.into_searcher(hay);
        t.next_match_back().map(|(x, _)| x)
    }

    // find every substring -- assert that it finds it, or an earlier occurence.
    let string = "Việt Namacbaabcaabaaba";
    for (i, ci) in string.char_indices() {
        let ip = i + ci.len_utf8();
        for j in string[ip..].char_indices()
                             .map(|(i, _)| i)
                             .chain(Some(string.len() - ip))
        {
            let pat = &string[i..ip + j];
            assert!(match find(string, pat) {
                None => false,
                Some(x) => x <= i,
            });
            assert!(match rfind(string, pat) {
                None => false,
                Some(x) => x >= i,
            });
        }
    }
}
