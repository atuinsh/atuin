use minspan::minspan;

use super::database::DbSearchMode;
use super::history::History;

pub fn reorder_fuzzy(mode: DbSearchMode, terms: &[&str], res: Vec<History>) -> Vec<History> {
    match mode {
        DbSearchMode::Fuzzy => reorder(terms, |x| &x.command, res),
        DbSearchMode::Prefix | DbSearchMode::FullText => res,
    }
}

/// Ranks each term against the command on its own, then scores by the smallest window covering
/// every term it found. SQL emits one independent condition per term and imposes no order between
/// them, so a single concatenated query cannot represent what it matched: terms in the reverse
/// order to the command, alternation branches, and terms on the far side of a `^`/`$` anchor all
/// fail the subsequence check even though SQL matched them.
///
/// Terms are ordered by how many went unfound before width, so a row matching more of the query
/// always outranks one matching less, and a row matching nothing at all sorts last.
fn reorder<F, A>(terms: &[&str], f: F, res: Vec<A>) -> Vec<A>
where
    F: Fn(&A) -> &String,
{
    let terms: Vec<Vec<char>> = terms.iter().map(|term| term.chars().collect()).collect();

    let mut r = res;
    r.sort_by_cached_key(|h| {
        let command: Vec<char> = f(h).chars().collect();

        let mut unfound = 0usize;
        let (mut lo, mut hi) = (usize::MAX, 0usize);
        for term in &terms {
            match minspan::span(term, &command) {
                Some((from, to)) => {
                    lo = lo.min(from);
                    hi = hi.max(to);
                }
                None => unfound += 1,
            }
        }

        let width = if lo > hi { 0 } else { 1 + hi - lo };
        (unfound, width)
    });
    r
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;
    use crate::history::History;

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

    #[rstest]
    // A non-matching row must sort last, not ahead of a genuine match.
    #[case::nonmatch_sorts_last(vec!["screen"], vec!["screen", "hello"], vec!["screen", "hello"])]
    // A tight match outranks a loose one.
    #[case::matches_by_span(vec!["curl"], vec!["central urllib", "curl"], vec!["curl", "central urllib"])]
    // Several terms score by the window covering them all, so the tightly clustered row wins.
    #[case::clustered_terms_win(vec!["foo", "bar"], vec!["foo qux bar", "foo bar"], vec!["foo bar", "foo qux bar"])]
    // That window does not depend on the order the terms were given in.
    #[case::term_order_is_irrelevant(vec!["bar", "foo"], vec!["foo qux bar", "foo bar"], vec!["foo bar", "foo qux bar"])]
    // A row matching both terms beats a tighter row matching only one.
    #[case::more_terms_beats_tighter(vec!["foo", "bar"], vec!["foo", "foo bar"], vec!["foo bar", "foo"])]
    fn reorder_ranks(
        #[case] terms: Vec<&str>,
        #[case] input: Vec<&str>,
        #[case] expected: Vec<&str>,
    ) {
        let res = input.into_iter().map(hist).collect();
        let out = reorder_fuzzy(DbSearchMode::Fuzzy, &terms, res);
        assert_eq!(commands(out), expected);
    }
}
