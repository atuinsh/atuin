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
{
    let mut r = res;
    let qvec = &query.chars().collect();
    r.sort_by_cached_key(|h| {
        // TODO for fzf search we should sum up scores for each matched term
        // A non-matching row sorts last rather than by a sentinel that can undercut a real match.
        minspan::span(qvec, &(f(h).chars().collect()))
            .map_or(usize::MAX, |(from, to)| 1 + to - from)
    });
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::History;
    use time::OffsetDateTime;

    fn hist(command: &str) -> History {
        History::capture()
            .timestamp(OffsetDateTime::now_utc())
            .command(command)
            .cwd("/")
            .build()
            .into()
    }

    fn commands(res: Vec<History>) -> Vec<String> {
        res.into_iter().map(|h| h.command).collect()
    }

    // A non-matching row must sort last, not ahead of a genuine match.
    #[test]
    fn reorder_nonmatch_sorts_last() {
        let res = vec![hist("screen"), hist("hello")];
        let out = reorder_fuzzy(SearchMode::Fuzzy, "screen", res);
        assert_eq!(commands(out), vec!["screen", "hello"]);
    }

    // The unchanged match path: a tight match outranks a loose one.
    #[test]
    fn reorder_matches_by_span() {
        let res = vec![hist("central urllib"), hist("curl")];
        let out = reorder_fuzzy(SearchMode::Fuzzy, "curl", res);
        assert_eq!(commands(out), vec!["curl", "central urllib"]);
    }
}
