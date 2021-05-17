use std::cmp;
use std::mem;

use aho_corasick::{self, packed, AhoCorasick, AhoCorasickBuilder};
use memchr::{memchr, memchr2, memchr3};
use syntax::hir::literal::{Literal, Literals};

use freqs::BYTE_FREQUENCIES;

/// A prefix extracted from a compiled regular expression.
///
/// A regex prefix is a set of literal strings that *must* be matched at the
/// beginning of a regex in order for the entire regex to match. Similarly
/// for a regex suffix.
#[derive(Clone, Debug)]
pub struct LiteralSearcher {
    complete: bool,
    lcp: FreqyPacked,
    lcs: FreqyPacked,
    matcher: Matcher,
}

#[derive(Clone, Debug)]
enum Matcher {
    /// No literals. (Never advances through the input.)
    Empty,
    /// A set of four or more single byte literals.
    Bytes(SingleByteSet),
    /// A single substring, find using memchr and frequency analysis.
    FreqyPacked(FreqyPacked),
    /// A single substring, find using Boyer-Moore.
    BoyerMoore(BoyerMooreSearch),
    /// An Aho-Corasick automaton.
    AC { ac: AhoCorasick<u32>, lits: Vec<Literal> },
    /// A packed multiple substring searcher, using SIMD.
    ///
    /// Note that Aho-Corasick will actually use this packed searcher
    /// internally automatically, however, there is some overhead associated
    /// with going through the Aho-Corasick machinery. So using the packed
    /// searcher directly results in some gains.
    Packed { s: packed::Searcher, lits: Vec<Literal> },
}

impl LiteralSearcher {
    /// Returns a matcher that never matches and never advances the input.
    pub fn empty() -> Self {
        Self::new(Literals::empty(), Matcher::Empty)
    }

    /// Returns a matcher for literal prefixes from the given set.
    pub fn prefixes(lits: Literals) -> Self {
        let matcher = Matcher::prefixes(&lits);
        Self::new(lits, matcher)
    }

    /// Returns a matcher for literal suffixes from the given set.
    pub fn suffixes(lits: Literals) -> Self {
        let matcher = Matcher::suffixes(&lits);
        Self::new(lits, matcher)
    }

    fn new(lits: Literals, matcher: Matcher) -> Self {
        let complete = lits.all_complete();
        LiteralSearcher {
            complete: complete,
            lcp: FreqyPacked::new(lits.longest_common_prefix().to_vec()),
            lcs: FreqyPacked::new(lits.longest_common_suffix().to_vec()),
            matcher: matcher,
        }
    }

    /// Returns true if all matches comprise the entire regular expression.
    ///
    /// This does not necessarily mean that a literal match implies a match
    /// of the regular expression. For example, the regular expression `^a`
    /// is comprised of a single complete literal `a`, but the regular
    /// expression demands that it only match at the beginning of a string.
    pub fn complete(&self) -> bool {
        self.complete && !self.is_empty()
    }

    /// Find the position of a literal in `haystack` if it exists.
    #[cfg_attr(feature = "perf-inline", inline(always))]
    pub fn find(&self, haystack: &[u8]) -> Option<(usize, usize)> {
        use self::Matcher::*;
        match self.matcher {
            Empty => Some((0, 0)),
            Bytes(ref sset) => sset.find(haystack).map(|i| (i, i + 1)),
            FreqyPacked(ref s) => s.find(haystack).map(|i| (i, i + s.len())),
            BoyerMoore(ref s) => s.find(haystack).map(|i| (i, i + s.len())),
            AC { ref ac, .. } => {
                ac.find(haystack).map(|m| (m.start(), m.end()))
            }
            Packed { ref s, .. } => {
                s.find(haystack).map(|m| (m.start(), m.end()))
            }
        }
    }

    /// Like find, except matches must start at index `0`.
    pub fn find_start(&self, haystack: &[u8]) -> Option<(usize, usize)> {
        for lit in self.iter() {
            if lit.len() > haystack.len() {
                continue;
            }
            if lit == &haystack[0..lit.len()] {
                return Some((0, lit.len()));
            }
        }
        None
    }

    /// Like find, except matches must end at index `haystack.len()`.
    pub fn find_end(&self, haystack: &[u8]) -> Option<(usize, usize)> {
        for lit in self.iter() {
            if lit.len() > haystack.len() {
                continue;
            }
            if lit == &haystack[haystack.len() - lit.len()..] {
                return Some((haystack.len() - lit.len(), haystack.len()));
            }
        }
        None
    }

    /// Returns an iterator over all literals to be matched.
    pub fn iter(&self) -> LiteralIter {
        match self.matcher {
            Matcher::Empty => LiteralIter::Empty,
            Matcher::Bytes(ref sset) => LiteralIter::Bytes(&sset.dense),
            Matcher::FreqyPacked(ref s) => LiteralIter::Single(&s.pat),
            Matcher::BoyerMoore(ref s) => LiteralIter::Single(&s.pattern),
            Matcher::AC { ref lits, .. } => LiteralIter::AC(lits),
            Matcher::Packed { ref lits, .. } => LiteralIter::Packed(lits),
        }
    }

    /// Returns a matcher for the longest common prefix of this matcher.
    pub fn lcp(&self) -> &FreqyPacked {
        &self.lcp
    }

    /// Returns a matcher for the longest common suffix of this matcher.
    pub fn lcs(&self) -> &FreqyPacked {
        &self.lcs
    }

    /// Returns true iff this prefix is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the number of prefixes in this machine.
    pub fn len(&self) -> usize {
        use self::Matcher::*;
        match self.matcher {
            Empty => 0,
            Bytes(ref sset) => sset.dense.len(),
            FreqyPacked(_) => 1,
            BoyerMoore(_) => 1,
            AC { ref ac, .. } => ac.pattern_count(),
            Packed { ref lits, .. } => lits.len(),
        }
    }

    /// Return the approximate heap usage of literals in bytes.
    pub fn approximate_size(&self) -> usize {
        use self::Matcher::*;
        match self.matcher {
            Empty => 0,
            Bytes(ref sset) => sset.approximate_size(),
            FreqyPacked(ref single) => single.approximate_size(),
            BoyerMoore(ref single) => single.approximate_size(),
            AC { ref ac, .. } => ac.heap_bytes(),
            Packed { ref s, .. } => s.heap_bytes(),
        }
    }
}

impl Matcher {
    fn prefixes(lits: &Literals) -> Self {
        let sset = SingleByteSet::prefixes(lits);
        Matcher::new(lits, sset)
    }

    fn suffixes(lits: &Literals) -> Self {
        let sset = SingleByteSet::suffixes(lits);
        Matcher::new(lits, sset)
    }

    fn new(lits: &Literals, sset: SingleByteSet) -> Self {
        if lits.literals().is_empty() {
            return Matcher::Empty;
        }
        if sset.dense.len() >= 26 {
            // Avoid trying to match a large number of single bytes.
            // This is *very* sensitive to a frequency analysis comparison
            // between the bytes in sset and the composition of the haystack.
            // No matter the size of sset, if its members all are rare in the
            // haystack, then it'd be worth using it. How to tune this... IDK.
            // ---AG
            return Matcher::Empty;
        }
        if sset.complete {
            return Matcher::Bytes(sset);
        }
        if lits.literals().len() == 1 {
            let lit = lits.literals()[0].to_vec();
            if BoyerMooreSearch::should_use(lit.as_slice()) {
                return Matcher::BoyerMoore(BoyerMooreSearch::new(lit));
            } else {
                return Matcher::FreqyPacked(FreqyPacked::new(lit));
            }
        }

        let pats = lits.literals().to_owned();
        let is_aho_corasick_fast = sset.dense.len() <= 1 && sset.all_ascii;
        if lits.literals().len() <= 100 && !is_aho_corasick_fast {
            let mut builder = packed::Config::new()
                .match_kind(packed::MatchKind::LeftmostFirst)
                .builder();
            if let Some(s) = builder.extend(&pats).build() {
                return Matcher::Packed { s, lits: pats };
            }
        }
        let ac = AhoCorasickBuilder::new()
            .match_kind(aho_corasick::MatchKind::LeftmostFirst)
            .dfa(true)
            .build_with_size::<u32, _, _>(&pats)
            .unwrap();
        Matcher::AC { ac, lits: pats }
    }
}

#[derive(Debug)]
pub enum LiteralIter<'a> {
    Empty,
    Bytes(&'a [u8]),
    Single(&'a [u8]),
    AC(&'a [Literal]),
    Packed(&'a [Literal]),
}

impl<'a> Iterator for LiteralIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        match *self {
            LiteralIter::Empty => None,
            LiteralIter::Bytes(ref mut many) => {
                if many.is_empty() {
                    None
                } else {
                    let next = &many[0..1];
                    *many = &many[1..];
                    Some(next)
                }
            }
            LiteralIter::Single(ref mut one) => {
                if one.is_empty() {
                    None
                } else {
                    let next = &one[..];
                    *one = &[];
                    Some(next)
                }
            }
            LiteralIter::AC(ref mut lits) => {
                if lits.is_empty() {
                    None
                } else {
                    let next = &lits[0];
                    *lits = &lits[1..];
                    Some(&**next)
                }
            }
            LiteralIter::Packed(ref mut lits) => {
                if lits.is_empty() {
                    None
                } else {
                    let next = &lits[0];
                    *lits = &lits[1..];
                    Some(&**next)
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct SingleByteSet {
    sparse: Vec<bool>,
    dense: Vec<u8>,
    complete: bool,
    all_ascii: bool,
}

impl SingleByteSet {
    fn new() -> SingleByteSet {
        SingleByteSet {
            sparse: vec![false; 256],
            dense: vec![],
            complete: true,
            all_ascii: true,
        }
    }

    fn prefixes(lits: &Literals) -> SingleByteSet {
        let mut sset = SingleByteSet::new();
        for lit in lits.literals() {
            sset.complete = sset.complete && lit.len() == 1;
            if let Some(&b) = lit.get(0) {
                if !sset.sparse[b as usize] {
                    if b > 0x7F {
                        sset.all_ascii = false;
                    }
                    sset.dense.push(b);
                    sset.sparse[b as usize] = true;
                }
            }
        }
        sset
    }

    fn suffixes(lits: &Literals) -> SingleByteSet {
        let mut sset = SingleByteSet::new();
        for lit in lits.literals() {
            sset.complete = sset.complete && lit.len() == 1;
            if let Some(&b) = lit.get(lit.len().checked_sub(1).unwrap()) {
                if !sset.sparse[b as usize] {
                    if b > 0x7F {
                        sset.all_ascii = false;
                    }
                    sset.dense.push(b);
                    sset.sparse[b as usize] = true;
                }
            }
        }
        sset
    }

    /// Faster find that special cases certain sizes to use memchr.
    #[cfg_attr(feature = "perf-inline", inline(always))]
    fn find(&self, text: &[u8]) -> Option<usize> {
        match self.dense.len() {
            0 => None,
            1 => memchr(self.dense[0], text),
            2 => memchr2(self.dense[0], self.dense[1], text),
            3 => memchr3(self.dense[0], self.dense[1], self.dense[2], text),
            _ => self._find(text),
        }
    }

    /// Generic find that works on any sized set.
    fn _find(&self, haystack: &[u8]) -> Option<usize> {
        for (i, &b) in haystack.iter().enumerate() {
            if self.sparse[b as usize] {
                return Some(i);
            }
        }
        None
    }

    fn approximate_size(&self) -> usize {
        (self.dense.len() * mem::size_of::<u8>())
            + (self.sparse.len() * mem::size_of::<bool>())
    }
}

/// Provides an implementation of fast subtring search using frequency
/// analysis.
///
/// memchr is so fast that we do everything we can to keep the loop in memchr
/// for as long as possible. The easiest way to do this is to intelligently
/// pick the byte to send to memchr. The best byte is the byte that occurs
/// least frequently in the haystack. Since doing frequency analysis on the
/// haystack is far too expensive, we compute a set of fixed frequencies up
/// front and hard code them in src/freqs.rs. Frequency analysis is done via
/// scripts/frequencies.py.
#[derive(Clone, Debug)]
pub struct FreqyPacked {
    /// The pattern.
    pat: Vec<u8>,
    /// The number of Unicode characters in the pattern. This is useful for
    /// determining the effective length of a pattern when deciding which
    /// optimizations to perform. A trailing incomplete UTF-8 sequence counts
    /// as one character.
    char_len: usize,
    /// The rarest byte in the pattern, according to pre-computed frequency
    /// analysis.
    rare1: u8,
    /// The offset of the rarest byte in `pat`.
    rare1i: usize,
    /// The second rarest byte in the pattern, according to pre-computed
    /// frequency analysis. (This may be equivalent to the rarest byte.)
    ///
    /// The second rarest byte is used as a type of guard for quickly detecting
    /// a mismatch after memchr locates an instance of the rarest byte. This
    /// is a hedge against pathological cases where the pre-computed frequency
    /// analysis may be off. (But of course, does not prevent *all*
    /// pathological cases.)
    rare2: u8,
    /// The offset of the second rarest byte in `pat`.
    rare2i: usize,
}

impl FreqyPacked {
    fn new(pat: Vec<u8>) -> FreqyPacked {
        if pat.is_empty() {
            return FreqyPacked::empty();
        }

        // Find the rarest two bytes. Try to make them distinct (but it's not
        // required).
        let mut rare1 = pat[0];
        let mut rare2 = pat[0];
        for b in pat[1..].iter().cloned() {
            if freq_rank(b) < freq_rank(rare1) {
                rare1 = b;
            }
        }
        for &b in &pat {
            if rare1 == rare2 {
                rare2 = b
            } else if b != rare1 && freq_rank(b) < freq_rank(rare2) {
                rare2 = b;
            }
        }

        // And find the offsets of their last occurrences.
        let rare1i = pat.iter().rposition(|&b| b == rare1).unwrap();
        let rare2i = pat.iter().rposition(|&b| b == rare2).unwrap();

        let char_len = char_len_lossy(&pat);
        FreqyPacked {
            pat: pat,
            char_len: char_len,
            rare1: rare1,
            rare1i: rare1i,
            rare2: rare2,
            rare2i: rare2i,
        }
    }

    fn empty() -> FreqyPacked {
        FreqyPacked {
            pat: vec![],
            char_len: 0,
            rare1: 0,
            rare1i: 0,
            rare2: 0,
            rare2i: 0,
        }
    }

    #[cfg_attr(feature = "perf-inline", inline(always))]
    pub fn find(&self, haystack: &[u8]) -> Option<usize> {
        let pat = &*self.pat;
        if haystack.len() < pat.len() || pat.is_empty() {
            return None;
        }
        let mut i = self.rare1i;
        while i < haystack.len() {
            i += match memchr(self.rare1, &haystack[i..]) {
                None => return None,
                Some(i) => i,
            };
            let start = i - self.rare1i;
            let end = start + pat.len();
            if end > haystack.len() {
                return None;
            }
            let aligned = &haystack[start..end];
            if aligned[self.rare2i] == self.rare2 && aligned == &*self.pat {
                return Some(start);
            }
            i += 1;
        }
        None
    }

    #[cfg_attr(feature = "perf-inline", inline(always))]
    pub fn is_suffix(&self, text: &[u8]) -> bool {
        if text.len() < self.len() {
            return false;
        }
        text[text.len() - self.len()..] == *self.pat
    }

    pub fn len(&self) -> usize {
        self.pat.len()
    }

    pub fn char_len(&self) -> usize {
        self.char_len
    }

    fn approximate_size(&self) -> usize {
        self.pat.len() * mem::size_of::<u8>()
    }
}

fn char_len_lossy(bytes: &[u8]) -> usize {
    String::from_utf8_lossy(bytes).chars().count()
}

/// An implementation of Tuned Boyer-Moore as laid out by
/// Andrew Hume and Daniel Sunday in "Fast String Searching".
/// O(n) in the size of the input.
///
/// Fast string searching algorithms come in many variations,
/// but they can generally be described in terms of three main
/// components.
///
/// The skip loop is where the string searcher wants to spend
/// as much time as possible. Exactly which character in the
/// pattern the skip loop examines varies from algorithm to
/// algorithm, but in the simplest case this loop repeated
/// looks at the last character in the pattern and jumps
/// forward in the input if it is not in the pattern.
/// Robert Boyer and J Moore called this the "fast" loop in
/// their original paper.
///
/// The match loop is responsible for actually examining the
/// whole potentially matching substring. In order to fail
/// faster, the match loop sometimes has a guard test attached.
/// The guard test uses frequency analysis of the different
/// characters in the pattern to choose the least frequency
/// occurring character and use it to find match failures
/// as quickly as possible.
///
/// The shift rule governs how the algorithm will shuffle its
/// test window in the event of a failure during the match loop.
/// Certain shift rules allow the worst-case run time of the
/// algorithm to be shown to be O(n) in the size of the input
/// rather than O(nm) in the size of the input and the size
/// of the pattern (as naive Boyer-Moore is).
///
/// "Fast String Searching", in addition to presenting a tuned
/// algorithm, provides a comprehensive taxonomy of the many
/// different flavors of string searchers. Under that taxonomy
/// TBM, the algorithm implemented here, uses an unrolled fast
/// skip loop with memchr fallback, a forward match loop with guard,
/// and the mini Sunday's delta shift rule. To unpack that you'll have to
/// read the paper.
#[derive(Clone, Debug)]
pub struct BoyerMooreSearch {
    /// The pattern we are going to look for in the haystack.
    pattern: Vec<u8>,

    /// The skip table for the skip loop.
    ///
    /// Maps the character at the end of the input
    /// to a shift.
    skip_table: Vec<usize>,

    /// The guard character (least frequently occurring char).
    guard: u8,
    /// The reverse-index of the guard character in the pattern.
    guard_reverse_idx: usize,

    /// Daniel Sunday's mini generalized delta2 shift table.
    ///
    /// We use a skip loop, so we only have to provide a shift
    /// for the skip char (last char). This is why it is a mini
    /// shift rule.
    md2_shift: usize,
}

impl BoyerMooreSearch {
    /// Create a new string searcher, performing whatever
    /// compilation steps are required.
    fn new(pattern: Vec<u8>) -> Self {
        debug_assert!(!pattern.is_empty());

        let (g, gi) = Self::select_guard(pattern.as_slice());
        let skip_table = Self::compile_skip_table(pattern.as_slice());
        let md2_shift = Self::compile_md2_shift(pattern.as_slice());
        BoyerMooreSearch {
            pattern: pattern,
            skip_table: skip_table,
            guard: g,
            guard_reverse_idx: gi,
            md2_shift: md2_shift,
        }
    }

    /// Find the pattern in `haystack`, returning the offset
    /// of the start of the first occurrence of the pattern
    /// in `haystack`.
    #[inline]
    fn find(&self, haystack: &[u8]) -> Option<usize> {
        if haystack.len() < self.pattern.len() {
            return None;
        }

        let mut window_end = self.pattern.len() - 1;

        // Inspired by the grep source. It is a way
        // to do correct loop unrolling without having to place
        // a crashpad of terminating charicters at the end in
        // the way described in the Fast String Searching paper.
        const NUM_UNROLL: usize = 10;
        // 1 for the initial position, and 1 for the md2 shift
        let short_circut = (NUM_UNROLL + 2) * self.pattern.len();

        if haystack.len() > short_circut {
            // just 1 for the md2 shift
            let backstop =
                haystack.len() - ((NUM_UNROLL + 1) * self.pattern.len());
            loop {
                window_end =
                    match self.skip_loop(haystack, window_end, backstop) {
                        Some(i) => i,
                        None => return None,
                    };
                if window_end >= backstop {
                    break;
                }

                if self.check_match(haystack, window_end) {
                    return Some(window_end - (self.pattern.len() - 1));
                } else {
                    let skip = self.skip_table[haystack[window_end] as usize];
                    window_end +=
                        if skip == 0 { self.md2_shift } else { skip };
                    continue;
                }
            }
        }

        // now process the input after the backstop
        while window_end < haystack.len() {
            let mut skip = self.skip_table[haystack[window_end] as usize];
            if skip == 0 {
                if self.check_match(haystack, window_end) {
                    return Some(window_end - (self.pattern.len() - 1));
                } else {
                    skip = self.md2_shift;
                }
            }
            window_end += skip;
        }

        None
    }

    fn len(&self) -> usize {
        return self.pattern.len();
    }

    /// The key heuristic behind which the BoyerMooreSearch lives.
    ///
    /// See `rust-lang/regex/issues/408`.
    ///
    /// Tuned Boyer-Moore is actually pretty slow! It turns out a handrolled
    /// platform-specific memchr routine with a bit of frequency
    /// analysis sprinkled on top actually wins most of the time.
    /// However, there are a few cases where Tuned Boyer-Moore still
    /// wins.
    ///
    /// If the haystack is random, frequency analysis doesn't help us,
    /// so Boyer-Moore will win for sufficiently large needles.
    /// Unfortunately, there is no obvious way to determine this
    /// ahead of time.
    ///
    /// If the pattern itself consists of very common characters,
    /// frequency analysis won't get us anywhere. The most extreme
    /// example of this is a pattern like `eeeeeeeeeeeeeeee`. Fortunately,
    /// this case is wholly determined by the pattern, so we can actually
    /// implement the heuristic.
    ///
    /// A third case is if the pattern is sufficiently long. The idea
    /// here is that once the pattern gets long enough the Tuned
    /// Boyer-Moore skip loop will start making strides long enough
    /// to beat the asm deep magic that is memchr.
    fn should_use(pattern: &[u8]) -> bool {
        // The minimum pattern length required to use TBM.
        const MIN_LEN: usize = 9;
        // The minimum frequency rank (lower is rarer) that every byte in the
        // pattern must have in order to use TBM. That is, if the pattern
        // contains _any_ byte with a lower rank, then TBM won't be used.
        const MIN_CUTOFF: usize = 150;
        // The maximum frequency rank for any byte.
        const MAX_CUTOFF: usize = 255;
        // The scaling factor used to determine the actual cutoff frequency
        // to use (keeping in mind that the minimum frequency rank is bounded
        // by MIN_CUTOFF). This scaling factor is an attempt to make TBM more
        // likely to be used as the pattern grows longer. That is, longer
        // patterns permit somewhat less frequent bytes than shorter patterns,
        // under the assumption that TBM gets better as the pattern gets
        // longer.
        const LEN_CUTOFF_PROPORTION: usize = 4;

        let scaled_rank = pattern.len().wrapping_mul(LEN_CUTOFF_PROPORTION);
        let cutoff = cmp::max(
            MIN_CUTOFF,
            MAX_CUTOFF - cmp::min(MAX_CUTOFF, scaled_rank),
        );
        // The pattern must be long enough to be worthwhile. e.g., memchr will
        // be faster on `e` because it is short even though e is quite common.
        pattern.len() > MIN_LEN
            // all the bytes must be more common than the cutoff.
            && pattern.iter().all(|c| freq_rank(*c) >= cutoff)
    }

    /// Check to see if there is a match at the given position
    #[inline]
    fn check_match(&self, haystack: &[u8], window_end: usize) -> bool {
        // guard test
        if haystack[window_end - self.guard_reverse_idx] != self.guard {
            return false;
        }

        // match loop
        let window_start = window_end - (self.pattern.len() - 1);
        for i in 0..self.pattern.len() {
            if self.pattern[i] != haystack[window_start + i] {
                return false;
            }
        }

        true
    }

    /// Skip forward according to the shift table.
    ///
    /// Returns the offset of the next occurrence
    /// of the last char in the pattern, or the none
    /// if it never reappears. If `skip_loop` hits the backstop
    /// it will leave early.
    #[inline]
    fn skip_loop(
        &self,
        haystack: &[u8],
        mut window_end: usize,
        backstop: usize,
    ) -> Option<usize> {
        let window_end_snapshot = window_end;
        let skip_of = |we: usize| -> usize {
            // Unsafe might make this faster, but the benchmarks
            // were hard to interpret.
            self.skip_table[haystack[we] as usize]
        };

        loop {
            let mut skip = skip_of(window_end);
            window_end += skip;
            skip = skip_of(window_end);
            window_end += skip;
            if skip != 0 {
                skip = skip_of(window_end);
                window_end += skip;
                skip = skip_of(window_end);
                window_end += skip;
                skip = skip_of(window_end);
                window_end += skip;
                if skip != 0 {
                    skip = skip_of(window_end);
                    window_end += skip;
                    skip = skip_of(window_end);
                    window_end += skip;
                    skip = skip_of(window_end);
                    window_end += skip;
                    if skip != 0 {
                        skip = skip_of(window_end);
                        window_end += skip;
                        skip = skip_of(window_end);
                        window_end += skip;

                        // If ten iterations did not make at least 16 words
                        // worth of progress, we just fall back on memchr.
                        if window_end - window_end_snapshot
                            > 16 * mem::size_of::<usize>()
                        {
                            // Returning a window_end >= backstop will
                            // immediatly break us out of the inner loop in
                            // `find`.
                            if window_end >= backstop {
                                return Some(window_end);
                            }

                            continue; // we made enough progress
                        } else {
                            // In case we are already there, and so that
                            // we will catch the guard char.
                            window_end = window_end
                                .checked_sub(1 + self.guard_reverse_idx)
                                .unwrap_or(0);

                            match memchr(self.guard, &haystack[window_end..]) {
                                None => return None,
                                Some(g_idx) => {
                                    return Some(
                                        window_end
                                            + g_idx
                                            + self.guard_reverse_idx,
                                    );
                                }
                            }
                        }
                    }
                }
            }

            return Some(window_end);
        }
    }

    /// Compute the ufast skip table.
    fn compile_skip_table(pattern: &[u8]) -> Vec<usize> {
        let mut tab = vec![pattern.len(); 256];

        // For every char in the pattern, we write a skip
        // that will line us up with the rightmost occurrence.
        //
        // N.B. the sentinel (0) is written by the last
        // loop iteration.
        for (i, c) in pattern.iter().enumerate() {
            tab[*c as usize] = (pattern.len() - 1) - i;
        }

        tab
    }

    /// Select the guard character based off of the precomputed
    /// frequency table.
    fn select_guard(pattern: &[u8]) -> (u8, usize) {
        let mut rarest = pattern[0];
        let mut rarest_rev_idx = pattern.len() - 1;
        for (i, c) in pattern.iter().enumerate() {
            if freq_rank(*c) < freq_rank(rarest) {
                rarest = *c;
                rarest_rev_idx = (pattern.len() - 1) - i;
            }
        }

        (rarest, rarest_rev_idx)
    }

    /// If there is another occurrence of the skip
    /// char, shift to it, otherwise just shift to
    /// the next window.
    fn compile_md2_shift(pattern: &[u8]) -> usize {
        let shiftc = *pattern.last().unwrap();

        // For a pattern of length 1 we will never apply the
        // shift rule, so we use a poison value on the principle
        // that failing fast is a good thing.
        if pattern.len() == 1 {
            return 0xDEADBEAF;
        }

        let mut i = pattern.len() - 2;
        while i > 0 {
            if pattern[i] == shiftc {
                return (pattern.len() - 1) - i;
            }
            i -= 1;
        }

        // The skip char never re-occurs in the pattern, so
        // we can just shift the whole window length.
        pattern.len() - 1
    }

    fn approximate_size(&self) -> usize {
        (self.pattern.len() * mem::size_of::<u8>())
            + (256 * mem::size_of::<usize>()) // skip table
    }
}

fn freq_rank(b: u8) -> usize {
    BYTE_FREQUENCIES[b as usize] as usize
}

#[cfg(test)]
mod tests {
    use super::{BoyerMooreSearch, FreqyPacked};

    //
    // Unit Tests
    //

    // The "hello, world" of string searching
    #[test]
    fn bm_find_subs() {
        let searcher = BoyerMooreSearch::new(Vec::from(&b"pattern"[..]));
        let haystack = b"I keep seeing patterns in this text";
        assert_eq!(14, searcher.find(haystack).unwrap());
    }

    #[test]
    fn bm_find_no_subs() {
        let searcher = BoyerMooreSearch::new(Vec::from(&b"pattern"[..]));
        let haystack = b"I keep seeing needles in this text";
        assert_eq!(None, searcher.find(haystack));
    }

    //
    // Regression Tests
    //

    #[test]
    fn bm_skip_reset_bug() {
        let haystack = vec![0, 0, 0, 0, 0, 1, 1, 0];
        let needle = vec![0, 1, 1, 0];

        let searcher = BoyerMooreSearch::new(needle);
        let offset = searcher.find(haystack.as_slice()).unwrap();
        assert_eq!(4, offset);
    }

    #[test]
    fn bm_backstop_underflow_bug() {
        let haystack = vec![0, 0];
        let needle = vec![0, 0];

        let searcher = BoyerMooreSearch::new(needle);
        let offset = searcher.find(haystack.as_slice()).unwrap();
        assert_eq!(0, offset);
    }

    #[test]
    fn bm_naive_off_by_one_bug() {
        let haystack = vec![91];
        let needle = vec![91];

        let naive_offset = naive_find(&needle, &haystack).unwrap();
        assert_eq!(0, naive_offset);
    }

    #[test]
    fn bm_memchr_fallback_indexing_bug() {
        let mut haystack = vec![
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 87, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let needle = vec![1, 1, 1, 1, 32, 32, 87];
        let needle_start = haystack.len();
        haystack.extend(needle.clone());

        let searcher = BoyerMooreSearch::new(needle);
        assert_eq!(needle_start, searcher.find(haystack.as_slice()).unwrap());
    }

    #[test]
    fn bm_backstop_boundary() {
        let haystack = b"\
// aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
e_data.clone_created(entity_id, entity_to_add.entity_id);
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
"
        .to_vec();
        let needle = b"clone_created".to_vec();

        let searcher = BoyerMooreSearch::new(needle);
        let result = searcher.find(&haystack);
        assert_eq!(Some(43), result);
    }

    #[test]
    fn bm_win_gnu_indexing_bug() {
        let haystack_raw = vec![
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let needle = vec![1, 1, 1, 1, 1, 1, 1];
        let haystack = haystack_raw.as_slice();

        BoyerMooreSearch::new(needle.clone()).find(haystack);
    }

    //
    // QuickCheck Properties
    //

    use quickcheck::TestResult;

    fn naive_find(needle: &[u8], haystack: &[u8]) -> Option<usize> {
        assert!(needle.len() <= haystack.len());

        for i in 0..(haystack.len() - (needle.len() - 1)) {
            if haystack[i] == needle[0]
                && &haystack[i..(i + needle.len())] == needle
            {
                return Some(i);
            }
        }

        None
    }

    quickcheck! {
        fn qc_bm_equals_nieve_find(pile1: Vec<u8>, pile2: Vec<u8>) -> TestResult {
            if pile1.len() == 0 || pile2.len() == 0 {
                return TestResult::discard();
            }

            let (needle, haystack) = if pile1.len() < pile2.len() {
                (pile1, pile2.as_slice())
            } else {
                (pile2, pile1.as_slice())
            };

            let searcher = BoyerMooreSearch::new(needle.clone());
            TestResult::from_bool(
                searcher.find(haystack) == naive_find(&needle, haystack))
        }

        fn qc_bm_equals_single(pile1: Vec<u8>, pile2: Vec<u8>) -> TestResult {
            if pile1.len() == 0 || pile2.len() == 0 {
                return TestResult::discard();
            }

            let (needle, haystack) = if pile1.len() < pile2.len() {
                (pile1, pile2.as_slice())
            } else {
                (pile2, pile1.as_slice())
            };

            let bm_searcher = BoyerMooreSearch::new(needle.clone());
            let freqy_memchr = FreqyPacked::new(needle);
            TestResult::from_bool(
                bm_searcher.find(haystack) == freqy_memchr.find(haystack))
        }

        fn qc_bm_finds_trailing_needle(
            haystack_pre: Vec<u8>,
            needle: Vec<u8>
        ) -> TestResult {
            if needle.len() == 0 {
                return TestResult::discard();
            }

            let mut haystack = haystack_pre.clone();
            let searcher = BoyerMooreSearch::new(needle.clone());

            if haystack.len() >= needle.len() &&
                searcher.find(haystack.as_slice()).is_some() {
                return TestResult::discard();
            }

            haystack.extend(needle.clone());

            // What if the the tail of the haystack can start the
            // needle?
            let start = haystack_pre.len()
                .checked_sub(needle.len())
                .unwrap_or(0);
            for i in 0..(needle.len() - 1) {
                if searcher.find(&haystack[(i + start)..]).is_some() {
                    return TestResult::discard();
                }
            }

            TestResult::from_bool(
                searcher.find(haystack.as_slice())
                        .map(|x| x == haystack_pre.len())
                        .unwrap_or(false))
        }

        // qc_equals_* is only testing the negative case as @burntsushi
        // pointed out in https://github.com/rust-lang/regex/issues/446.
        // This quickcheck prop represents an effort to force testing of
        // the positive case. qc_bm_finds_first and qc_bm_finds_trailing_needle
        // already check some of the positive cases, but they don't cover
        // cases where the needle is in the middle of haystack. This prop
        // fills that hole.
        fn qc_bm_finds_subslice(
            haystack: Vec<u8>,
            needle_start: usize,
            needle_length: usize
        ) -> TestResult {
            if haystack.len() == 0 {
                return TestResult::discard();
            }

            let needle_start = needle_start % haystack.len();
            let needle_length = needle_length % (haystack.len() - needle_start);

            if needle_length == 0 {
                return TestResult::discard();
            }

            let needle = &haystack[needle_start..(needle_start + needle_length)];

            let bm_searcher = BoyerMooreSearch::new(needle.to_vec());

            let start = naive_find(&needle, &haystack);
            match start {
                None => TestResult::from_bool(false),
                Some(nf_start) =>
                    TestResult::from_bool(
                        nf_start <= needle_start
                            && bm_searcher.find(&haystack) == start
                    )
            }
        }

        fn qc_bm_finds_first(needle: Vec<u8>) -> TestResult {
            if needle.len() == 0 {
                return TestResult::discard();
            }

            let mut haystack = needle.clone();
            let searcher = BoyerMooreSearch::new(needle.clone());
            haystack.extend(needle);

            TestResult::from_bool(
                searcher.find(haystack.as_slice())
                        .map(|x| x == 0)
                        .unwrap_or(false))
        }
    }
}
