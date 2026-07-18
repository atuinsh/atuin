# Vt100PlainTextExt Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extract the duplicated "feed raw terminal bytes through a `vt100` parser and read back plain text" logic from `atuin-pty-proxy` and `atuin-ai` into one well-tested extension trait `Vt100PlainTextExt` in `atuin-common`, behind a `vt100` feature gate.

**Architecture:** A new top-level module `atuin_common::vt100`, compiled only under the `vt100` feature. It exposes `trait Vt100PlainTextExt: AsRef<[u8]>` with a single default method `to_plain_text(&self, cols: u16) -> String`. The method reconciles the two existing implementations by *always* applying ONLCR newline-normalization (a safe no-op when a `\r` already precedes an `\n`, so it does not change PTY-sourced input) before parsing, and by post-processing the screen contents with the pty-proxy trim/pop logic (a near no-op on top of `vt100`'s already-trimmed `contents()`). Both call-site crates switch to the trait; their local helpers are deleted, except the streaming-preview helpers in `atuin-ai` (`normalize_newlines_for_vt100`, `vt100_screen_lines`) which serve a different purpose and stay.

**Tech Stack:** Rust (edition 2024), `vt100` 0.16 (workspace dep), `rstest` + `proptest` for tests, cargo features for optional compilation.

---

## Background & Reconciliation Notes

Read this before starting. It captures *why* the unified algorithm looks the way it does.

**Two existing implementations:**

1. `crates/atuin-pty-proxy/src/capture.rs:169-197` — `render_plain_text(bytes, cols)`:
   - No ONLCR normalization (PTY kernel driver already emits `\r\n`).
   - `estimated_rows`: `(count('\n') + 1) + bytes.len()/cols + 1`, clamped to `[1, 10_000]`.
   - `vt100::Parser::new(rows, cols, 0)`, `process`, then `normalize_screen_contents`: split `contents()` into lines, `trim_end` each, pop trailing empty lines, `join("\n")`.
   - Called at `capture.rs:141,142,145` (prompt, command, output). `command` result is additionally `.trim_matches('\r'|'\n')`.

2. `crates/atuin-ai/src/tools/mod.rs:896-920` — `strip_ansi_via_vt100(raw)`:
   - Applies ONLCR normalization first via `normalize_newlines_for_vt100` (pipe capture has bare `\n`).
   - Fixed `cols = PREVIEW_WIDTH = 120`.
   - `estimated_rows`: `(count('\n') + len/120 + 1).min(10_000)`.
   - Uses `screen.contents()` directly (no trim/pop; `vt100`'s `contents()` already trims trailing per-line whitespace and trailing blank rows).
   - Called at `mod.rs:1051,1052` (stdout, stderr).

**Why always-ONLCR is safe:** `normalize_newlines_for_vt100` inserts `\r` before a `\n` *only when the previous byte is not already `\r`*. PTY input is already `\r\n`, so the insert never fires → byte-for-byte identical input to the parser → pty-proxy behavior is preserved. Verified against every pty-proxy test input (backspace-echo cases have no `\n`; the ANSI case uses `\r\n` where `\r` already precedes). This lets one code path serve both crates.

**Why the trim/pop post-processing is safe for atuin-ai:** `vt100::Screen::contents()` already trims trailing whitespace per line and removes trailing blank rows, so re-applying `trim_end` + pop-trailing-empties is idempotent on that output. Net effect on atuin-ai: unchanged output (strictly no less clean).

**Unified algorithm (`to_plain_text`):**
```
empty input            -> String::new()
cols = cols.max(1)
normalized = onlcr(bytes)              // insert \r before bare \n
rows = estimated_rows(&normalized, cols)   // clamp [1, MAX_ROWS]
parser = vt100::Parser::new(rows, cols, 0); parser.process(&normalized)
normalize_screen_contents(&parser.screen().contents())
```

**Scope boundaries:**
- IN: replace `render_plain_text` (3 pty call sites) and `strip_ansi_via_vt100` (2 ai call sites) — 5 sites total.
- OUT: `normalize_newlines_for_vt100` and `vt100_screen_lines` remain in `atuin-ai` — they drive the incremental live-preview parser (fixed `PREVIEW_HEIGHT` viewport, `Vec<String>` output), a different shape than whole-buffer plain-text extraction. They stay used, so no dead code.

**Convention to mirror:** `crates/atuin-common/src/string/escape_non_printable_posix_ext.rs` — `pub trait XExt: AsRef<...>` with a default method, a blanket `impl<T: AsRef<...>> XExt for T {}`, rich `///` docs, and a `#[cfg(test)]` module using `#[rstest]` table cases + `proptest!` invariants.

---

### Task 1: Add the `vt100` feature and optional dependency to `atuin-common`

**Files:**
- Modify: `crates/atuin-common/Cargo.toml`

**Step 1: Add the optional dependency**

In `crates/atuin-common/Cargo.toml`, under `[dependencies]`, add (keep alphabetical-ish grouping with the other optional deps near `unicode-*`):

```toml
vt100 = { workspace = true, optional = true }
```

**Step 2: Add the feature**

Under `[features]`, add after the `unicode` feature block:

```toml
# The `vt100` module: `Vt100PlainTextExt`, which renders raw terminal byte
# streams (ANSI escapes, backspaces, carriage returns, cursor motion) into
# clean plain text via a `vt100` terminal emulator. Compiled only when enabled.
vt100 = ["dep:vt100"]
```

**Step 3: Verify it resolves**

Run: `cargo tree -p atuin-common --features vt100 -i vt100`
Expected: shows `vt100 v0.16.x` pulled in by `atuin-common`.

Run: `cargo check -p atuin-common` (no features)
Expected: PASS, and `vt100` is NOT compiled (it is optional + unused without the feature).

**Step 4: Commit**

```bash
git add crates/atuin-common/Cargo.toml
git commit -m "build(atuin-common): add optional vt100 feature and dependency"
```

---

### Task 2: Write the failing test module for `Vt100PlainTextExt`

We write tests first (TDD). The module file is created with the test module and a stub so the crate compiles and the tests *fail meaningfully*.

**Files:**
- Create: `crates/atuin-common/src/vt100.rs`
- Modify: `crates/atuin-common/src/lib.rs`

**Step 1: Register the module (feature-gated) in `lib.rs`**

In `crates/atuin-common/src/lib.rs`, add alongside the other `pub mod` declarations (keep alphabetical: after `pub mod utils;` is fine, or grouped near the top-level mods):

```rust
#[cfg(feature = "vt100")]
pub mod vt100;
```

**Step 2: Create `crates/atuin-common/src/vt100.rs` with docs, a stub, and the full test suite**

Write the file with the trait declared but the method body left as `todo!()` so tests compile and fail at runtime:

```rust
//! Rendering raw terminal byte streams into clean plain text.
//!
//! Terminal programs emit far more than the characters you see: ANSI SGR color
//! codes, cursor-movement escapes, carriage returns that rewrite the current
//! line, backspaces that erase already-typed characters, progress bars, and
//! bracketed-paste / OSC sequences. Naively stripping "escape-looking" bytes
//! with a regex gets this wrong constantly.
//!
//! [`Vt100PlainTextExt`] takes the correct approach: it feeds the bytes through
//! an actual VT100 terminal emulator ([`vt100`]) and reads back the resulting
//! screen contents. Whatever a real terminal would *display* is what you get —
//! nothing more, nothing less.

use std::borrow::Cow;

/// Upper bound on the emulated screen height, in rows.
///
/// The row count is estimated from the input so scrollback is preserved without
/// wrapping, but a pathological input (millions of newlines) must not be able to
/// make us allocate an unbounded grid. Output is therefore capped at this many
/// lines.
const MAX_ROWS: usize = 10_000;

/// Extension trait that renders raw terminal byte streams into clean plain text.
///
/// Implemented for anything that is `AsRef<[u8]>` (e.g. `Vec<u8>`, `&[u8]`,
/// `[u8; N]`), so you can call [`to_plain_text`](Vt100PlainTextExt::to_plain_text)
/// directly on captured output buffers.
///
/// # What it does
///
/// The bytes are fed through a [`vt100`] terminal emulator sized to hold the
/// full output (bounded by [`MAX_ROWS`]), and the emulator's final screen
/// contents are returned. This means:
///
/// - ANSI escape sequences (colors, cursor motion, screen clears, OSC/DCS) are
///   interpreted and removed, leaving only displayed text.
/// - Carriage returns (`\r`) that rewrite a line — as used by progress bars —
///   resolve to the final line contents.
/// - Backspaces (`\x08`) erase the preceding character, so terminal echo edits
///   collapse to what the user actually left on screen.
/// - Bare line feeds (`\n`) are treated as newlines even when the source did not
///   emit a carriage return (pipe-captured output), matching a terminal driver's
///   `ONLCR` behavior. This is a no-op for input that already uses `\r\n`.
/// - Trailing whitespace on each line and trailing blank lines are trimmed.
///
/// The result contains no terminal control characters. The only C0 controls that
/// may remain are `\n` (line separators) and `\t` (if the emulator preserved a
/// literal tab; tabs are typically expanded to spaces).
///
/// # Cost
///
/// This parses the entire input through a terminal emulator and allocates a grid
/// up to `cols * MAX_ROWS` cells. It is intended for post-hoc cleanup of captured
/// command output, not for hot loops.
pub trait Vt100PlainTextExt: AsRef<[u8]> {
    /// Render the bytes to plain text as they would appear on a `cols`-wide
    /// terminal.
    ///
    /// `cols` is the emulated terminal width; long lines wrap at this boundary.
    /// A `cols` of `0` is treated as `1`. Empty input yields an empty string.
    ///
    /// See the [trait docs](Vt100PlainTextExt) for the full list of transforms.
    fn to_plain_text(&self, cols: u16) -> String {
        todo!("implemented in Task 3")
    }
}

impl<T: AsRef<[u8]> + ?Sized> Vt100PlainTextExt for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rstest::rstest;

    /// Assert the rendered text carries no terminal control characters other
    /// than the line/column separators a plain-text document legitimately uses.
    fn assert_no_terminal_controls(text: &str) {
        assert!(
            !text
                .chars()
                .any(|ch| ch.is_control() && ch != '\n' && ch != '\t'),
            "text still contains terminal controls: {text:?}"
        );
    }

    /// Table of raw-input -> expected-plain-text cases. These consolidate the
    /// original pty-proxy and atuin-ai unit tests plus additional edge cases.
    #[rstest]
    // Plain text is passed straight through.
    #[case::plain("echo hi", "echo hi")]
    // Empty input -> empty output.
    #[case::empty("", "")]
    // Backspace erases the preceding character (terminal echo edit).
    #[case::single_backspace("e\x08echo hi", "echo hi")]
    // A storm of backspaces still collapses to the final line.
    #[case::backspace_storm(
        "e\x08echo\x08 \x08\x08 \x08\x08\x08e \x08\x08 \x08e\x08echo hi",
        "echo hi"
    )]
    // SGR color codes and a zsh "no trailing newline" `%` marker are removed.
    #[case::ansi_and_percent_marker(
        "\x1b[32mhi\x1b[0m\r\n%                                    \r \r",
        "hi"
    )]
    // Multi-byte UTF-8 survives a backspace edit in the middle of the line.
    #[case::utf8_after_backspace("🦀x\x08 \x08 crab", "🦀 crab")]
    // A carriage-return progress-bar style rewrite keeps only the final text.
    #[case::carriage_return_rewrite("aaaa\rbbbb", "bbbb")]
    // Bare LF (pipe capture) is treated as a newline, so lines start at column 0.
    #[case::bare_lf_is_newline("line one\nline two", "line one\nline two")]
    fn renders_expected_plain_text(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(input.as_bytes().to_plain_text(80), expected);
        assert_no_terminal_controls(&input.as_bytes().to_plain_text(80));
    }

    #[test]
    fn empty_input_is_empty_regardless_of_cols() {
        assert_eq!(b"".to_plain_text(0), "");
        assert_eq!(b"".to_plain_text(1), "");
        assert_eq!(b"".to_plain_text(u16::MAX), "");
    }

    #[test]
    fn zero_cols_is_treated_as_one() {
        // Must not panic (no divide-by-zero, no zero-width grid).
        let _ = b"anything at all".to_plain_text(0);
    }

    #[test]
    fn works_on_vec_and_array_receivers() {
        // Trait is available on owned and array receivers, not just &[u8].
        let owned: Vec<u8> = b"echo hi".to_vec();
        assert_eq!(owned.to_plain_text(80), "echo hi");
        let arr: [u8; 7] = *b"echo hi";
        assert_eq!(arr.to_plain_text(80), "echo hi");
    }

    #[test]
    fn trailing_blank_lines_are_trimmed() {
        assert_eq!(b"hi\r\n\r\n\r\n".to_plain_text(80), "hi");
    }

    proptest! {
        /// For ANY bytes and ANY width, rendering must not panic and must not
        /// leave terminal control characters behind.
        #[test]
        fn never_panics_and_strips_controls(bytes in proptest::collection::vec(any::<u8>(), 0..4096), cols in any::<u16>()) {
            let out = bytes.to_plain_text(cols);
            prop_assert!(!out.chars().any(|c| c.is_control() && c != '\n' && c != '\t'));
        }

        /// Output never exceeds the row cap, no matter how many newlines the
        /// input contains.
        #[test]
        fn respects_row_cap(newlines in 0usize..50_000) {
            let bytes = vec![b'\n'; newlines];
            let out = bytes.to_plain_text(80);
            prop_assert!(out.lines().count() <= super::MAX_ROWS);
        }

        /// Rendering already-clean single-line printable ASCII (shorter than the
        /// width, so no wrapping) is idempotent.
        #[test]
        fn idempotent_on_clean_single_line(s in "[ -~]{0,80}") {
            let once = s.as_bytes().to_plain_text(200);
            let twice = once.as_bytes().to_plain_text(200);
            prop_assert_eq!(once, twice);
        }
    }
}
```

Note: `impl<T: AsRef<[u8]> + ?Sized>` (with `?Sized`) so `[u8]` and `str`-like unsized receivers work; matches the intent of a maximally-usable blanket impl. The unused `Cow` import will be removed in Task 3 if not needed — leave it out if clippy complains now (it is only referenced here as a doc anchor placeholder; delete the `use std::borrow::Cow;` line if it warns).

Actually: remove the `use std::borrow::Cow;` line — it is not used. It was copied from the sibling module. Omit it.

**Step 3: Run the tests to verify they FAIL for the right reason**

Run: `cargo test -p atuin-common --features vt100 vt100::`
Expected: compiles, tests FAIL at runtime with `not yet implemented: implemented in Task 3` (the `todo!()`).

**Step 4: Commit the failing tests**

```bash
git add crates/atuin-common/src/vt100.rs crates/atuin-common/src/lib.rs
git commit -m "test(atuin-common): add failing Vt100PlainTextExt test suite"
```

---

### Task 3: Implement `to_plain_text` to make the tests pass

**Files:**
- Modify: `crates/atuin-common/src/vt100.rs`

**Step 1: Replace the stub method and add the private helpers**

Replace the `to_plain_text` body and add two private free functions below the `impl` block (above `#[cfg(test)]`):

```rust
    fn to_plain_text(&self, cols: u16) -> String {
        let bytes = self.as_ref();
        if bytes.is_empty() {
            return String::new();
        }

        let cols = cols.max(1);
        let normalized = normalize_newlines(bytes);
        let rows = estimated_rows(&normalized, cols);

        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.process(&normalized);

        normalize_screen_contents(&parser.screen().contents())
    }
```

```rust
/// Insert a carriage return before any line feed that is not already preceded by
/// one, mimicking a terminal driver's `ONLCR` flag.
///
/// Pipe-captured output uses bare `\n`; in a real terminal a line feed only moves
/// the cursor down without returning to column 0, so without this every line
/// would start further right than the last and eventually wrap into garbage.
/// Input that already uses `\r\n` is returned unchanged (the insert never fires),
/// which is why this is safe to apply unconditionally to PTY-sourced input too.
fn normalize_newlines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 8);
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' && (i == 0 || bytes[i - 1] != b'\r') {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

/// Estimate how many rows the emulated screen needs to hold `bytes` without
/// losing content off the top, capped at [`MAX_ROWS`].
///
/// Real terminal output tends to have short lines, so a `bytes / cols` estimate
/// alone badly under-counts; we add the newline count. The extra `+1` leaves a
/// row of headroom for a final partial line.
fn estimated_rows(bytes: &[u8], cols: u16) -> u16 {
    let newline_rows = bytes.iter().filter(|&&b| b == b'\n').count() + 1;
    let wrapped_rows = bytes.len() / cols as usize;
    newline_rows
        .saturating_add(wrapped_rows)
        .saturating_add(1)
        .clamp(1, MAX_ROWS) as u16
}

/// Trim trailing whitespace from each line and drop trailing blank lines,
/// then rejoin with `\n`.
fn normalize_screen_contents(contents: &str) -> String {
    let mut lines = contents.lines().map(str::trim_end).collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}
```

**Step 2: Run the tests to verify they PASS**

Run: `cargo test -p atuin-common --features vt100 vt100::`
Expected: PASS (all rstest cases + proptest invariants).

**Step 3: Lint and format**

Run: `cargo clippy -p atuin-common --features vt100 --all-targets -- -D warnings`
Expected: no warnings. (If the `#[allow(unused_variables)]`-free stub `cols` triggered a warning earlier, it is gone now.)

Run: `cargo fmt -p atuin-common`
Expected: no diff (or apply the formatting).

**Step 4: Commit**

```bash
git add crates/atuin-common/src/vt100.rs
git commit -m "feat(atuin-common): implement Vt100PlainTextExt::to_plain_text"
```

---

### Task 4: Switch `atuin-pty-proxy` to the shared trait

**Files:**
- Modify: `crates/atuin-pty-proxy/Cargo.toml`
- Modify: `crates/atuin-pty-proxy/src/capture.rs`

**Step 1: Add the dependency**

In `crates/atuin-pty-proxy/Cargo.toml`, under `[target.'cfg(unix)'.dependencies]` (NOT the top `[dependencies]` — `capture.rs` and its `vt100` use are unix-only), add:

```toml
atuin-common = { workspace = true, features = ["vt100"] }
```

**Step 2: Import the trait and replace the call sites**

In `crates/atuin-pty-proxy/src/capture.rs`, add near the top imports:

```rust
use atuin_common::vt100::Vt100PlainTextExt;
```

Replace the three call sites (`capture.rs:141-145`). Change:

```rust
        let prompt = render_plain_text(&buffers.prompt, cols);
        let command = render_plain_text(&buffers.command, cols)
            .trim_matches(|c| c == '\r' || c == '\n')
            .to_string();
        let output = render_plain_text(&buffers.output, cols);
```

to:

```rust
        let prompt = buffers.prompt.to_plain_text(cols);
        let command = buffers
            .command
            .to_plain_text(cols)
            .trim_matches(|c| c == '\r' || c == '\n')
            .to_string();
        let output = buffers.output.to_plain_text(cols);
```

**Step 3: Delete the now-dead local implementation**

Remove from `capture.rs`:
- `const CLEAN_TEXT_MAX_ROWS: usize = 10_000;` (line 169)
- `fn render_plain_text(...)` (lines 171-180)
- `fn normalize_screen_contents(...)` (lines 182-188)
- `fn estimated_rows(...)` (lines 190-197)

**Step 4: Migrate the two unit tests that called `render_plain_text` directly**

The `#[cfg(test)] mod tests` in `capture.rs` calls `render_plain_text(...)` at lines 218-244. These behaviors are now covered by `atuin-common`'s test suite, so DELETE these two now-unreachable tests to avoid a compile error:
- `command_text_collapses_terminal_echo_edits` (lines 216-227)
- `text_cleaning_strips_ansi_and_terminal_controls` (lines 229-238)
- `text_cleaning_preserves_valid_utf8_after_backspace` (lines 240-246)

Keep every test that exercises the tracker end-to-end (`command_text_replays_backspaces`, `captures_complete_command`, and everything below) — those still validate the integration and continue to use `render_plain_text`'s behavior *through* `to_plain_text`.

If `assert_no_terminal_controls` becomes unused after the deletions, remove it too (it is still used by `command_text_replays_backspaces`, so it should remain — verify).

**Step 5: Build and test**

Run: `cargo test -p atuin-pty-proxy`
Expected: PASS — the tracker tests confirm behavior is preserved through the shared trait.

Run: `cargo clippy -p atuin-pty-proxy --all-targets -- -D warnings`
Expected: no warnings (no dead code, no unused imports).

**Step 6: Commit**

```bash
git add crates/atuin-pty-proxy/Cargo.toml crates/atuin-pty-proxy/src/capture.rs
git commit -m "refactor(atuin-pty-proxy): use atuin-common Vt100PlainTextExt"
```

---

### Task 5: Switch `atuin-ai` to the shared trait

**Files:**
- Modify: `crates/atuin-ai/Cargo.toml`
- Modify: `crates/atuin-ai/src/tools/mod.rs`

**Step 1: Add the `vt100` feature to the existing `atuin-common` dependency**

In `crates/atuin-ai/Cargo.toml`, change:

```toml
atuin-common = { workspace = true, features = ["unicode"] }
```

to:

```toml
atuin-common = { workspace = true, features = ["unicode", "vt100"] }
```

**Step 2: Import the trait and replace the call sites**

In `crates/atuin-ai/src/tools/mod.rs`, add to the imports:

```rust
use atuin_common::vt100::Vt100PlainTextExt;
```

Replace `mod.rs:1051-1052`:

```rust
    let stdout_text = strip_ansi_via_vt100(&full_stdout);
    let stderr_text = strip_ansi_via_vt100(&full_stderr);
```

with:

```rust
    let stdout_text = full_stdout.to_plain_text(PREVIEW_WIDTH);
    let stderr_text = full_stderr.to_plain_text(PREVIEW_WIDTH);
```

**Step 3: Delete the now-dead `strip_ansi_via_vt100`**

Remove `fn strip_ansi_via_vt100(...)` (lines 896-920, including its doc comment starting at line 896). Leave `normalize_newlines_for_vt100` (lines 850-869) and `vt100_screen_lines` (lines 871-894) UNTOUCHED — they are still used by the streaming preview at lines 976, 997, 1011, 1020, 1046.

**Step 4: Build and test**

Run: `cargo test -p atuin-ai`
Expected: PASS.

Run: `cargo clippy -p atuin-ai --all-targets -- -D warnings`
Expected: no warnings. In particular, confirm `normalize_newlines_for_vt100` and `vt100_screen_lines` are NOT reported as dead code (they are still called by the streaming path).

**Step 5: Commit**

```bash
git add crates/atuin-ai/Cargo.toml crates/atuin-ai/src/tools/mod.rs
git commit -m "refactor(atuin-ai): use atuin-common Vt100PlainTextExt"
```

---

### Task 6: Full workspace verification

**Step 1: Build everything**

Run: `cargo build --workspace --all-features`
Expected: PASS.

**Step 2: Test everything touched**

Run: `cargo test -p atuin-common --features vt100 && cargo test -p atuin-pty-proxy && cargo test -p atuin-ai`
Expected: all PASS.

**Step 3: Confirm the feature gate actually gates**

Run: `cargo check -p atuin-common`
Expected: PASS, and `vt100` is not compiled (verify with `cargo tree -p atuin-common -i vt100` → "package not found in this dependency graph" or absent without `--features vt100`).

**Step 4: Workspace lint + format**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: no warnings.

Run: `cargo fmt --all --check`
Expected: no diff.

**Step 5: Final review checkpoint**

Confirm:
- `atuin-common::vt100::Vt100PlainTextExt` is the single source of truth.
- pty-proxy and atuin-ai contain no local ANSI-strip/plain-text renderer.
- Streaming-preview helpers in atuin-ai remain and are used.
- No behavior regression: all pre-existing integration tests pass.

---

## PR

After Task 6 passes, push the branch and open a **draft** PR titled with the prefix `AI WIP | ` (e.g. `AI WIP | Extract Vt100PlainTextExt into atuin-common`). Do **not** write a PR body.
