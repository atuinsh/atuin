use minspan::minspan;

use super::{database::DbSearchMode, history::History};

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
///
/// Each term folds case exactly where SQL did. Smart case sends an uppercase term to GLOB, which
/// is case-sensitive, and a lowercase one to LIKE, which folds ASCII case; ranking a lowercase
/// term without folding would score a row SQL matched only by folding as a non-match.
fn reorder<F, A>(terms: &[&str], f: F, res: Vec<A>) -> Vec<A>
where
    F: Fn(&A) -> &String,
{
    let mut r = res;

    // With nothing to rank on every key is equal, and the stable sort would return the input.
    if terms.is_empty() || r.len() < 2 {
        return r;
    }

    // Paired with whether SQL matched the term case-sensitively.
    let terms: Vec<(Vec<char>, bool)> = terms
        .iter()
        .map(|term| (term.chars().collect(), term.contains(char::is_uppercase)))
        .collect();

    let any_folded = terms.iter().any(|(_, exact)| !*exact);

    // Refilled per row rather than reallocated: this runs over every result on every keystroke.
    let mut command: Vec<char> = Vec::new();
    let mut folded: Vec<char> = Vec::new();

    r.sort_by_cached_key(|h| {
        // Whether the command carries case is known by the time it has been walked once.
        command.clear();
        let mut has_ascii_case = false;
        for c in f(h).chars() {
            has_ascii_case |= c.is_ascii_uppercase();
            command.push(c);
        }

        // Folding per character keeps offsets aligned with `command`, over the same ASCII-only range
        // LIKE folds. A command carrying no ASCII case folds to itself, so it needs no copy.
        let fold = any_folded && has_ascii_case;
        if fold {
            folded.clear();
            folded.extend(command.iter().map(char::to_ascii_lowercase));
        }

        let mut unfound = 0usize;
        let (mut lo, mut hi) = (usize::MAX, 0usize);
        for (term, exact) in &terms {
            let haystack = if fold && !*exact { &folded } else { &command };
            match minspan::span(term, haystack) {
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
    use super::*;
    use crate::history::History;
    use rstest::rstest;
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
