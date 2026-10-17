fn main() {
    divan::main();
}

// Compiled straight into the bench binary, like the markers parser: see `benches/markers.rs`.
#[cfg(unix)]
#[allow(dead_code, reason = "the module's tests build in too, with no harness to run them")]
#[path = "../src/escape_tracker.rs"]
mod escape_tracker;

#[cfg(unix)]
mod unix {
    use divan::Bencher;
    use divan::counter::BytesCount;

    use super::escape_tracker::EscapeTracker;

    /// The forwarding thread's read buffer (`src/runtime.rs`), so the size of what the tracker is
    /// handed at a time.
    const CHUNK_BYTES: usize = 8192;

    /// Output like `ls -la --color`'s: mostly text, a colour change around every name, and names
    /// outside ASCII.
    fn listing(lines: usize) -> Vec<u8> {
        let line =
            "drwxr-xr-x  4 user user 4096 Jan  1 12:00 \x1b[01;34mrépertoire_ünïcødé\x1b[0m\r\n";
        line.repeat(lines).into_bytes()
    }

    #[divan::bench(args = [64, 1024, 16384])]
    fn push(bencher: Bencher, lines: usize) {
        bencher
            .with_inputs(|| listing(lines))
            .input_counter(|data| BytesCount::of_slice(data.as_slice()))
            .bench_values(|data| {
                let mut tracker = EscapeTracker::default();
                let titled = data
                    .chunks(CHUNK_BYTES)
                    .fold(false, |titled, chunk| tracker.push(chunk) | titled);
                divan::black_box((titled, tracker.at_ground()))
            });
    }
}
