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
        // TODO for fzf search we should sum up scores for each matched term
        let (from, to) = match minspan::span(qvec, &(f(h).chars().collect())) {
            Some(x) => x,
            // this is a little unfortunate: when we are asked to match a query that is found nowhere,
            // we don't want to return a None, as the comparison behaviour would put the worst matches
            // at the front. therefore, we'll return a set of indices that are one larger than the longest
            // possible legitimate match. This is meaningless except as a comparison.
            None => (0, res.len()),
        };
        1 + to - from
    });
    r
}
