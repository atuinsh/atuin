# Codex-Style Terminal Migration Prompt

Use this prompt in a fresh Codex context:

---

You are implementing a terminal architecture change in `atuin-ai` to match the Codex TUI behavior:

- no alternate screen for normal inline mode
- preserve real terminal scrollback
- keep a live interactive viewport at the bottom for current input/review state
- push finalized history into terminal scrollback (not just into ratatui buffer)

## Current Codebase (read first)

This crate currently uses `ratatui::Viewport::Inline` in:

- `atuin-ai/src/tui/terminal.rs` (`TerminalGuard::new`)

Main loop and rendering live in:

- `atuin-ai/src/commands/inline.rs` (`run_inline_tui`)
- `atuin-ai/src/tui/render.rs`
- `atuin-ai/src/tui/state.rs`
- `atuin-ai/src/tui/view_model.rs`
- `atuin-ai/src/tui/app.rs`

Current behavior keeps all conversation blocks inside ratatui rendering and relies on inline viewport scrolling.

## Target Behavior

Implement Codex-style terminal flow:

1. Render only the active/live area in a bottom viewport.
2. When content is finalized (history), emit it directly to terminal scrollback using ANSI scroll-region operations.
3. Keep cursor stable and preserve raw-mode usability.
4. Avoid `Viewport::Inline`; use a custom terminal wrapper that tracks a `viewport_area` and performs diff-based rendering.

## Required Implementation Plan

### 1) Add custom terminal primitives

Create:

- `atuin-ai/src/tui/custom_terminal.rs`
- `atuin-ai/src/tui/insert_history.rs`

Use the Codex approach as reference:

- `codex-rs/tui/src/custom_terminal.rs`
- `codex-rs/tui/src/insert_history.rs`

If those files are not directly available in this repo, implement equivalent behavior:

- terminal struct with:
  - backend
  - front/back buffers
  - `viewport_area`
  - `last_known_screen_size`
  - `last_known_cursor_pos`
- `draw` + diff flush logic
- ability to set viewport area explicitly
- history insertion via scroll-region + reverse-index and cursor restoration

Preserve MIT attribution header if code is copied/adapted from ratatui-derived implementation.

### 2) Replace `Viewport::Inline` usage

In `atuin-ai/src/tui/terminal.rs`:

- remove dependency on `TerminalOptions { viewport: Viewport::Inline(...) }`
- switch guard to hold custom terminal type
- keep existing panic-hook and raw-mode restoration semantics
- keep `keep_output` behavior

### 3) Add pending history queue + synchronized draw path

In TUI runtime layer (prefer `terminal.rs` or a new `inline_runtime.rs` module):

- add API to queue history lines:
  - `insert_history_lines(Vec<Line<'static>>)`
- in draw cycle:
  - adjust viewport size to requested live height
  - flush pending history lines to scrollback before rendering frame
  - render live frame in viewport

### 4) Integrate with app state transitions

In `inline.rs`/`app.rs`/`state.rs`, wire when to commit history:

- finalized user+assistant/tool output should be converted to display `Line<'static>` and inserted into history queue
- after commit, live viewport should no longer re-render the same finalized blocks (avoid duplication)
- preserve existing functional modes:
  - Input
  - Generating/Streaming
  - Review
  - Error
- preserve follow-up flow (`f`) and command actions (execute/insert/cancel)

Use pragmatic state bookkeeping (e.g. committed marker/index) rather than rewriting the whole architecture.

### 5) Keep rendering API stable where possible

Minimize churn in:

- `render.rs`
- `view_model.rs`

Refactor only what is needed to separate committed history from live viewport content.

### 6) Tests

Add focused tests for terminal mechanics:

- history insertion writes expected text into vt100-like backend output
- cursor is restored after history insert
- wrapped lines are preserved consistently
- no duplicate rendering between committed history and live viewport

Prefer adding tests near new modules (`custom_terminal.rs`, `insert_history.rs`) and existing app-level tests where appropriate.

If vt100 backend helper is needed, add one similar to Codex’s test backend pattern.

### 7) Non-goals / constraints

- do not introduce alt-screen for normal inline flow
- do not remove existing shell output protocol markers:
  - `__atuin_ai_execute__`
  - `__atuin_ai_insert__`
  - `__atuin_ai_cancel__`
- do not regress non-TTY error handling
- keep terminal usable after panic and normal exit

## Acceptance Criteria

1. Inline mode preserves terminal scrollback for generated/finalized history.
2. Live TUI remains interactive at bottom without alternate screen.
3. No visual duplication of already-committed history in live viewport.
4. `cargo test -p atuin-ai` passes.
5. Existing debug render tooling still works (or is updated with clear rationale).

## Deliverables

1. Code changes implementing the above.
2. Short summary of architecture changes.
3. List of modified files.
4. Notes on terminal compatibility caveats (if any).

---

Work directly in code. Do not stop at planning.
