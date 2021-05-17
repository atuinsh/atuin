#[cfg(feature = "std")]
use dense::{self, DenseDFA};
use dfa::DFA;
#[cfg(feature = "std")]
use error::Result;
#[cfg(feature = "std")]
use sparse::SparseDFA;
#[cfg(feature = "std")]
use state_id::StateID;

/// A regular expression that uses deterministic finite automata for fast
/// searching.
///
/// A regular expression is comprised of two DFAs, a "forward" DFA and a
/// "reverse" DFA. The forward DFA is responsible for detecting the end of a
/// match while the reverse DFA is responsible for detecting the start of a
/// match. Thus, in order to find the bounds of any given match, a forward
/// search must first be run followed by a reverse search. A match found by
/// the forward DFA guarantees that the reverse DFA will also find a match.
///
/// The type of the DFA used by a `Regex` corresponds to the `D` type
/// parameter, which must satisfy the [`DFA`](trait.DFA.html) trait. Typically,
/// `D` is either a [`DenseDFA`](enum.DenseDFA.html) or a
/// [`SparseDFA`](enum.SparseDFA.html), where dense DFAs use more memory but
/// search faster, while sparse DFAs use less memory but search more slowly.
///
/// By default, a regex's DFA type parameter is set to
/// `DenseDFA<Vec<usize>, usize>`. For most in-memory work loads, this is the
/// most convenient type that gives the best search performance.
///
/// # Sparse DFAs
///
/// Since a `Regex` is generic over the `DFA` trait, it can be used with any
/// kind of DFA. While this crate constructs dense DFAs by default, it is easy
/// enough to build corresponding sparse DFAs, and then build a regex from
/// them:
///
/// ```
/// use regex_automata::Regex;
///
/// # fn example() -> Result<(), regex_automata::Error> {
/// // First, build a regex that uses dense DFAs.
/// let dense_re = Regex::new("foo[0-9]+")?;
///
/// // Second, build sparse DFAs from the forward and reverse dense DFAs.
/// let fwd = dense_re.forward().to_sparse()?;
/// let rev = dense_re.reverse().to_sparse()?;
///
/// // Third, build a new regex from the constituent sparse DFAs.
/// let sparse_re = Regex::from_dfas(fwd, rev);
///
/// // A regex that uses sparse DFAs can be used just like with dense DFAs.
/// assert_eq!(true, sparse_re.is_match(b"foo123"));
/// # Ok(()) }; example().unwrap()
/// ```
#[cfg(feature = "std")]
#[derive(Clone, Debug)]
pub struct Regex<D: DFA = DenseDFA<Vec<usize>, usize>> {
    forward: D,
    reverse: D,
}

/// A regular expression that uses deterministic finite automata for fast
/// searching.
///
/// A regular expression is comprised of two DFAs, a "forward" DFA and a
/// "reverse" DFA. The forward DFA is responsible for detecting the end of a
/// match while the reverse DFA is responsible for detecting the start of a
/// match. Thus, in order to find the bounds of any given match, a forward
/// search must first be run followed by a reverse search. A match found by
/// the forward DFA guarantees that the reverse DFA will also find a match.
///
/// The type of the DFA used by a `Regex` corresponds to the `D` type
/// parameter, which must satisfy the [`DFA`](trait.DFA.html) trait. Typically,
/// `D` is either a [`DenseDFA`](enum.DenseDFA.html) or a
/// [`SparseDFA`](enum.SparseDFA.html), where dense DFAs use more memory but
/// search faster, while sparse DFAs use less memory but search more slowly.
///
/// When using this crate without the standard library, the `Regex` type has
/// no default type parameter.
///
/// # Sparse DFAs
///
/// Since a `Regex` is generic over the `DFA` trait, it can be used with any
/// kind of DFA. While this crate constructs dense DFAs by default, it is easy
/// enough to build corresponding sparse DFAs, and then build a regex from
/// them:
///
/// ```
/// use regex_automata::Regex;
///
/// # fn example() -> Result<(), regex_automata::Error> {
/// // First, build a regex that uses dense DFAs.
/// let dense_re = Regex::new("foo[0-9]+")?;
///
/// // Second, build sparse DFAs from the forward and reverse dense DFAs.
/// let fwd = dense_re.forward().to_sparse()?;
/// let rev = dense_re.reverse().to_sparse()?;
///
/// // Third, build a new regex from the constituent sparse DFAs.
/// let sparse_re = Regex::from_dfas(fwd, rev);
///
/// // A regex that uses sparse DFAs can be used just like with dense DFAs.
/// assert_eq!(true, sparse_re.is_match(b"foo123"));
/// # Ok(()) }; example().unwrap()
/// ```
#[cfg(not(feature = "std"))]
#[derive(Clone, Debug)]
pub struct Regex<D> {
    forward: D,
    reverse: D,
}

#[cfg(feature = "std")]
impl Regex {
    /// Parse the given regular expression using a default configuration and
    /// return the corresponding regex.
    ///
    /// The default configuration uses `usize` for state IDs, premultiplies
    /// them and reduces the alphabet size by splitting bytes into equivalence
    /// classes. The underlying DFAs are *not* minimized.
    ///
    /// If you want a non-default configuration, then use the
    /// [`RegexBuilder`](struct.RegexBuilder.html)
    /// to set your own configuration.
    ///
    /// # Example
    ///
    /// ```
    /// use regex_automata::Regex;
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let re = Regex::new("foo[0-9]+bar")?;
    /// assert_eq!(Some((3, 14)), re.find(b"zzzfoo12345barzzz"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    pub fn new(pattern: &str) -> Result<Regex> {
        RegexBuilder::new().build(pattern)
    }
}

#[cfg(feature = "std")]
impl Regex<SparseDFA<Vec<u8>, usize>> {
    /// Parse the given regular expression using a default configuration and
    /// return the corresponding regex using sparse DFAs.
    ///
    /// The default configuration uses `usize` for state IDs, reduces the
    /// alphabet size by splitting bytes into equivalence classes. The
    /// underlying DFAs are *not* minimized.
    ///
    /// If you want a non-default configuration, then use the
    /// [`RegexBuilder`](struct.RegexBuilder.html)
    /// to set your own configuration.
    ///
    /// # Example
    ///
    /// ```
    /// use regex_automata::Regex;
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let re = Regex::new_sparse("foo[0-9]+bar")?;
    /// assert_eq!(Some((3, 14)), re.find(b"zzzfoo12345barzzz"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    pub fn new_sparse(
        pattern: &str,
    ) -> Result<Regex<SparseDFA<Vec<u8>, usize>>> {
        RegexBuilder::new().build_sparse(pattern)
    }
}

impl<D: DFA> Regex<D> {
    /// Returns true if and only if the given bytes match.
    ///
    /// This routine may short circuit if it knows that scanning future input
    /// will never lead to a different result. In particular, if the underlying
    /// DFA enters a match state or a dead state, then this routine will return
    /// `true` or `false`, respectively, without inspecting any future input.
    ///
    /// # Example
    ///
    /// ```
    /// use regex_automata::Regex;
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let re = Regex::new("foo[0-9]+bar")?;
    /// assert_eq!(true, re.is_match(b"foo12345bar"));
    /// assert_eq!(false, re.is_match(b"foobar"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    pub fn is_match(&self, input: &[u8]) -> bool {
        self.is_match_at(input, 0)
    }

    /// Returns the first position at which a match is found.
    ///
    /// This routine stops scanning input in precisely the same circumstances
    /// as `is_match`. The key difference is that this routine returns the
    /// position at which it stopped scanning input if and only if a match
    /// was found. If no match is found, then `None` is returned.
    ///
    /// # Example
    ///
    /// ```
    /// use regex_automata::Regex;
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let re = Regex::new("foo[0-9]+")?;
    /// assert_eq!(Some(4), re.shortest_match(b"foo12345"));
    ///
    /// // Normally, the end of the leftmost first match here would be 3,
    /// // but the shortest match semantics detect a match earlier.
    /// let re = Regex::new("abc|a")?;
    /// assert_eq!(Some(1), re.shortest_match(b"abc"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    pub fn shortest_match(&self, input: &[u8]) -> Option<usize> {
        self.shortest_match_at(input, 0)
    }

    /// Returns the start and end offset of the leftmost first match. If no
    /// match exists, then `None` is returned.
    ///
    /// The "leftmost first" match corresponds to the match with the smallest
    /// starting offset, but where the end offset is determined by preferring
    /// earlier branches in the original regular expression. For example,
    /// `Sam|Samwise` will match `Sam` in `Samwise`, but `Samwise|Sam` will
    /// match `Samwise` in `Samwise`.
    ///
    /// Generally speaking, the "leftmost first" match is how most backtracking
    /// regular expressions tend to work. This is in contrast to POSIX-style
    /// regular expressions that yield "leftmost longest" matches. Namely,
    /// both `Sam|Samwise` and `Samwise|Sam` match `Samwise` when using
    /// leftmost longest semantics.
    ///
    /// # Example
    ///
    /// ```
    /// use regex_automata::Regex;
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let re = Regex::new("foo[0-9]+")?;
    /// assert_eq!(Some((3, 11)), re.find(b"zzzfoo12345zzz"));
    ///
    /// // Even though a match is found after reading the first byte (`a`),
    /// // the leftmost first match semantics demand that we find the earliest
    /// // match that prefers earlier parts of the pattern over latter parts.
    /// let re = Regex::new("abc|a")?;
    /// assert_eq!(Some((0, 3)), re.find(b"abc"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    pub fn find(&self, input: &[u8]) -> Option<(usize, usize)> {
        self.find_at(input, 0)
    }

    /// Returns the same as `is_match`, but starts the search at the given
    /// offset.
    ///
    /// The significance of the starting point is that it takes the surrounding
    /// context into consideration. For example, if the DFA is anchored, then
    /// a match can only occur when `start == 0`.
    pub fn is_match_at(&self, input: &[u8], start: usize) -> bool {
        self.forward().is_match_at(input, start)
    }

    /// Returns the same as `shortest_match`, but starts the search at the
    /// given offset.
    ///
    /// The significance of the starting point is that it takes the surrounding
    /// context into consideration. For example, if the DFA is anchored, then
    /// a match can only occur when `start == 0`.
    pub fn shortest_match_at(
        &self,
        input: &[u8],
        start: usize,
    ) -> Option<usize> {
        self.forward().shortest_match_at(input, start)
    }

    /// Returns the same as `find`, but starts the search at the given
    /// offset.
    ///
    /// The significance of the starting point is that it takes the surrounding
    /// context into consideration. For example, if the DFA is anchored, then
    /// a match can only occur when `start == 0`.
    pub fn find_at(
        &self,
        input: &[u8],
        start: usize,
    ) -> Option<(usize, usize)> {
        let end = match self.forward().find_at(input, start) {
            None => return None,
            Some(end) => end,
        };
        let start = self
            .reverse()
            .rfind(&input[start..end])
            .map(|i| start + i)
            .expect("reverse search must match if forward search does");
        Some((start, end))
    }

    /// Returns an iterator over all non-overlapping leftmost first matches
    /// in the given bytes. If no match exists, then the iterator yields no
    /// elements.
    ///
    /// Note that if the regex can match the empty string, then it is
    /// possible for the iterator to yield a zero-width match at a location
    /// that is not a valid UTF-8 boundary (for example, between the code units
    /// of a UTF-8 encoded codepoint). This can happen regardless of whether
    /// [`allow_invalid_utf8`](struct.RegexBuilder.html#method.allow_invalid_utf8)
    /// was enabled or not.
    ///
    /// # Example
    ///
    /// ```
    /// use regex_automata::Regex;
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let re = Regex::new("foo[0-9]+")?;
    /// let text = b"foo1 foo12 foo123";
    /// let matches: Vec<(usize, usize)> = re.find_iter(text).collect();
    /// assert_eq!(matches, vec![(0, 4), (5, 10), (11, 17)]);
    /// # Ok(()) }; example().unwrap()
    /// ```
    pub fn find_iter<'r, 't>(&'r self, input: &'t [u8]) -> Matches<'r, 't, D> {
        Matches::new(self, input)
    }

    /// Build a new regex from its constituent forward and reverse DFAs.
    ///
    /// This is useful when deserializing a regex from some arbitrary
    /// memory region. This is also useful for building regexes from other
    /// types of DFAs.
    ///
    /// # Example
    ///
    /// This example is a bit a contrived. The usual use of these methods
    /// would involve serializing `initial_re` somewhere and then deserializing
    /// it later to build a regex.
    ///
    /// ```
    /// use regex_automata::Regex;
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let initial_re = Regex::new("foo[0-9]+")?;
    /// assert_eq!(true, initial_re.is_match(b"foo123"));
    ///
    /// let (fwd, rev) = (initial_re.forward(), initial_re.reverse());
    /// let re = Regex::from_dfas(fwd, rev);
    /// assert_eq!(true, re.is_match(b"foo123"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    ///
    /// This example shows how you might build smaller DFAs, and then use those
    /// smaller DFAs to build a new regex.
    ///
    /// ```
    /// use regex_automata::Regex;
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let initial_re = Regex::new("foo[0-9]+")?;
    /// assert_eq!(true, initial_re.is_match(b"foo123"));
    ///
    /// let fwd = initial_re.forward().to_u16()?;
    /// let rev = initial_re.reverse().to_u16()?;
    /// let re = Regex::from_dfas(fwd, rev);
    /// assert_eq!(true, re.is_match(b"foo123"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    ///
    /// This example shows how to build a `Regex` that uses sparse DFAs instead
    /// of dense DFAs:
    ///
    /// ```
    /// use regex_automata::Regex;
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let initial_re = Regex::new("foo[0-9]+")?;
    /// assert_eq!(true, initial_re.is_match(b"foo123"));
    ///
    /// let fwd = initial_re.forward().to_sparse()?;
    /// let rev = initial_re.reverse().to_sparse()?;
    /// let re = Regex::from_dfas(fwd, rev);
    /// assert_eq!(true, re.is_match(b"foo123"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    pub fn from_dfas(forward: D, reverse: D) -> Regex<D> {
        Regex { forward, reverse }
    }

    /// Return the underlying DFA responsible for forward matching.
    pub fn forward(&self) -> &D {
        &self.forward
    }

    /// Return the underlying DFA responsible for reverse matching.
    pub fn reverse(&self) -> &D {
        &self.reverse
    }
}

/// An iterator over all non-overlapping matches for a particular search.
///
/// The iterator yields a `(usize, usize)` value until no more matches could be
/// found. The first `usize` is the start of the match (inclusive) while the
/// second `usize` is the end of the match (exclusive).
///
/// `S` is the type used to represent state identifiers in the underlying
/// regex. The lifetime variables are as follows:
///
/// * `'r` is the lifetime of the regular expression value itself.
/// * `'t` is the lifetime of the text being searched.
#[derive(Clone, Debug)]
pub struct Matches<'r, 't, D: DFA + 'r> {
    re: &'r Regex<D>,
    text: &'t [u8],
    last_end: usize,
    last_match: Option<usize>,
}

impl<'r, 't, D: DFA> Matches<'r, 't, D> {
    fn new(re: &'r Regex<D>, text: &'t [u8]) -> Matches<'r, 't, D> {
        Matches { re, text, last_end: 0, last_match: None }
    }
}

impl<'r, 't, D: DFA> Iterator for Matches<'r, 't, D> {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<(usize, usize)> {
        if self.last_end > self.text.len() {
            return None;
        }
        let (s, e) = match self.re.find_at(self.text, self.last_end) {
            None => return None,
            Some((s, e)) => (s, e),
        };
        if s == e {
            // This is an empty match. To ensure we make progress, start
            // the next search at the smallest possible starting position
            // of the next match following this one.
            self.last_end = e + 1;
            // Don't accept empty matches immediately following a match.
            // Just move on to the next match.
            if Some(e) == self.last_match {
                return self.next();
            }
        } else {
            self.last_end = e;
        }
        self.last_match = Some(e);
        Some((s, e))
    }
}

/// A builder for a regex based on deterministic finite automatons.
///
/// This builder permits configuring several aspects of the construction
/// process such as case insensitivity, Unicode support and various options
/// that impact the size of the underlying DFAs. In some cases, options (like
/// performing DFA minimization) can come with a substantial additional cost.
///
/// This builder generally constructs two DFAs, where one is responsible for
/// finding the end of a match and the other is responsible for finding the
/// start of a match. If you only need to detect whether something matched,
/// or only the end of a match, then you should use a
/// [`dense::Builder`](dense/struct.Builder.html)
/// to construct a single DFA, which is cheaper than building two DFAs.
#[cfg(feature = "std")]
#[derive(Clone, Debug)]
pub struct RegexBuilder {
    dfa: dense::Builder,
}

#[cfg(feature = "std")]
impl RegexBuilder {
    /// Create a new regex builder with the default configuration.
    pub fn new() -> RegexBuilder {
        RegexBuilder { dfa: dense::Builder::new() }
    }

    /// Build a regex from the given pattern.
    ///
    /// If there was a problem parsing or compiling the pattern, then an error
    /// is returned.
    pub fn build(&self, pattern: &str) -> Result<Regex> {
        self.build_with_size::<usize>(pattern)
    }

    /// Build a regex from the given pattern using sparse DFAs.
    ///
    /// If there was a problem parsing or compiling the pattern, then an error
    /// is returned.
    pub fn build_sparse(
        &self,
        pattern: &str,
    ) -> Result<Regex<SparseDFA<Vec<u8>, usize>>> {
        self.build_with_size_sparse::<usize>(pattern)
    }

    /// Build a regex from the given pattern using a specific representation
    /// for the underlying DFA state IDs.
    ///
    /// If there was a problem parsing or compiling the pattern, then an error
    /// is returned.
    ///
    /// The representation of state IDs is determined by the `S` type
    /// parameter. In general, `S` is usually one of `u8`, `u16`, `u32`, `u64`
    /// or `usize`, where `usize` is the default used for `build`. The purpose
    /// of specifying a representation for state IDs is to reduce the memory
    /// footprint of the underlying DFAs.
    ///
    /// When using this routine, the chosen state ID representation will be
    /// used throughout determinization and minimization, if minimization was
    /// requested. Even if the minimized DFAs can fit into the chosen state ID
    /// representation but the initial determinized DFA cannot, then this will
    /// still return an error. To get a minimized DFA with a smaller state ID
    /// representation, first build it with a bigger state ID representation,
    /// and then shrink the sizes of the DFAs using one of its conversion
    /// routines, such as [`DenseDFA::to_u16`](enum.DenseDFA.html#method.to_u16).
    /// Finally, reconstitute the regex via
    /// [`Regex::from_dfa`](struct.Regex.html#method.from_dfa).
    pub fn build_with_size<S: StateID>(
        &self,
        pattern: &str,
    ) -> Result<Regex<DenseDFA<Vec<S>, S>>> {
        let forward = self.dfa.build_with_size(pattern)?;
        let reverse = self
            .dfa
            .clone()
            .anchored(true)
            .reverse(true)
            .longest_match(true)
            .build_with_size(pattern)?;
        Ok(Regex::from_dfas(forward, reverse))
    }

    /// Build a regex from the given pattern using a specific representation
    /// for the underlying DFA state IDs using sparse DFAs.
    pub fn build_with_size_sparse<S: StateID>(
        &self,
        pattern: &str,
    ) -> Result<Regex<SparseDFA<Vec<u8>, S>>> {
        let re = self.build_with_size(pattern)?;
        let fwd = re.forward().to_sparse()?;
        let rev = re.reverse().to_sparse()?;
        Ok(Regex::from_dfas(fwd, rev))
    }

    /// Set whether matching must be anchored at the beginning of the input.
    ///
    /// When enabled, a match must begin at the start of the input. When
    /// disabled, the regex will act as if the pattern started with a `.*?`,
    /// which enables a match to appear anywhere.
    ///
    /// By default this is disabled.
    pub fn anchored(&mut self, yes: bool) -> &mut RegexBuilder {
        self.dfa.anchored(yes);
        self
    }

    /// Enable or disable the case insensitive flag by default.
    ///
    /// By default this is disabled. It may alternatively be selectively
    /// enabled in the regular expression itself via the `i` flag.
    pub fn case_insensitive(&mut self, yes: bool) -> &mut RegexBuilder {
        self.dfa.case_insensitive(yes);
        self
    }

    /// Enable verbose mode in the regular expression.
    ///
    /// When enabled, verbose mode permits insigificant whitespace in many
    /// places in the regular expression, as well as comments. Comments are
    /// started using `#` and continue until the end of the line.
    ///
    /// By default, this is disabled. It may be selectively enabled in the
    /// regular expression by using the `x` flag regardless of this setting.
    pub fn ignore_whitespace(&mut self, yes: bool) -> &mut RegexBuilder {
        self.dfa.ignore_whitespace(yes);
        self
    }

    /// Enable or disable the "dot matches any character" flag by default.
    ///
    /// By default this is disabled. It may alternatively be selectively
    /// enabled in the regular expression itself via the `s` flag.
    pub fn dot_matches_new_line(&mut self, yes: bool) -> &mut RegexBuilder {
        self.dfa.dot_matches_new_line(yes);
        self
    }

    /// Enable or disable the "swap greed" flag by default.
    ///
    /// By default this is disabled. It may alternatively be selectively
    /// enabled in the regular expression itself via the `U` flag.
    pub fn swap_greed(&mut self, yes: bool) -> &mut RegexBuilder {
        self.dfa.swap_greed(yes);
        self
    }

    /// Enable or disable the Unicode flag (`u`) by default.
    ///
    /// By default this is **enabled**. It may alternatively be selectively
    /// disabled in the regular expression itself via the `u` flag.
    ///
    /// Note that unless `allow_invalid_utf8` is enabled (it's disabled by
    /// default), a regular expression will fail to parse if Unicode mode is
    /// disabled and a sub-expression could possibly match invalid UTF-8.
    pub fn unicode(&mut self, yes: bool) -> &mut RegexBuilder {
        self.dfa.unicode(yes);
        self
    }

    /// When enabled, the builder will permit the construction of a regular
    /// expression that may match invalid UTF-8.
    ///
    /// When disabled (the default), the builder is guaranteed to produce a
    /// regex that will only ever match valid UTF-8 (otherwise, the builder
    /// will return an error).
    pub fn allow_invalid_utf8(&mut self, yes: bool) -> &mut RegexBuilder {
        self.dfa.allow_invalid_utf8(yes);
        self
    }

    /// Set the nesting limit used for the regular expression parser.
    ///
    /// The nesting limit controls how deep the abstract syntax tree is allowed
    /// to be. If the AST exceeds the given limit (e.g., with too many nested
    /// groups), then an error is returned by the parser.
    ///
    /// The purpose of this limit is to act as a heuristic to prevent stack
    /// overflow when building a finite automaton from a regular expression's
    /// abstract syntax tree. In particular, construction currently uses
    /// recursion. In the future, the implementation may stop using recursion
    /// and this option will no longer be necessary.
    ///
    /// This limit is not checked until the entire AST is parsed. Therefore,
    /// if callers want to put a limit on the amount of heap space used, then
    /// they should impose a limit on the length, in bytes, of the concrete
    /// pattern string. In particular, this is viable since the parser will
    /// limit itself to heap space proportional to the lenth of the pattern
    /// string.
    ///
    /// Note that a nest limit of `0` will return a nest limit error for most
    /// patterns but not all. For example, a nest limit of `0` permits `a` but
    /// not `ab`, since `ab` requires a concatenation AST item, which results
    /// in a nest depth of `1`. In general, a nest limit is not something that
    /// manifests in an obvious way in the concrete syntax, therefore, it
    /// should not be used in a granular way.
    pub fn nest_limit(&mut self, limit: u32) -> &mut RegexBuilder {
        self.dfa.nest_limit(limit);
        self
    }

    /// Minimize the underlying DFAs.
    ///
    /// When enabled, the DFAs powering the resulting regex will be minimized
    /// such that it is as small as possible.
    ///
    /// Whether one enables minimization or not depends on the types of costs
    /// you're willing to pay and how much you care about its benefits. In
    /// particular, minimization has worst case `O(n*k*logn)` time and `O(k*n)`
    /// space, where `n` is the number of DFA states and `k` is the alphabet
    /// size. In practice, minimization can be quite costly in terms of both
    /// space and time, so it should only be done if you're willing to wait
    /// longer to produce a DFA. In general, you might want a minimal DFA in
    /// the following circumstances:
    ///
    /// 1. You would like to optimize for the size of the automaton. This can
    ///    manifest in one of two ways. Firstly, if you're converting the
    ///    DFA into Rust code (or a table embedded in the code), then a minimal
    ///    DFA will translate into a corresponding reduction in code  size, and
    ///    thus, also the final compiled binary size. Secondly, if you are
    ///    building many DFAs and putting them on the heap, you'll be able to
    ///    fit more if they are smaller. Note though that building a minimal
    ///    DFA itself requires additional space; you only realize the space
    ///    savings once the minimal DFA is constructed (at which point, the
    ///    space used for minimization is freed).
    /// 2. You've observed that a smaller DFA results in faster match
    ///    performance. Naively, this isn't guaranteed since there is no
    ///    inherent difference between matching with a bigger-than-minimal
    ///    DFA and a minimal DFA. However, a smaller DFA may make use of your
    ///    CPU's cache more efficiently.
    /// 3. You are trying to establish an equivalence between regular
    ///    languages. The standard method for this is to build a minimal DFA
    ///    for each language and then compare them. If the DFAs are equivalent
    ///    (up to state renaming), then the languages are equivalent.
    ///
    /// This option is disabled by default.
    pub fn minimize(&mut self, yes: bool) -> &mut RegexBuilder {
        self.dfa.minimize(yes);
        self
    }

    /// Premultiply state identifiers in the underlying DFA transition tables.
    ///
    /// When enabled, state identifiers are premultiplied to point to their
    /// corresponding row in the DFA's transition table. That is, given the
    /// `i`th state, its corresponding premultiplied identifier is `i * k`
    /// where `k` is the alphabet size of the DFA. (The alphabet size is at
    /// most 256, but is in practice smaller if byte classes is enabled.)
    ///
    /// When state identifiers are not premultiplied, then the identifier of
    /// the `i`th state is `i`.
    ///
    /// The advantage of premultiplying state identifiers is that is saves
    /// a multiplication instruction per byte when searching with the DFA.
    /// This has been observed to lead to a 20% performance benefit in
    /// micro-benchmarks.
    ///
    /// The primary disadvantage of premultiplying state identifiers is
    /// that they require a larger integer size to represent. For example,
    /// if your DFA has 200 states, then its premultiplied form requires
    /// 16 bits to represent every possible state identifier, where as its
    /// non-premultiplied form only requires 8 bits.
    ///
    /// This option is enabled by default.
    pub fn premultiply(&mut self, yes: bool) -> &mut RegexBuilder {
        self.dfa.premultiply(yes);
        self
    }

    /// Shrink the size of the underlying DFA alphabet by mapping bytes to
    /// their equivalence classes.
    ///
    /// When enabled, each DFA will use a map from all possible bytes to their
    /// corresponding equivalence class. Each equivalence class represents a
    /// set of bytes that does not discriminate between a match and a non-match
    /// in the DFA. For example, the pattern `[ab]+` has at least two
    /// equivalence classes: a set containing `a` and `b` and a set containing
    /// every byte except for `a` and `b`. `a` and `b` are in the same
    /// equivalence classes because they never discriminate between a match
    /// and a non-match.
    ///
    /// The advantage of this map is that the size of the transition table can
    /// be reduced drastically from `#states * 256 * sizeof(id)` to
    /// `#states * k * sizeof(id)` where `k` is the number of equivalence
    /// classes. As a result, total space usage can decrease substantially.
    /// Moreover, since a smaller alphabet is used, compilation becomes faster
    /// as well.
    ///
    /// The disadvantage of this map is that every byte searched must be
    /// passed through this map before it can be used to determine the next
    /// transition. This has a small match time performance cost.
    ///
    /// This option is enabled by default.
    pub fn byte_classes(&mut self, yes: bool) -> &mut RegexBuilder {
        self.dfa.byte_classes(yes);
        self
    }

    /// Apply best effort heuristics to shrink the NFA at the expense of more
    /// time/memory.
    ///
    /// This may be exposed in the future, but for now is exported for use in
    /// the `regex-automata-debug` tool.
    #[doc(hidden)]
    pub fn shrink(&mut self, yes: bool) -> &mut RegexBuilder {
        self.dfa.shrink(yes);
        self
    }
}

#[cfg(feature = "std")]
impl Default for RegexBuilder {
    fn default() -> RegexBuilder {
        RegexBuilder::new()
    }
}
