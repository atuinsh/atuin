use std::collections::HashMap;
use std::mem;
use std::rc::Rc;

use dense;
use error::Result;
use nfa::{self, NFA};
use sparse_set::SparseSet;
use state_id::{dead_id, StateID};

type DFARepr<S> = dense::Repr<Vec<S>, S>;

/// A determinizer converts an NFA to a DFA.
///
/// This determinizer follows the typical powerset construction, where each
/// DFA state is comprised of one or more NFA states. In the worst case, there
/// is one DFA state for every possible combination of NFA states. In practice,
/// this only happens in certain conditions, typically when there are bounded
/// repetitions.
///
/// The type variable `S` refers to the chosen state identifier representation
/// used for the DFA.
///
/// The lifetime variable `'a` refers to the lifetime of the NFA being
/// converted to a DFA.
#[derive(Debug)]
pub(crate) struct Determinizer<'a, S: StateID> {
    /// The NFA we're converting into a DFA.
    nfa: &'a NFA,
    /// The DFA we're building.
    dfa: DFARepr<S>,
    /// Each DFA state being built is defined as an *ordered* set of NFA
    /// states, along with a flag indicating whether the state is a match
    /// state or not.
    ///
    /// This is never empty. The first state is always a dummy state such that
    /// a state id == 0 corresponds to a dead state.
    builder_states: Vec<Rc<State>>,
    /// A cache of DFA states that already exist and can be easily looked up
    /// via ordered sets of NFA states.
    cache: HashMap<Rc<State>, S>,
    /// Scratch space for a stack of NFA states to visit, for depth first
    /// visiting without recursion.
    stack: Vec<nfa::StateID>,
    /// Scratch space for storing an ordered sequence of NFA states, for
    /// amortizing allocation.
    scratch_nfa_states: Vec<nfa::StateID>,
    /// Whether to build a DFA that finds the longest possible match.
    longest_match: bool,
}

/// An intermediate representation for a DFA state during determinization.
#[derive(Debug, Eq, Hash, PartialEq)]
struct State {
    /// Whether this state is a match state or not.
    is_match: bool,
    /// An ordered sequence of NFA states that make up this DFA state.
    nfa_states: Vec<nfa::StateID>,
}

impl<'a, S: StateID> Determinizer<'a, S> {
    /// Create a new determinizer for converting the given NFA to a DFA.
    pub fn new(nfa: &'a NFA) -> Determinizer<'a, S> {
        let dead = Rc::new(State::dead());
        let mut cache = HashMap::default();
        cache.insert(dead.clone(), dead_id());

        Determinizer {
            nfa,
            dfa: DFARepr::empty().anchored(nfa.is_anchored()),
            builder_states: vec![dead],
            cache,
            stack: vec![],
            scratch_nfa_states: vec![],
            longest_match: false,
        }
    }

    /// Instruct the determinizer to use equivalence classes as the transition
    /// alphabet instead of all possible byte values.
    pub fn with_byte_classes(mut self) -> Determinizer<'a, S> {
        let byte_classes = self.nfa.byte_classes().clone();
        self.dfa = DFARepr::empty_with_byte_classes(byte_classes)
            .anchored(self.nfa.is_anchored());
        self
    }

    /// Instruct the determinizer to build a DFA that recognizes the longest
    /// possible match instead of the leftmost first match. This is useful when
    /// constructing reverse DFAs for finding the start of a match.
    pub fn longest_match(mut self, yes: bool) -> Determinizer<'a, S> {
        self.longest_match = yes;
        self
    }

    /// Build the DFA. If there was a problem constructing the DFA (e.g., if
    /// the chosen state identifier representation is too small), then an error
    /// is returned.
    pub fn build(mut self) -> Result<DFARepr<S>> {
        let representative_bytes: Vec<u8> =
            self.dfa.byte_classes().representatives().collect();
        let mut sparse = self.new_sparse_set();
        let mut uncompiled = vec![self.add_start(&mut sparse)?];
        while let Some(dfa_id) = uncompiled.pop() {
            for &b in &representative_bytes {
                let (next_dfa_id, is_new) =
                    self.cached_state(dfa_id, b, &mut sparse)?;
                self.dfa.add_transition(dfa_id, b, next_dfa_id);
                if is_new {
                    uncompiled.push(next_dfa_id);
                }
            }
        }

        // At this point, we shuffle the matching states in the final DFA to
        // the beginning. This permits a DFA's match loop to detect a match
        // condition by merely inspecting the current state's identifier, and
        // avoids the need for any additional auxiliary storage.
        let is_match: Vec<bool> =
            self.builder_states.iter().map(|s| s.is_match).collect();
        self.dfa.shuffle_match_states(&is_match);
        Ok(self.dfa)
    }

    /// Return the identifier for the next DFA state given an existing DFA
    /// state and an input byte. If the next DFA state already exists, then
    /// return its identifier from the cache. Otherwise, build the state, cache
    /// it and return its identifier.
    ///
    /// The given sparse set is used for scratch space. It must have a capacity
    /// equivalent to the total number of NFA states, but its contents are
    /// otherwise unspecified.
    ///
    /// This routine returns a boolean indicating whether a new state was
    /// built. If a new state is built, then the caller needs to add it to its
    /// frontier of uncompiled DFA states to compute transitions for.
    fn cached_state(
        &mut self,
        dfa_id: S,
        b: u8,
        sparse: &mut SparseSet,
    ) -> Result<(S, bool)> {
        sparse.clear();
        // Compute the set of all reachable NFA states, including epsilons.
        self.next(dfa_id, b, sparse);
        // Build a candidate state and check if it has already been built.
        let state = self.new_state(sparse);
        if let Some(&cached_id) = self.cache.get(&state) {
            // Since we have a cached state, put the constructed state's
            // memory back into our scratch space, so that it can be reused.
            mem::replace(&mut self.scratch_nfa_states, state.nfa_states);
            return Ok((cached_id, false));
        }
        // Nothing was in the cache, so add this state to the cache.
        self.add_state(state).map(|s| (s, true))
    }

    /// Compute the set of all eachable NFA states, including the full epsilon
    /// closure, from a DFA state for a single byte of input.
    fn next(&mut self, dfa_id: S, b: u8, next_nfa_states: &mut SparseSet) {
        next_nfa_states.clear();
        for i in 0..self.builder_states[dfa_id.to_usize()].nfa_states.len() {
            let nfa_id = self.builder_states[dfa_id.to_usize()].nfa_states[i];
            match *self.nfa.state(nfa_id) {
                nfa::State::Union { .. }
                | nfa::State::Fail
                | nfa::State::Match => {}
                nfa::State::Range { range: ref r } => {
                    if r.start <= b && b <= r.end {
                        self.epsilon_closure(r.next, next_nfa_states);
                    }
                }
                nfa::State::Sparse { ref ranges } => {
                    for r in ranges.iter() {
                        if r.start > b {
                            break;
                        } else if r.start <= b && b <= r.end {
                            self.epsilon_closure(r.next, next_nfa_states);
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Compute the epsilon closure for the given NFA state.
    fn epsilon_closure(&mut self, start: nfa::StateID, set: &mut SparseSet) {
        if !self.nfa.state(start).is_epsilon() {
            set.insert(start);
            return;
        }

        self.stack.push(start);
        while let Some(mut id) = self.stack.pop() {
            loop {
                if set.contains(id) {
                    break;
                }
                set.insert(id);
                match *self.nfa.state(id) {
                    nfa::State::Range { .. }
                    | nfa::State::Sparse { .. }
                    | nfa::State::Fail
                    | nfa::State::Match => break,
                    nfa::State::Union { ref alternates } => {
                        id = match alternates.get(0) {
                            None => break,
                            Some(&id) => id,
                        };
                        self.stack.extend(alternates[1..].iter().rev());
                    }
                }
            }
        }
    }

    /// Compute the initial DFA state and return its identifier.
    ///
    /// The sparse set given is used for scratch space, and must have capacity
    /// equal to the total number of NFA states. Its contents are unspecified.
    fn add_start(&mut self, sparse: &mut SparseSet) -> Result<S> {
        sparse.clear();
        self.epsilon_closure(self.nfa.start(), sparse);
        let state = self.new_state(&sparse);
        let id = self.add_state(state)?;
        self.dfa.set_start_state(id);
        Ok(id)
    }

    /// Add the given state to the DFA and make it available in the cache.
    ///
    /// The state initially has no transitions. That is, it transitions to the
    /// dead state for all possible inputs.
    fn add_state(&mut self, state: State) -> Result<S> {
        let id = self.dfa.add_empty_state()?;
        let rstate = Rc::new(state);
        self.builder_states.push(rstate.clone());
        self.cache.insert(rstate, id);
        Ok(id)
    }

    /// Convert the given set of ordered NFA states to a DFA state.
    fn new_state(&mut self, set: &SparseSet) -> State {
        let mut state = State {
            is_match: false,
            nfa_states: mem::replace(&mut self.scratch_nfa_states, vec![]),
        };
        state.nfa_states.clear();

        for &id in set {
            match *self.nfa.state(id) {
                nfa::State::Range { .. } => {
                    state.nfa_states.push(id);
                }
                nfa::State::Sparse { .. } => {
                    state.nfa_states.push(id);
                }
                nfa::State::Fail => {
                    break;
                }
                nfa::State::Match => {
                    state.is_match = true;
                    if !self.longest_match {
                        break;
                    }
                }
                nfa::State::Union { .. } => {}
            }
        }
        state
    }

    /// Create a new sparse set with enough capacity to hold all NFA states.
    fn new_sparse_set(&self) -> SparseSet {
        SparseSet::new(self.nfa.len())
    }
}

impl State {
    /// Create a new empty dead state.
    fn dead() -> State {
        State { nfa_states: vec![], is_match: false }
    }
}
