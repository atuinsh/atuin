use state_id::StateID;

/// A trait describing the interface of a deterministic finite automaton (DFA).
///
/// Every DFA has exactly one start state and at least one dead state (which
/// may be the same, as in the case of an empty DFA). In all cases, a state
/// identifier of `0` must be a dead state such that `DFA::is_dead_state(0)`
/// always returns `true`.
///
/// Every DFA also has zero or more match states, such that
/// `DFA::is_match_state(id)` returns `true` if and only if `id` corresponds to
/// a match state.
///
/// In general, users of this trait likely will only need to use the search
/// routines such as `is_match`, `shortest_match`, `find` or `rfind`. The other
/// methods are lower level and are used for walking the transitions of a DFA
/// manually. In particular, the aforementioned search routines are implemented
/// generically in terms of the lower level transition walking routines.
pub trait DFA {
    /// The representation used for state identifiers in this DFA.
    ///
    /// Typically, this is one of `u8`, `u16`, `u32`, `u64` or `usize`.
    type ID: StateID;

    /// Return the identifier of this DFA's start state.
    fn start_state(&self) -> Self::ID;

    /// Returns true if and only if the given identifier corresponds to a match
    /// state.
    fn is_match_state(&self, id: Self::ID) -> bool;

    /// Returns true if and only if the given identifier corresponds to a dead
    /// state. When a DFA enters a dead state, it is impossible to leave and
    /// thus can never lead to a match.
    fn is_dead_state(&self, id: Self::ID) -> bool;

    /// Returns true if and only if the given identifier corresponds to either
    /// a dead state or a match state, such that one of `is_match_state(id)`
    /// or `is_dead_state(id)` must return true.
    ///
    /// Depending on the implementation of the DFA, this routine can be used
    /// to save a branch in the core matching loop. Nevertheless,
    /// `is_match_state(id) || is_dead_state(id)` is always a valid
    /// implementation.
    fn is_match_or_dead_state(&self, id: Self::ID) -> bool;

    /// Returns true if and only if this DFA is anchored.
    ///
    /// When a DFA is anchored, it is only allowed to report matches that
    /// start at index `0`.
    fn is_anchored(&self) -> bool;

    /// Given the current state that this DFA is in and the next input byte,
    /// this method returns the identifier of the next state. The identifier
    /// returned is always valid, but it may correspond to a dead state.
    fn next_state(&self, current: Self::ID, input: u8) -> Self::ID;

    /// Like `next_state`, but its implementation may look up the next state
    /// without memory safety checks such as bounds checks. As such, callers
    /// must ensure that the given identifier corresponds to a valid DFA
    /// state. Implementors must, in turn, ensure that this routine is safe
    /// for all valid state identifiers and for all possible `u8` values.
    unsafe fn next_state_unchecked(
        &self,
        current: Self::ID,
        input: u8,
    ) -> Self::ID;

    /// Returns true if and only if the given bytes match this DFA.
    ///
    /// This routine may short circuit if it knows that scanning future input
    /// will never lead to a different result. In particular, if a DFA enters
    /// a match state or a dead state, then this routine will return `true` or
    /// `false`, respectively, without inspecting any future input.
    ///
    /// # Example
    ///
    /// This example shows how to use this method with a
    /// [`DenseDFA`](enum.DenseDFA.html).
    ///
    /// ```
    /// use regex_automata::{DFA, DenseDFA};
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let dfa = DenseDFA::new("foo[0-9]+bar")?;
    /// assert_eq!(true, dfa.is_match(b"foo12345bar"));
    /// assert_eq!(false, dfa.is_match(b"foobar"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    #[inline]
    fn is_match(&self, bytes: &[u8]) -> bool {
        self.is_match_at(bytes, 0)
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
    /// This example shows how to use this method with a
    /// [`DenseDFA`](enum.DenseDFA.html).
    ///
    /// ```
    /// use regex_automata::{DFA, DenseDFA};
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let dfa = DenseDFA::new("foo[0-9]+")?;
    /// assert_eq!(Some(4), dfa.shortest_match(b"foo12345"));
    ///
    /// // Normally, the end of the leftmost first match here would be 3,
    /// // but the shortest match semantics detect a match earlier.
    /// let dfa = DenseDFA::new("abc|a")?;
    /// assert_eq!(Some(1), dfa.shortest_match(b"abc"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    #[inline]
    fn shortest_match(&self, bytes: &[u8]) -> Option<usize> {
        self.shortest_match_at(bytes, 0)
    }

    /// Returns the end offset of the longest match. If no match exists,
    /// then `None` is returned.
    ///
    /// Implementors of this trait are not required to implement any particular
    /// match semantics (such as leftmost-first), which are instead manifest in
    /// the DFA's topology itself.
    ///
    /// In particular, this method must continue searching even after it
    /// enters a match state. The search should only terminate once it has
    /// reached the end of the input or when it has entered a dead state. Upon
    /// termination, the position of the last byte seen while still in a match
    /// state is returned.
    ///
    /// # Example
    ///
    /// This example shows how to use this method with a
    /// [`DenseDFA`](enum.DenseDFA.html). By default, a dense DFA uses
    /// "leftmost first" match semantics.
    ///
    /// Leftmost first match semantics corresponds to the match with the
    /// smallest starting offset, but where the end offset is determined by
    /// preferring earlier branches in the original regular expression. For
    /// example, `Sam|Samwise` will match `Sam` in `Samwise`, but `Samwise|Sam`
    /// will match `Samwise` in `Samwise`.
    ///
    /// Generally speaking, the "leftmost first" match is how most backtracking
    /// regular expressions tend to work. This is in contrast to POSIX-style
    /// regular expressions that yield "leftmost longest" matches. Namely,
    /// both `Sam|Samwise` and `Samwise|Sam` match `Samwise` when using
    /// leftmost longest semantics.
    ///
    /// ```
    /// use regex_automata::{DFA, DenseDFA};
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let dfa = DenseDFA::new("foo[0-9]+")?;
    /// assert_eq!(Some(8), dfa.find(b"foo12345"));
    ///
    /// // Even though a match is found after reading the first byte (`a`),
    /// // the leftmost first match semantics demand that we find the earliest
    /// // match that prefers earlier parts of the pattern over latter parts.
    /// let dfa = DenseDFA::new("abc|a")?;
    /// assert_eq!(Some(3), dfa.find(b"abc"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    #[inline]
    fn find(&self, bytes: &[u8]) -> Option<usize> {
        self.find_at(bytes, 0)
    }

    /// Returns the start offset of the longest match in reverse, by searching
    /// from the end of the input towards the start of the input. If no match
    /// exists, then `None` is returned. In other words, this has the same
    /// match semantics as `find`, but in reverse.
    ///
    /// # Example
    ///
    /// This example shows how to use this method with a
    /// [`DenseDFA`](enum.DenseDFA.html). In particular, this routine
    /// is principally useful when used in conjunction with the
    /// [`dense::Builder::reverse`](dense/struct.Builder.html#method.reverse)
    /// configuration knob. In general, it's unlikely to be correct to use both
    /// `find` and `rfind` with the same DFA since any particular DFA will only
    /// support searching in one direction.
    ///
    /// ```
    /// use regex_automata::{dense, DFA};
    ///
    /// # fn example() -> Result<(), regex_automata::Error> {
    /// let dfa = dense::Builder::new().reverse(true).build("foo[0-9]+")?;
    /// assert_eq!(Some(0), dfa.rfind(b"foo12345"));
    /// # Ok(()) }; example().unwrap()
    /// ```
    #[inline]
    fn rfind(&self, bytes: &[u8]) -> Option<usize> {
        self.rfind_at(bytes, bytes.len())
    }

    /// Returns the same as `is_match`, but starts the search at the given
    /// offset.
    ///
    /// The significance of the starting point is that it takes the surrounding
    /// context into consideration. For example, if the DFA is anchored, then
    /// a match can only occur when `start == 0`.
    #[inline]
    fn is_match_at(&self, bytes: &[u8], start: usize) -> bool {
        if self.is_anchored() && start > 0 {
            return false;
        }

        let mut state = self.start_state();
        if self.is_match_or_dead_state(state) {
            return self.is_match_state(state);
        }
        for &b in bytes[start..].iter() {
            state = unsafe { self.next_state_unchecked(state, b) };
            if self.is_match_or_dead_state(state) {
                return self.is_match_state(state);
            }
        }
        false
    }

    /// Returns the same as `shortest_match`, but starts the search at the
    /// given offset.
    ///
    /// The significance of the starting point is that it takes the surrounding
    /// context into consideration. For example, if the DFA is anchored, then
    /// a match can only occur when `start == 0`.
    #[inline]
    fn shortest_match_at(&self, bytes: &[u8], start: usize) -> Option<usize> {
        if self.is_anchored() && start > 0 {
            return None;
        }

        let mut state = self.start_state();
        if self.is_match_or_dead_state(state) {
            return if self.is_dead_state(state) { None } else { Some(start) };
        }
        for (i, &b) in bytes[start..].iter().enumerate() {
            state = unsafe { self.next_state_unchecked(state, b) };
            if self.is_match_or_dead_state(state) {
                return if self.is_dead_state(state) {
                    None
                } else {
                    Some(start + i + 1)
                };
            }
        }
        None
    }

    /// Returns the same as `find`, but starts the search at the given
    /// offset.
    ///
    /// The significance of the starting point is that it takes the surrounding
    /// context into consideration. For example, if the DFA is anchored, then
    /// a match can only occur when `start == 0`.
    #[inline]
    fn find_at(&self, bytes: &[u8], start: usize) -> Option<usize> {
        if self.is_anchored() && start > 0 {
            return None;
        }

        let mut state = self.start_state();
        let mut last_match = if self.is_dead_state(state) {
            return None;
        } else if self.is_match_state(state) {
            Some(start)
        } else {
            None
        };
        for (i, &b) in bytes[start..].iter().enumerate() {
            state = unsafe { self.next_state_unchecked(state, b) };
            if self.is_match_or_dead_state(state) {
                if self.is_dead_state(state) {
                    return last_match;
                }
                last_match = Some(start + i + 1);
            }
        }
        last_match
    }

    /// Returns the same as `rfind`, but starts the search at the given
    /// offset.
    ///
    /// The significance of the starting point is that it takes the surrounding
    /// context into consideration. For example, if the DFA is anchored, then
    /// a match can only occur when `start == bytes.len()`.
    #[inline(never)]
    fn rfind_at(&self, bytes: &[u8], start: usize) -> Option<usize> {
        if self.is_anchored() && start < bytes.len() {
            return None;
        }

        let mut state = self.start_state();
        let mut last_match = if self.is_dead_state(state) {
            return None;
        } else if self.is_match_state(state) {
            Some(start)
        } else {
            None
        };
        for (i, &b) in bytes[..start].iter().enumerate().rev() {
            state = unsafe { self.next_state_unchecked(state, b) };
            if self.is_match_or_dead_state(state) {
                if self.is_dead_state(state) {
                    return last_match;
                }
                last_match = Some(i);
            }
        }
        last_match
    }
}

impl<'a, T: DFA> DFA for &'a T {
    type ID = T::ID;

    #[inline]
    fn start_state(&self) -> Self::ID {
        (**self).start_state()
    }

    #[inline]
    fn is_match_state(&self, id: Self::ID) -> bool {
        (**self).is_match_state(id)
    }

    #[inline]
    fn is_match_or_dead_state(&self, id: Self::ID) -> bool {
        (**self).is_match_or_dead_state(id)
    }

    #[inline]
    fn is_dead_state(&self, id: Self::ID) -> bool {
        (**self).is_dead_state(id)
    }

    #[inline]
    fn is_anchored(&self) -> bool {
        (**self).is_anchored()
    }

    #[inline]
    fn next_state(&self, current: Self::ID, input: u8) -> Self::ID {
        (**self).next_state(current, input)
    }

    #[inline]
    unsafe fn next_state_unchecked(
        &self,
        current: Self::ID,
        input: u8,
    ) -> Self::ID {
        (**self).next_state_unchecked(current, input)
    }
}
