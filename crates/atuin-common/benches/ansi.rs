//! Benchmarks for [`atuin_common::ansi`].
//!
//! `to_plain_text` drives a terminal emulator over the entire captured output of a shell command,
//! so both its runtime and its peak memory matter; the allocation profiler is installed to keep an
//! eye on the latter.
//!
//! **Note** that these numbers understate the cost of a first call in a fresh process: divan calls
//! the function in a loop, so the allocator hands back the same (already faulted-in) pages every
//! iteration. Production calls it once per stream, per command. Read the `alloc` columns, not just
//! the timings, when judging a change.

use std::num::NonZeroU16;

use atuin_common::ansi;

#[global_allocator]
static ALLOC: divan::AllocProfiler = divan::AllocProfiler::system();

fn main() {
    // Run registered benchmarks.
    divan::main();
}

/// The screen geometry `atuin-ai` renders command output with: `PREVIEW_HEIGHT` x `PREVIEW_WIDTH`.
const ROWS: NonZeroU16 = NonZeroU16::new(10).unwrap();
const COLS: NonZeroU16 = NonZeroU16::new(120).unwrap();

/// The shapes of command output we care about. Changing these makes benchmarks irreproducible.
const CASES: &[&str] = &[
    "plain_small",
    "plain_medium",
    "plain_large",
    "ansi_colored",
    "progress_bar",
    "long_line",
    "crlf_medium",
];

/// Rendering command output to plain text, over each shape of output.
#[divan::bench(args = CASES)]
fn to_plain_text(bencher: divan::Bencher, case: &str) {
    // Generating the input is not part of what we measure, so it happens as an input.
    bencher.with_inputs(|| input(case)).bench_refs(|bytes| ansi::to_plain_text(bytes, ROWS, COLS));
}

/// How much the screen height costs. Everything above the last `rows` rows is captured on the way
/// past by the scroll callback, so a taller screen is not a cheaper one.
#[divan::bench(args = [1u16, 10, 24, 100, 1_002, 16_384])]
fn to_plain_text_by_rows(bencher: divan::Bencher, rows: u16) {
    let rows = NonZeroU16::new(rows).expect("row count must be nonzero");
    bencher
        .with_inputs(|| input("plain_medium"))
        .bench_refs(|bytes| ansi::to_plain_text(bytes, rows, COLS));
}

/// `onlcr` on its own: the streaming path feeds it every read from the child process.
#[divan::bench(args = CASES)]
fn onlcr(bencher: divan::Bencher, case: &str) {
    bencher.with_inputs(|| input(case)).bench_refs(|bytes| {
        let mut len = 0usize;
        ansi::onlcr(bytes.as_slice()).for_each(|chunk| len += chunk.len());
        len
    });
}

/// Build the input for a case named in [`CASES`].
fn input(case: &str) -> Vec<u8> {
    match case {
        // ~10 lines: what `git status` or a short `ls` looks like.
        "plain_small" => (0..10)
            .map(|i| format!("src/some/path/file_{i}.rs: everything looks fine here\n"))
            .collect::<String>()
            .into_bytes(),
        // ~1k lines / ~60 KiB: a normal build log.
        "plain_medium" => (0..1_000)
            .map(|i| format!("   Compiling some-crate-number-{i} v0.1.{i} (/home/user/code)\n"))
            .collect::<String>()
            .into_bytes(),
        // ~50k lines / ~3 MiB: a big log, e.g. a verbose build or a test suite.
        "plain_large" => (0..50_000)
            .map(|i| format!("   Compiling some-crate-number-{i} v0.1.{i} (/home/user/code)\n"))
            .collect::<String>()
            .into_bytes(),
        // 1k lines carrying SGR colour codes, like cargo or ripgrep output.
        "ansi_colored" => (0..1_000)
            .map(|i| {
                format!("\x1b[1;32m    Finished\x1b[0m \x1b[36mtarget {i}\x1b[0m in 0.0{i}s\n")
            })
            .collect::<String>()
            .into_bytes(),
        // 20k carriage-return rewrites of a single progress line, without a newline in sight.
        "progress_bar" => (0..20_000)
            .map(|i| format!("\rdownloading [{:=<40}] {}%", "", i % 100))
            .collect::<String>()
            .into_bytes(),
        // One 100 KiB line with no newline at all: nothing but soft wrapping.
        "long_line" => "x".repeat(100_000).into_bytes(),
        // 1k lines that already use CRLF, so `onlcr` has nothing to insert.
        "crlf_medium" => (0..1_000)
            .map(|i| format!("   Compiling some-crate-number-{i} v0.1.{i} (/home/user/code)\r\n"))
            .collect::<String>()
            .into_bytes(),
        other => panic!("unknown benchmark case: {other}"),
    }
}
