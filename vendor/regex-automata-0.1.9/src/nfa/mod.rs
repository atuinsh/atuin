use std::fmt;

use classes::ByteClasses;
pub use nfa::compiler::Builder;

mod compiler;
mod map;
mod range_trie;

/// The representation for an NFA state identifier.
pub type StateID = usize;

/// A final compiled NFA.
///
/// The states of the NFA are indexed by state IDs, which are how transitions
/// are expressed.
#[derive(Clone)]
pub struct NFA {
    /// Whether this NFA can only match at the beginning of input or not.
    ///
    /// When true, a match should only be reported if it begins at the 0th
    /// index of the haystack.
    anchored: bool,
    /// The starting state of this NFA.
    start: StateID,
    /// The state list. This list is guaranteed to be indexable by the starting
    /// state ID, and it is also guaranteed to contain exactly one `Match`
    /// state.
    states: Vec<State>,
    /// A mapping from any byte value to its corresponding equivalence class
    /// identifier. Two bytes in the same equivalence class cannot discriminate
    /// between a match or a non-match. This map can be used to shrink the
    /// total size of a DFA's transition table with a small match-time cost.
    ///
    /// Note that the NFA's transitions are *not* defined in terms of these
    /// equivalence classes. The NFA's transitions are defined on the original
    /// byte values. For the most part, this is because they wouldn't really
    /// help the NFA much since the NFA already uses a sparse representation
    /// to represent transitions. Byte classes are most effective in a dense
    /// representation.
    byte_classes: ByteClasses,
}

impl NFA {
    /// Returns an NFA that always matches at every position.
    pub fn always_match() -> NFA {
        NFA {
            anchored: false,
            start: 0,
            states: vec![State::Match],
            byte_classes: ByteClasses::empty(),
        }
    }

    /// Returns an NFA that never matches at any position.
    pub fn never_match() -> NFA {
        NFA {
            anchored: false,
            start: 0,
            states: vec![State::Fail],
            byte_classes: ByteClasses::empty(),
        }
    }

    /// Returns true if and only if this NFA is anchored.
    pub fn is_anchored(&self) -> bool {
        self.anchored
    }

    /// Return the number of states in this NFA.
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// Return the ID of the initial state of this NFA.
    pub fn start(&self) -> StateID {
        self.start
    }

    /// Return the NFA state corresponding to the given ID.
    pub fn state(&self, id: StateID) -> &State {
        &self.states[id]
    }

    /// Return the set of equivalence classes for this NFA. The slice returned
    /// always has length 256 and maps each possible byte value to its
    /// corresponding equivalence class ID (which is never more than 255).
    pub fn byte_classes(&self) -> &ByteClasses {
        &self.byte_classes
    }
}

impl fmt::Debug for NFA {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for (i, state) in self.states.iter().enumerate() {
            let status = if i == self.start { '>' } else { ' ' };
            writeln!(f, "{}{:06}: {:?}", status, i, state)?;
        }
        Ok(())
    }
}

/// A state in a final compiled NFA.
#[derive(Clone, Eq, PartialEq)]
pub enum State {
    /// A state that transitions to `next` if and only if the current input
    /// byte is in the range `[start, end]` (inclusive).
    ///
    /// This is a special case of Sparse in that it encodes only one transition
    /// (and therefore avoids the allocation).
    Range { range: Transition },
    /// A state with possibly many transitions, represented in a sparse
    /// fashion. Transitions are ordered lexicographically by input range.
    /// As such, this may only be used when every transition has equal
    /// priority. (In practice, this is only used for encoding large UTF-8
    /// automata.)
    Sparse { ranges: Box<[Transition]> },
    /// An alternation such that there exists an epsilon transition to all
    /// states in `alternates`, where matches found via earlier transitions
    /// are preferred over later transitions.
    Union { alternates: Box<[StateID]> },
    /// A fail state. When encountered, the automaton is guaranteed to never
    /// reach a match state.
    Fail,
    /// A match state. There is exactly one such occurrence of this state in
    /// an NFA.
    Match,
}

/// A transition to another state, only if the given byte falls in the
/// inclusive range specified.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct Transition {
    pub start: u8,
    pub end: u8,
    pub next: StateID,
}

impl State {
    /// Returns true if and only if this state contains one or more epsilon
    /// transitions.
    pub fn is_epsilon(&self) -> bool {
        match *self {
            State::Range { .. }
            | State::Sparse { .. }
            | State::Fail
            | State::Match => false,
            State::Union { .. } => true,
        }
    }

    /// Remap the transitions in this state using the given map. Namely, the
    /// given map should be indexed according to the transitions currently
    /// in this state.
    ///
    /// This is used during the final phase of the NFA compiler, which turns
    /// its intermediate NFA into the final NFA.
    fn remap(&mut self, remap: &[StateID]) {
        match *self {
            State::Range { ref mut range } => range.next = remap[range.next],
            State::Sparse { ref mut ranges } => {
                for r in ranges.iter_mut() {
                    r.next = remap[r.next];
                }
            }
            State::Union { ref mut alternates } => {
                for alt in alternates.iter_mut() {
                    *alt = remap[*alt];
                }
            }
            State::Fail => {}
            State::Match => {}
        }
    }
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            State::Range { ref range } => range.fmt(f),
            State::Sparse { ref ranges } => {
                let rs = ranges
                    .iter()
                    .map(|t| format!("{:?}", t))
                    .collect::<Vec<String>>()
                    .join(", ");
                write!(f, "sparse({})", rs)
            }
            State::Union { ref alternates } => {
                let alts = alternates
                    .iter()
                    .map(|id| format!("{}", id))
                    .collect::<Vec<String>>()
                    .join(", ");
                write!(f, "alt({})", alts)
            }
            State::Fail => write!(f, "FAIL"),
            State::Match => write!(f, "MATCH"),
        }
    }
}

impl fmt::Debug for Transition {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let Transition { start, end, next } = *self;
        if self.start == self.end {
            write!(f, "{} => {}", escape(start), next)
        } else {
            write!(f, "{}-{} => {}", escape(start), escape(end), next)
        }
    }
}

/// Return the given byte as its escaped string form.
fn escape(b: u8) -> String {
    use std::ascii;

    String::from_utf8(ascii::escape_default(b).collect::<Vec<_>>()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dense;
    use dfa::DFA;

    #[test]
    fn always_match() {
        let nfa = NFA::always_match();
        let dfa = dense::Builder::new().build_from_nfa::<usize>(&nfa).unwrap();

        assert_eq!(Some(0), dfa.find_at(b"", 0));
        assert_eq!(Some(0), dfa.find_at(b"a", 0));
        assert_eq!(Some(1), dfa.find_at(b"a", 1));
        assert_eq!(Some(0), dfa.find_at(b"ab", 0));
        assert_eq!(Some(1), dfa.find_at(b"ab", 1));
        assert_eq!(Some(2), dfa.find_at(b"ab", 2));
    }

    #[test]
    fn never_match() {
        let nfa = NFA::never_match();
        let dfa = dense::Builder::new().build_from_nfa::<usize>(&nfa).unwrap();

        assert_eq!(None, dfa.find_at(b"", 0));
        assert_eq!(None, dfa.find_at(b"a", 0));
        assert_eq!(None, dfa.find_at(b"a", 1));
        assert_eq!(None, dfa.find_at(b"ab", 0));
        assert_eq!(None, dfa.find_at(b"ab", 1));
        assert_eq!(None, dfa.find_at(b"ab", 2));
    }
}
