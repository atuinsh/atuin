use atuin_client::history::History;

type ScoredHistory = (f64, History);

// Fuzzy search already comes sorted by minspan
// This sorting should be applicable to all search modes, and solve the more "obvious" issues
// first.
// Later on, we can pass in context and do some boosts there too.
pub fn sort(query: &str, input: Vec<History>) -> Vec<History> {
    // This can totally be extended. We need to be _careful_ that it's not slow.
    // We also need to balance sorting db-side with sorting here. SQLite can do a lot,
    // but some things are just much easier/more doable in Rust.

    let mut scored = input
        .into_iter()
        .map(|h| {
            // If history is _prefixed_ with the query, score it more highly
            let score = if h.command.starts_with(query) {
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
