fn main() {
    divan::main();
}

// Compile the parser straight into the bench binary rather than exposing it through the library's
// public API just to measure it. The module is self-contained (std + memchr), and `harness = false`
// means it builds without `cfg(test)`, so its test submodule is dropped. It lives at the crate root
// (not inside `mod unix`) so `#[path]` resolves against `benches/`, a directory that actually exists.
#[cfg(unix)]
#[allow(dead_code)] // only Parser/push are exercised here; the rest of the API is unused.
#[path = "../src/osc133.rs"]
mod osc133;

// The parser is unix-gated, so the benchmarks are too. On other platforms this
// leaves `divan::main()` with nothing registered, which is fine.
#[cfg(unix)]
mod unix {
    use divan::Bencher;

    use super::osc133;

    /// Build a stream of shell interactions: OSC 133 markers wrapping plain
    /// command output. Real terminal output is overwhelmingly plain text with
    /// occasional escape sequences, which is exactly the case the `Ground`-state
    /// scan handles — so the plain output dominates the buffer here too.
    fn stream(cycles: usize) -> Vec<u8> {
        let line = b"drwxr-xr-x  4 user user 4096 Jan  1 12:00 some_directory_name\n";
        let mut out = Vec::with_capacity(cycles * (line.len() * 20 + 64));
        for i in 0..cycles {
            out.extend_from_slice(b"\x1b]133;A\x07"); // prompt start
            out.extend_from_slice(b"user@host:~/project$ ");
            out.extend_from_slice(b"\x1b]133;B\x07"); // command start
            out.extend_from_slice(b"ls -la\r\n");
            out.extend_from_slice(b"\x1b]133;C\x07"); // output start
            for _ in 0..20 {
                out.extend_from_slice(line);
            }
            out.extend_from_slice(if i % 2 == 0 { b"\x1b]133;D;0\x07" } else { b"\x1b]133;D;1\x07" });
        }
        out
    }

    #[divan::bench(args = [64, 256, 1024])]
    fn parse(bencher: Bencher, cycles: usize) {
        bencher
            .with_inputs(|| stream(cycles))
            .input_counter(|data| divan::counter::BytesCount::of_slice(data.as_slice()))
            .bench_values(|data| {
                let mut parser = osc133::Parser::new();
                let mut chunks = parser.push(&data);
                let count = chunks.by_ref().count();
                divan::black_box(chunks.trailing_data());
                count
            });
    }
}
