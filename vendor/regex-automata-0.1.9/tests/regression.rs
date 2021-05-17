use regex_automata::{dense, DFA};

// A regression test for checking that minimization correctly translates
// whether a state is a match state or not. Previously, it was possible for
// minimization to mark a non-matching state as matching.
#[test]
fn minimize_sets_correct_match_states() {
    let pattern =
        // This is a subset of the grapheme matching regex. I couldn't seem
        // to get a repro any smaller than this unfortunately.
        r"(?x)
            (?:
                \p{gcb=Prepend}*
                (?:
                    (?:
                        (?:
                            \p{gcb=L}*
                            (?:\p{gcb=V}+|\p{gcb=LV}\p{gcb=V}*|\p{gcb=LVT})
                            \p{gcb=T}*
                        )
                        |
                        \p{gcb=L}+
                        |
                        \p{gcb=T}+
                    )
                    |
                    \p{Extended_Pictographic}
                    (?:\p{gcb=Extend}*\p{gcb=ZWJ}\p{Extended_Pictographic})*
                    |
                    [^\p{gcb=Control}\p{gcb=CR}\p{gcb=LF}]
                )
                [\p{gcb=Extend}\p{gcb=ZWJ}\p{gcb=SpacingMark}]*
            )
        ";

    let dfa = dense::Builder::new()
        .minimize(true)
        .anchored(true)
        .build(pattern)
        .unwrap();
    assert_eq!(None, dfa.find(b"\xE2"));
}
