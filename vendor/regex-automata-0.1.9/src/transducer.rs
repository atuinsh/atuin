use fst::Automaton;

use crate::{StateID, DFA};

macro_rules! imp {
    ($ty:ty, $id:ty) => {
        impl<T: AsRef<[$id]>, S: StateID> Automaton for $ty {
            type State = S;

            #[inline]
            fn start(&self) -> S {
                self.start_state()
            }

            #[inline]
            fn is_match(&self, state: &S) -> bool {
                self.is_match_state(*state)
            }

            #[inline]
            fn accept(&self, state: &S, byte: u8) -> S {
                self.next_state(*state, byte)
            }

            #[inline]
            fn can_match(&self, state: &S) -> bool {
                !self.is_dead_state(*state)
            }
        }
    };
}

imp!(crate::dense::DenseDFA<T, S>, S);
imp!(crate::dense::Standard<T, S>, S);
imp!(crate::dense::ByteClass<T, S>, S);
imp!(crate::dense::Premultiplied<T, S>, S);
imp!(crate::dense::PremultipliedByteClass<T, S>, S);
imp!(crate::sparse::SparseDFA<T, S>, u8);
imp!(crate::sparse::Standard<T, S>, u8);
imp!(crate::sparse::ByteClass<T, S>, u8);

#[cfg(test)]
mod tests {
    use bstr::BString;
    use fst::{Automaton, IntoStreamer, Set, Streamer};

    use crate::dense::{self, DenseDFA};
    use crate::sparse::SparseDFA;

    fn search<A: Automaton, D: AsRef<[u8]>>(
        set: &Set<D>,
        aut: A,
    ) -> Vec<BString> {
        let mut stream = set.search(aut).into_stream();

        let mut results = vec![];
        while let Some(key) = stream.next() {
            results.push(BString::from(key));
        }
        results
    }

    #[test]
    fn dense_anywhere() {
        let set =
            Set::from_iter(&["a", "bar", "baz", "wat", "xba", "xbax", "z"])
                .unwrap();
        let dfa = DenseDFA::new("ba.*").unwrap();
        let got = search(&set, &dfa);
        assert_eq!(got, vec!["bar", "baz", "xba", "xbax"]);
    }

    #[test]
    fn dense_anchored() {
        let set =
            Set::from_iter(&["a", "bar", "baz", "wat", "xba", "xbax", "z"])
                .unwrap();
        let dfa = dense::Builder::new().anchored(true).build("ba.*").unwrap();
        let got = search(&set, &dfa);
        assert_eq!(got, vec!["bar", "baz"]);
    }

    #[test]
    fn sparse_anywhere() {
        let set =
            Set::from_iter(&["a", "bar", "baz", "wat", "xba", "xbax", "z"])
                .unwrap();
        let dfa = SparseDFA::new("ba.*").unwrap();
        let got = search(&set, &dfa);
        assert_eq!(got, vec!["bar", "baz", "xba", "xbax"]);
    }

    #[test]
    fn sparse_anchored() {
        let set =
            Set::from_iter(&["a", "bar", "baz", "wat", "xba", "xbax", "z"])
                .unwrap();
        let dfa = dense::Builder::new()
            .anchored(true)
            .build("ba.*")
            .unwrap()
            .to_sparse()
            .unwrap();
        let got = search(&set, &dfa);
        assert_eq!(got, vec!["bar", "baz"]);
    }
}
