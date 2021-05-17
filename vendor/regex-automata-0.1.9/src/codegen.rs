// This module is unused. It was written as an experiment to get a ballpark
// idea of what state machines look like when translated to Rust code, and
// in particular, an idea of how much code it generates. The implementation
// below isn't optimal with respect to size, but the result wasn't exactly
// small. At some point, we should pursue building this out beyond
// experimentation, and in particular, probably provide a command line tool
// and/or a macro. It's a fair bit of work, so I abandoned it for the initial
// release. ---AG

use std::collections::HashMap;
use std::io::Write;

use dense::DFA;
use state_id::StateID;

macro_rules! wstr {
    ($($tt:tt)*) => { write!($($tt)*).unwrap() }
}

macro_rules! wstrln {
    ($($tt:tt)*) => { writeln!($($tt)*).unwrap() }
}

pub fn is_match_forward<S: StateID>(dfa: &DFA<S>) -> String {
    let names = state_variant_names(dfa);

    let mut buf = vec![];
    wstrln!(buf, "pub fn is_match(input: &[u8])  -> bool {{");
    if dfa.is_match_state(dfa.start()) {
        wstrln!(buf, "    return true;");
        wstrln!(buf, "}}");
        return String::from_utf8(buf).unwrap();
    }

    wstrln!(buf, "{}", state_enum_def(dfa, &names));

    wstrln!(buf, "    let mut state = {};", names[&dfa.start()]);
    wstrln!(buf, "    for &b in input.iter() {{");
    wstrln!(buf, "        state = match state {{");
    for (id, s) in dfa.iter() {
        if dfa.is_match_state(id) {
            continue;
        }

        wstrln!(buf, "            {} => {{", &names[&id]);
        wstrln!(buf, "                match b {{");
        for (start, end, next_id) in s.sparse_transitions() {
            if dfa.is_match_state(next_id) {
                wstrln!(buf, "                    {:?}...{:?} => return true,", start, end);
            } else {
                if start == end {
                    wstrln!(buf, "                    {:?} => {},", start, &names[&next_id]);
                } else {
                    wstrln!(buf, "                    {:?}...{:?} => {},", start, end, &names[&next_id]);
                }
            }
        }
        wstrln!(buf, "                    _ => S::S0,");
        wstrln!(buf, "                }}");
        wstrln!(buf, "            }}");
    }
    wstrln!(buf, "        }};");
    wstrln!(buf, "    }}");

    wstrln!(buf, "    false");
    wstrln!(buf, "}}");
    String::from_utf8(buf).unwrap()
}

fn state_enum_def<S: StateID>(
    dfa: &DFA<S>,
    variant_names: &HashMap<S, String>,
) -> String {
    let mut buf = vec![];
    wstrln!(buf, "    #[derive(Clone, Copy)]");
    wstr!(buf, "    enum S {{");

    let mut i = 0;
    for (id, _) in dfa.iter() {
        if dfa.is_match_state(id) {
            continue;
        }
        if i % 10 == 0 {
            wstr!(buf, "\n       ");
        }
        let name = format!("S{}", id.to_usize());
        wstr!(buf, " {},", name);
        i += 1;
    }
    wstr!(buf, "\n");
    wstrln!(buf, "    }}");
    String::from_utf8(buf).unwrap()
}

fn state_variant_names<S: StateID>(dfa: &DFA<S>) -> HashMap<S, String> {
    let mut variants = HashMap::new();
    for (id, _) in dfa.iter() {
        if dfa.is_match_state(id) {
            continue;
        }
        variants.insert(id, format!("S::S{}", id.to_usize()));
    }
    variants
}
