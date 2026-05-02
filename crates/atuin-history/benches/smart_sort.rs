use atuin_client::history::History;
use atuin_history::sort::sort;

use rand::Rng;

fn main() {
    // Run registered benchmarks.
    divan::main();
}

// Smart sort usually runs on 200 entries, test on a few sizes
#[divan::bench(args=[100, 200, 400, 800, 1600, 10000])]
fn smart_sort(lines: usize) {
    // benchmark a few different sizes of "history"
    // first we need to generate some history. This will use a whole bunch of memory, sorry
    let mut rng = rand::thread_rng();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();

    let possible_commands = ["echo", "ls", "cd", "grep", "atuin", "curl"];
    let mut commands = Vec::<History>::with_capacity(lines);

    for _ in 0..lines {
        let command = possible_commands[rng.gen_range(0..possible_commands.len())];

        let command = History::import()
            .command(command)
            .timestamp(time::OffsetDateTime::from_unix_timestamp(rng.gen_range(0..now)).unwrap())
            .build()
            .into();

        commands.push(command);
    }

    let _ = sort("curl", commands);
}
