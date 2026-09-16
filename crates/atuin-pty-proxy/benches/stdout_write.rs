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
//   NEW: a raw `rustix::io::write` loop straight to the fd: one syscall per read.
//
// Neither approach is reachable from a bench (the forwarder is buried in `run`),
// so both are replicated here; the real fix lives in src/runtime.rs. stdout can't
// be used from a bench, so both write to /dev/null, whose per-write syscall cost
// is what the comparison isolates.
#[cfg(unix)]
mod unix {
    use std::fs::{File, OpenOptions};
    use std::io::{LineWriter, Write};
    use std::os::fd::AsFd;

    use divan::Bencher;
    use divan::counter::ItemsCount;
    use rustix::fd::BorrowedFd;
    use rustix::io::Errno;

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
                    write_all_fd(fd, divan::black_box(chunk)).unwrap();
                }
                divan::black_box(&file);
            });
    }

    fn write_all_fd(fd: BorrowedFd<'_>, mut data: &[u8]) -> Result<(), Errno> {
        while !data.is_empty() {
            match rustix::io::write(fd, data) {
                Ok(0) => return Err(Errno::IO),
                Ok(n) => data = &data[n..],
                Err(Errno::INTR | Errno::AGAIN) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}
