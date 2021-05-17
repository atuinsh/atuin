use std::time::Instant;

use indicatif::{HumanDuration, ProgressBar};

fn many_units_of_easy_work(n: u64, label: &str, draw_delta: Option<u64>) {
    let pb = ProgressBar::new(n);
    if let Some(v) = draw_delta {
        pb.set_draw_delta(v);
    }

    let mut sum = 0;
    let started = Instant::now();
    for i in 0..n {
        // Any quick computation, followed by an update to the progress bar.
        sum += 2 * i + 3;
        pb.inc(1);
    }
    pb.finish();
    let finished = started.elapsed();

    println!(
        "[{}] Sum ({}) calculated in {}",
        label,
        sum,
        HumanDuration(finished)
    );
}

fn main() {
    const N: u64 = 1 << 20;

    // Perform a long sequence of many simple computations monitored by a
    // default progress bar.
    many_units_of_easy_work(N, "Default progress bar ", None);

    // Perform the same sequence of many simple computations, but only redraw
    // after each 0.005% of additional progress.
    many_units_of_easy_work(N, "Draw delta is 0.005% ", Some(N / (5 * 100000)));

    // Perform the same sequence of many simple computations, but only redraw
    // after each 0.01% of additional progress.
    many_units_of_easy_work(N, "Draw delta is 0.01%  ", Some(N / 10000));
}
