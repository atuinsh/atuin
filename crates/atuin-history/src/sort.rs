use atuin_client::history::History;
use atuin_client::settings::SearchMode;

type ScoredHistory = (f64, History);

// Fuzzy search already comes sorted by minspan
// This sorting should be applicable to all search modes, and solve the more "obvious" issues
// first.
// Later on, we can pass in context and do some boosts there too.
#[must_use]
pub fn sort(query: &str, search_mode: SearchMode, input: Vec<History>) -> Vec<History> {
    // This can totally be extended. We need to be _careful_ that it's not slow.
    // We also need to balance sorting db-side with sorting here. SQLite can do a lot,
    // but some things are just much easier/more doable in Rust.

    let mut scored = input
        .into_iter()
        .map(|h| {
            // If history is _prefixed_ with the query, score it more highly.
            //
            // In fulltext mode the query semantically means "*query*" (matches anywhere), so
            // boosting prefix matches over other substring matches defeats the mode: a match
            // buried mid-command would always rank below every prefix match no matter how old,
            // instead of being ranked primarily by recency as the docs promise.
            let score = if search_mode == SearchMode::FullText {
                // Mirror the smart-case behavior of the DB-side LIKE/GLOB query
                // (QueryToken::has_uppercase in database.rs): an all-lowercase query
                // matches case-insensitively, so a lowercase-only query must not
                // demote differently-cased commands that the DB already matched.
                let matches = if query.contains(char::is_uppercase) {
                    h.command.contains(query)
                } else {
                    h.command.to_lowercase().contains(&query.to_lowercase())
                };
                if matches {
                    2.0
                } else {
                    1.0
                }
            } else if h.command.starts_with(query) {
                2.0
            } else if h.command.contains(query) {
                1.75
            } else {
                1.0
            };

            // calculate how long ago the history was, in seconds
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            let time = h.timestamp.unix_timestamp();
            let diff = std::cmp::max(1, now - time); // no /0 please

            // prefer newer history, but not hugely so as to offset the other scoring
            // the numbers will get super small over time, but I don't want time to overpower other
            // scoring
            #[allow(clippy::cast_precision_loss)]
            let time_score = 1.0 + (1.0 / diff as f64);
            let score = score * time_score;

            (score, h)
        })
        .collect::<Vec<ScoredHistory>>();

    scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().reverse());

    // Remove the scores and return the history
    scored.into_iter().map(|(_, h)| h).collect::<Vec<History>>()
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::*;

    // Both timestamps are in the past relative to real wall-clock time, and far enough
    // apart that the recency multiplier can't meaningfully close a 2.0-vs-1.0 score gap;
    // this isolates the assertions to the prefix/contains scoring behavior under test.
    fn history_ago(command: &str, seconds_ago: i64) -> History {
        let timestamp = OffsetDateTime::now_utc() - time::Duration::seconds(seconds_ago);
        History::import().command(command).timestamp(timestamp).build().into()
    }

    // Regression test for https://github.com/atuinsh/atuin/issues/3923: in fulltext mode a
    // recent command matching the query mid-string should not be ranked below every older
    // command that merely starts with the query, since fulltext explicitly means "*query*".
    #[test]
    fn fulltext_does_not_prefer_prefix_match_over_recency() {
        let recent_substring_match = history_ago("ps axu|grep -i git", 10);
        let old_prefix_match = history_ago("grep -r foo", 1_000_000);

        let input = vec![old_prefix_match.clone(), recent_substring_match.clone()];
        let sorted = sort("grep", SearchMode::FullText, input);

        assert_eq!(sorted[0].command, recent_substring_match.command);
        assert_eq!(sorted[1].command, old_prefix_match.command);
    }

    // Prefix mode is unaffected: an exact prefix match should still be able to outrank a more
    // recent, merely-contained match.
    #[test]
    fn prefix_mode_still_prefers_prefix_match() {
        let recent_substring_match = history_ago("ps axu|grep -i git", 10);
        let old_prefix_match = history_ago("grep -r foo", 1_000_000);

        let input = vec![recent_substring_match.clone(), old_prefix_match.clone()];
        let sorted = sort("grep", SearchMode::Prefix, input);

        assert_eq!(sorted[0].command, old_prefix_match.command);
        assert_eq!(sorted[1].command, recent_substring_match.command);
    }

    // Regression test for a case-sensitivity bug caught in review: the DB-side fulltext query
    // matches case-insensitively for an all-lowercase query, so a differently-cased command
    // must not be treated as a non-match here and demoted below an older exact-case one.
    #[test]
    fn fulltext_lowercase_query_matches_case_insensitively() {
        let recent_different_case = history_ago("GIT status", 10);
        let old_exact_case = history_ago("git log", 1_000_000);

        let input = vec![old_exact_case.clone(), recent_different_case.clone()];
        let sorted = sort("git", SearchMode::FullText, input);

        assert_eq!(sorted[0].command, recent_different_case.command);
        assert_eq!(sorted[1].command, old_exact_case.command);
    }

    // A query containing uppercase opts into case-sensitive matching, mirroring the DB-side
    // GLOB behavior for such queries (QueryToken::has_uppercase in database.rs): a differently
    // cased command should score as a non-match, not be treated as equivalent.
    #[test]
    fn fulltext_uppercase_query_matches_case_sensitively() {
        let recent_wrong_case = history_ago("git status", 10);
        let old_exact_case = history_ago("STATUS log", 1_000_000);

        let input = vec![recent_wrong_case.clone(), old_exact_case.clone()];
        let sorted = sort("STATUS", SearchMode::FullText, input);

        assert_eq!(sorted[0].command, old_exact_case.command);
        assert_eq!(sorted[1].command, recent_wrong_case.command);
    }
}
