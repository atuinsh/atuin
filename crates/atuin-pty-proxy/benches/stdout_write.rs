fn main() {
    divan::main();
}

// The stdout-forwarding thread (src/runtime.rs) copies every PTY read to fd 1.
// This measures the two ways of doing that over identical, realistic input:
//
//   OLD: a `std::io::LineWriter` (what `std::io::stdout()` is) plus an explicit
//        `flush()` after each read. A read that ends mid-line — the common case,
//        since output usually finishes on a prompt fragment — is written as the
//        newline-terminated prefix (one `write`) with the tail buffered, then the
//        `flush()` emits that tail as a second `write`: two syscalls per read.
//   NEW: `WriteAllExt::write_all_retrying` (the shipped helper): one syscall per read.
//
// The OLD forwarder is buried in `run` and can't be reached from a bench, so it is
// replicated here; NEW calls the real helper. stdout can't be used from a bench, so
// both write to /dev/null, whose per-write syscall cost is what this isolates.
#[cfg(unix)]
mod unix {
    use std::fs::{File, OpenOptions};
    use std::io::{LineWriter, Write};
    use std::os::fd::AsFd;
    use std::time::Duration;

    use atuin_common::os::unix::io::WriteAllExt;
    use divan::Bencher;
    use divan::counter::ItemsCount;

    // A single `read()` from a 8 KiB PTY buffer during interactive use: a couple
    // of finished output lines followed by a fresh prompt, so it carries embedded
    // newlines but ends mid-line. This is exactly the shape that costs the
    // LineWriter a second syscall.
    const CHUNK: &[u8] = b"file_a.rs  file_b.rs  file_c.rs\r\ntarget  Cargo.toml\r\nuser@host ~/project (main) $ ";

    // Roughly a screen-second of interactive output.
    const CHUNKS: usize = 2000;

    fn chunks() -> Vec<&'static [u8]> {
        vec![CHUNK; CHUNKS]
    }

    fn devnull() -> File {
        OpenOptions::new().write(true).open("/dev/null").unwrap()
    }

    /// Mirrors the current forwarder: buffered LineWriter with a flush per read.
    #[divan::bench]
    fn line_writer_flush(bencher: Bencher) {
        bencher
            .with_inputs(|| (chunks(), devnull()))
            .input_counter(|_| ItemsCount::new(CHUNKS))
            .bench_values(|(chunks, file)| {
                let mut w = LineWriter::new(file);
                for chunk in &chunks {
                    w.write_all(chunk).unwrap();
                    w.flush().unwrap();
                }
                divan::black_box(&mut w);
            });
    }

    /// Mirrors the fix: one raw `write` loop per read, no buffering, no flush.
    #[divan::bench]
    fn raw_fd_write(bencher: Bencher) {
        bencher
            .with_inputs(|| (chunks(), devnull()))
            .input_counter(|_| ItemsCount::new(CHUNKS))
            .bench_values(|(chunks, file)| {
                let fd = file.as_fd();
                for chunk in &chunks {
                    fd.write_all_retrying(divan::black_box(chunk), Duration::MAX).unwrap();
                }
                divan::black_box(&file);
            });
    }
}
