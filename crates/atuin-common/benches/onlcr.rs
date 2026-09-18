use atuin_common::ansi::onlcr;
use divan::Bencher;

fn main() {
    divan::main();
}

/// Program output using bare `\n` line endings — the case `onlcr` rewrites to
/// `\r\n`, and the one that forces it to scan every line for newlines.
fn text(lines: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..lines {
        out.extend_from_slice(format!("line {i}: some terminal output goes here\n").as_bytes());
    }
    out
}

#[divan::bench(args = [256, 1024, 8192])]
fn scan(bencher: Bencher, lines: usize) {
    bencher
        .with_inputs(|| text(lines))
        .input_counter(|data| divan::counter::BytesCount::of_slice(data.as_slice()))
        // Sum the chunk lengths rather than collecting, so we measure the newline
        // scan itself and not a Vec allocation.
        .bench_values(|data| onlcr(&data).map(<[u8]>::len).sum::<usize>());
}
