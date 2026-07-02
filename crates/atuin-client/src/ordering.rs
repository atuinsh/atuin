use minspan::minspan;

use super::{history::History, settings::SearchMode};

pub fn reorder_fuzzy(mode: SearchMode, query: &str, res: Vec<History>) -> Vec<History> {
    match mode {
        SearchMode::Fuzzy => reorder(query, |x| &x.command, res),
        _ => res,
    }
}

fn reorder<F, A>(query: &str, f: F, res: Vec<A>) -> Vec<A>
where
    F: Fn(&A) -> &String,
    A: Clone,
{
    let mut r = res.clone();
    let qvec = &query.chars().collect();
    r.sort_by_cached_key(|h| {
        let command = f(h);
        // TODO for fzf search we should sum up scores for each matched term
        let (from, to) = match minspan::span(qvec, &(command.chars().collect())) {
            Some(x) => x,
            // this is a little unfortunate: when we are asked to match a query that is found nowhere,
            // we don't want to return a None, as the comparison behaviour would put the worst matches
            // at the front. therefore, we'll return a set of indices that are one larger than the longest
            // possible legitimate match. This is meaningless except as a comparison.
            None => (0, res.len()),
        };
        (prefix_rank(command, query), 1 + to - from)
    });
    r
}

fn prefix_rank(command: &str, query: &str) -> u8 {
    let query = query.trim();
    if query.is_empty() {
        return 2;
    }

    let command = command.to_lowercase();
    let query = query.to_lowercase();

    if command.starts_with(&query) {
        0
    } else if command.contains(&query) {
        1
    } else {
        2
    }
}
