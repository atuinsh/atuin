//! Head-to-head for the per-cell character rendering in the history list
//! (command/client/search/history_list.rs `command` -> `draw`).
//!
//! Drawing a command row iterates `display.char_indices()` and hands each
//! character to `draw(&str, ..)` one cell at a time. The OLD path built a
//! fresh heap `String` per character via `ch.to_string()`; the NEW path
//! encodes the char into a 4-byte stack buffer with `ch.encode_utf8`.
//!
//! Both variants feed the resulting `&str` through an identical `draw`-like
//! sink, so the only measured difference is the per-character allocation.

fn main() {
    divan::main();
}

/// A realistic long command: mixed ASCII plus a few multi-byte characters,
/// roughly the width of a wide-terminal history row (~120 chars).
fn command() -> String {
    let s = "git commit -m \"café: résumé the naïve piñata — 日本語 test\" && cargo build --release --features sqlite,daemon 2>&1 | grep -i warning";
    debug_assert!((110..=130).contains(&s.chars().count()));
    s.to_string()
}

/// Stand-in for `draw`: consume the `&str` the way `Buffer::set_stringn` does,
/// walking its characters into a reused cell buffer. Identical for both paths.
#[inline]
fn draw(sink: &mut String, s: &str) {
    for ch in divan::black_box(s).chars() {
        sink.push(divan::black_box(ch));
    }
}

#[divan::bench(min_time = 1)]
fn old_to_string(bencher: divan::Bencher) {
    bencher.with_inputs(command).bench_values(|display: String| {
        let mut sink = String::with_capacity(display.len());
        for (_i, ch) in display.char_indices() {
            draw(&mut sink, &ch.to_string());
        }
        divan::black_box(sink);
    });
}

#[divan::bench(min_time = 1)]
fn new_encode_utf8(bencher: divan::Bencher) {
    bencher.with_inputs(command).bench_values(|display: String| {
        let mut sink = String::with_capacity(display.len());
        for (_i, ch) in display.char_indices() {
            let mut buf = [0u8; 4];
            draw(&mut sink, ch.encode_utf8(&mut buf));
        }
        divan::black_box(sink);
    });
}
