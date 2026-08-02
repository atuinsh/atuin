//! Benchmarks for the matcher which powers Atuin's interactive search.
//!
//! Every keystroke re-scores the candidate list, so the matcher is one of the most
//! latency-sensitive pieces of code in the client.
//!
//! The corpus is a deterministic set of synthetic shell commands: using a bare `rand` accessor
//! would make these benchmarks non-reproducible.

use atuin_nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use atuin_nucleo_matcher::{Config, Matcher, Utf32String};

fn main() {
    // Run registered benchmarks.
    divan::main();
}

/// Corpus sizes. The interactive search hardcodes a limit of 200 deduplicated entries, while the
/// daemon index matches against everything it knows about.
const CORPUS_SIZES: [usize; 3] = [200, 1_000, 10_000];

/// Queries covering the atom kinds a user can type: fuzzy, multi word, substring (`'`) and
/// prefix (`^`).
const QUERIES: [&str; 4] = ["gitcm", "cargo build", "'docker", "^ls"];

/// Seed of the corpus generator. Changing it results in irreproducible benchmarks.
const SEED: u64 = 0x853c_49e6_748f_ea9b;

const PROGRAMS: [&str; 8] = [
    "git",
    "cargo",
    "docker compose",
    "ls",
    "grep -rn",
    "curl -sSL",
    "kubectl get",
    "atuin search",
];

const ARGS: [&str; 6] = [
    "commit --amend",
    "build --release",
    "up -d",
    "-la",
    "--follow",
    "status",
];

const PATHS: [&str; 6] = [
    "crates/atuin-client/src/history.rs",
    "~/dev/atuin",
    "/tmp/scratch",
    "docs/src/config.md",
    "https://api.atuin.sh/sync/status",
    "target/release/atuin",
];

#[divan::bench(args = CORPUS_SIZES, min_time = 1)]
fn fuzzy_match(bencher: divan::Bencher, n: usize) {
    let haystacks: Vec<Utf32String> = corpus(n).iter().map(|cmd| cmd.as_str().into()).collect();
    let needle: Utf32String = "gitcm".into();

    bencher
        .with_inputs(|| Matcher::new(Config::DEFAULT))
        .bench_local_refs(|matcher| {
            let mut total: u32 = 0;
            for haystack in &haystacks {
                if let Some(score) = matcher.fuzzy_match(haystack.slice(..), needle.slice(..)) {
                    total += u32::from(score);
                }
            }
            total
        });
}

/// Scores and ranks the whole candidate list, which is what a single keystroke does in the UI.
#[divan::bench(args = QUERIES, min_time = 1)]
fn match_list(bencher: divan::Bencher, query: &str) {
    let haystacks = corpus(1_000);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);

    bencher
        .with_inputs(|| Matcher::new(Config::DEFAULT))
        .bench_local_refs(|matcher| pattern.match_list(&haystacks, matcher));
}

/// Build a deterministic list of shell-like commands.
fn corpus(n: usize) -> Vec<String> {
    let mut state = SEED;

    (0..n)
        .map(|_| {
            let program = PROGRAMS[next(&mut state) % PROGRAMS.len()];
            let arg = ARGS[next(&mut state) % ARGS.len()];
            let path = PATHS[next(&mut state) % PATHS.len()];

            format!("{program} {arg} {path}")
        })
        .collect()
}

/// xorshift64: a tiny, stable pseudo random number generator, so that the corpus does not depend on
/// an external crate or on the platform.
fn next(state: &mut u64) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;

    (*state >> 1) as usize
}
