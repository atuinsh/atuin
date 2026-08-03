use atuin_client::history::History;
use atuin_history::sort::sort;

use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;
use time::OffsetDateTime;
use time::macros::datetime;

fn main() {
    // Run registered benchmarks.
    divan::main();
}

// Changing any of these values will result in irreproducible benchmarks.
const SEED_RNG: u64 = 42;
const SEED_NOW: OffsetDateTime = datetime!(2026-01-01 12:59:59 -5);

// Smart sort usually runs on 200 entries, test on a few sizes
#[divan::bench(args=[100, 200, 400, 800, 1600, 10000])]
fn smart_sort(bencher: divan::Bencher, lines: usize) {
    // Generating the history is not part of what we measure, so it happens as an input.
    bencher
        .with_inputs(|| history(lines))
        .bench_values(|commands| sort("curl", commands));
}

/// Generate a few different sizes of "history". This will use a whole bunch of memory, sorry.
fn history(lines: usize) -> Vec<History> {
    // A seeded generator keeps the benchmark deterministic across runs.
    let mut rng = StdRng::seed_from_u64(SEED_RNG);
    let now = SEED_NOW.unix_timestamp();

    let possible_commands = ["echo", "ls", "cd", "grep", "atuin", "curl"];
    let mut commands = Vec::<History>::with_capacity(lines);

    for _ in 0..lines {
        let command = possible_commands[rng.gen_range(0..possible_commands.len())];

        let command = History::import()
            .command(command)
            .timestamp(OffsetDateTime::from_unix_timestamp(rng.gen_range(0..now)).unwrap())
            .build()
            .into();

        commands.push(command);
    }

    commands
}
