# Plan: Client-Side Shell Command Execution with Preview Window

## Overview

Implement client-side execution for `execute_shell_command` with real-time streaming output displayed in a fixed-height preview panel. The preview auto-scrolls to show the latest output (tail behavior). Commands are interruptible via Ctrl+C. Shell output is processed through a VT100 emulator to properly handle ANSI escape sequences, progress bars (`\r`), cursor movement, and colors. This establishes a pattern that will extend to `read_file` and `write_file` tools.

---

## Part 1: Shell Command Streaming Execution with VT100 Emulation

**File: `crates/atuin-ai/src/tools/mod.rs`**

Add a standalone streaming execution function:

```rust
pub(crate) async fn execute_shell_command_streaming(
    shell_call: &ShellToolCall,
    output_tx: mpsc::Sender<Vec<String>>,  // sends styled lines
    mut interrupt_rx: tokio::sync::oneshot::Receiver<()>,
) -> ToolOutcome
```

**VT100 emulator approach:**

- Create `vt100::Parser::new(height, width, scrollback_len)` where:
  - `height` = preview viewport height (10) — the VT100 screen matches the preview
  - `width` = terminal width (query via `crossterm::terminal::size()` or default 80)
  - `scrollback_len` = 0 (we only need the visible screen, not history)
- `vt100::Parser` is `Send + Sync`, safe to use directly in tokio tasks
- Spawn `tokio::process::Command` with `Stdio::piped()` for stdout+stderr
- Read bytes from both stdout and stderr concurrently via `tokio::io::BufReader`
- Feed raw bytes to the parser: `parser.process(&bytes)`
- The VT100 emulator handles:
  - ANSI color/style escape sequences
  - `\r` (carriage return) for progress bars — cursor moves to line start, overwrites
  - Cursor movement, screen clearing
  - Line wrapping
- Every **50ms**, snapshot the screen and send to UI:
  ```rust
  // Extract plain text lines from the VT100 screen
  let screen = parser.screen();
  let (rows, cols) = screen.size();
  let mut lines = Vec::new();
  for row in 0..rows {
      let mut line = String::new();
      for col in 0..cols {
          if let Some(cell) = screen.cell(row, col) {
              line.push_str(cell.contents());
          }
      }
      lines.push(line);
  }
  let _ = output_tx.send(lines);
  ```
- On process completion or interruption, send final screen state
- Return `ToolOutcome::Success(...)` with exit code, or `ToolOutcome::Error(...)` on failure

**Why VT100 over raw ANSI stripping:**
- Progress bars that use `\r` to rewrite lines display correctly (last state shown)
- Cursor-positioned output (curses-like) renders properly
- Colors are handled correctly (we strip them for plain text now, but the emulator consumes the escape sequences so they don't appear as garbled text)
- Future: we can extract styled cells (`cell.fgcolor()`, `cell.bold()`) to render colored output

**Dependencies** (`crates/atuin-ai/Cargo.toml`):
```toml
vt100 = "0.16"
```

---

## Part 2: State Model Changes

**File: `crates/atuin-ai/src/tools/mod.rs`** — `ToolCallState`

Add new variant for executing with live preview:

```rust
pub(crate) enum ToolCallState {
    CheckingPermissions,
    AskingForPermission,
    Denied(String),
    Executing,
    ExecutingPreview {            // NEW
        command: String,
        output_lines: Vec<String>,  // current VT100 screen lines (plain text)
        exit_code: Option<i32>,
        interrupted: bool,
    },
}
```

Add helper to `PendingToolCall`:

```rust
pub fn mark_executing_preview(&mut self, command: String) {
    self.state = ToolCallState::ExecutingPreview {
        command,
        output_lines: Vec::new(),
        exit_code: None,
        interrupted: false,
    };
}
```

**File: `crates/atuin-ai/src/tui/state.rs`** — `Session`

- Add `shell_abort_tx: Option<tokio::sync::oneshot::Sender<()>>` for interrupting running commands

---

## Part 3: Event System Changes

**File: `crates/atuin-ai/src/tui/events.rs`** — `AiTuiEvent`

Add new variant:

```rust
InterruptToolExecution,  // Ctrl+C while a shell command is running
```

**File: `crates/atuin-ai/src/tui/state.rs`** — `AppMode`

Add new variant:

```rust
pub(crate) enum AppMode {
    Input,
    Generating,
    Streaming,
    Error,
    ExecutingPreview,  // NEW — shell command running with preview
}
```

Update `footer_text()`:
```rust
AppMode::ExecutingPreview => "[Ctrl+C] Interrupt  [Esc] Interrupt",
```

---

## Part 4: Dispatch Changes

**File: `crates/atuin-ai/src/tui/dispatch.rs`**

### Modify `on_select_permission` — Allow arm

When the user allows a `Shell` tool call:

1. Mark the tool as `ExecutingPreview` in state immediately (for instant UI feedback)
2. Set `AppMode::ExecutingPreview`
3. Create a `tokio::sync::oneshot::channel` for interruption, store sender in `session.shell_abort_tx`
4. Clone the tool data and spawn an async task that:
   - Calls `execute_shell_command_streaming()` with the output channel and interrupt receiver
   - Receives `Vec<String>` (screen lines) via an `mpsc::Receiver`, calls `handle.update()` to replace `output_lines` in the `ExecutingPreview` state
   - On completion, sets `exit_code` and `interrupted` flags in state
   - Calls `complete_tool_call()` to record the `ToolOutcome`
   - Sends `AiTuiEvent::ContinueAfterTools` if no more unresolved tools
   - Resets `AppMode::Input`

For non-shell tools, keep the current behavior (synchronous `execute()` in background task).

### New handler: `on_interrupt_tool_execution`

- Take `shell_abort_tx` from state and send the interrupt signal
- The spawned execution task receives it and aborts the child process
- Command result includes partial output with interruption note
- Transition `AppMode` back to `Input`

### New dispatch entry

```rust
AiTuiEvent::InterruptToolExecution => {
    on_interrupt_tool_execution(handle);
}
```

---

## Part 5: Key Handling Changes

**File: `crates/atuin-ai/src/tui/components/atuin_ai.rs`**

Modify Ctrl+C handling to check mode:

```rust
if modifiers.contains(KeyModifiers::CONTROL) && *code == KeyCode::Char('c') {
    match props.mode {
        AppMode::ExecutingPreview => {
            let _ = tx.send(AiTuiEvent::InterruptToolExecution);
        }
        _ => {
            let _ = tx.send(AiTuiEvent::Exit);
        }
    }
    return EventResult::Consumed;
}
```

Add `ExecutingPreview` to the mode-specific key handling:

```rust
AppMode::ExecutingPreview => match code {
    KeyCode::Esc => {
        let _ = tx.send(AiTuiEvent::InterruptToolExecution);
        EventResult::Consumed
    }
    _ => EventResult::Ignored,  // swallow all other keys during execution
},
```

---

## Part 6: eye_declare — Viewport Component

**File: `eye_declare/crates/eye_declare/src/viewport.rs`** (new file)

Add a `Viewport` component that renders a fixed-height window into a larger text buffer:

```rust
#[props]
pub struct ViewportProps {
    pub lines: Vec<String>,        // all lines to display
    pub height: u16,               // visible height in rows
    pub border: Option<BorderType>,
    pub title: Option<String>,
    pub style: Style,              // text style (used as base/default)
    pub border_style: Style,       // border style
}
```

Behavior:
- Renders only the last `height` lines from `lines` (tail behavior)
- If fewer lines than height, renders from top
- Supports bordered rendering with optional title
- Auto-scrolls to bottom as new lines are added (tail behavior)
- Long lines are truncated at viewport width

Ratatui implementation:
- Clips rendering to viewport height using a sub-area
- Draws border if configured
- Renders `Text` widgets inside the clipped region
- Exports from `eye_declare::lib.rs`

**Future enhancement**: Accept `Vec<ratatui::text::Line<'static>>` instead of `Vec<String>` for styled text (when we extract VT100 cell colors). The `vt100` crate provides `cell.fgcolor()`, `cell.bold()`, etc. which can be mapped to ratatui `Style`. This can be done in a follow-up without changing the component's external API (add a new variant or make lines generic).

---

## Part 7: View Changes

**File: `crates/atuin-ai/src/tui/view/mod.rs`**

### New function: `executing_preview_view()`

Renders the live command preview:

```rust
fn executing_preview_view(tool_call: &PendingToolCall) -> Elements {
    // Extract command, output_lines, exit_code, interrupted from ToolCallState::ExecutingPreview
    element! {
        View(key: format!("preview-{}", tool_call.id), padding_top: Cells::from(1)) {
            // Command header
            Text {
                Span(text: "Running: ", style: Style::default().fg(Color::Yellow))
                Span(text: &command, style: Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            }
            // Fixed-height viewport showing last 10 lines of VT100 screen
            Viewport(
                lines: output_lines.clone(),
                height: 10,
                border: BorderType::Plain,
                style: Style::default().fg(Color::White),
            )
            // Status line
            #(if let Some(code) = exit_code {
                Text {
                    Span(text: format!("Exit code: {code}"), style: ...)
                }
            })
            #(if interrupted {
                Text {
                    Span(text: "Interrupted", style: Style::default().fg(Color::Red))
                }
            })
            Text {
                Span(text: "[Ctrl+C] Interrupt", style: Style::default().fg(Color::DarkGray))
            }
        }
    }
}
```

### Modify `input_view()`

Check for both asking-for-permission and executing-preview states:

```rust
fn input_view(state: &Session) -> Elements {
    let asking_tool = state.pending_tool_calls.iter()
        .find(|call| call.state == ToolCallState::AskingForPermission);
    let executing_tool = state.pending_tool_calls.iter()
        .find(|call| matches!(call.state, ToolCallState::ExecutingPreview { .. }));

    element! {
        #(if let Some(tc) = asking_tool { #(tool_call_view(tc)) })
        #(if let Some(tc) = executing_tool { #(executing_preview_view(tc)) })
        #(if asking_tool.is_none() && executing_tool.is_none() {
            // existing input box rendering
            View(key: "input-box", ...) { InputBox(...) }
        })
    }
}
```

---

## File Change Summary

| File | Change |
|------|--------|
| `eye_declare/.../viewport.rs` (new) | `Viewport` component — fixed-height window with tail behavior |
| `eye_declare/.../lib.rs` | Export `Viewport` |
| `crates/atuin-ai/Cargo.toml` | Add `vt100 = "0.16"` dependency |
| `crates/atuin-ai/src/tools/mod.rs` | `execute_shell_command_streaming()` with VT100, `ExecutingPreview` state, `mark_executing_preview()` |
| `crates/atuin-ai/src/tui/state.rs` | `AppMode::ExecutingPreview`, `shell_abort_tx`, footer text |
| `crates/atuin-ai/src/tui/events.rs` | `InterruptToolExecution` variant |
| `crates/atuin-ai/src/tui/dispatch.rs` | Streaming execution in `on_select_permission`, `on_interrupt_tool_execution` handler |
| `crates/atuin-ai/src/tui/components/atuin_ai.rs` | Ctrl+C → interrupt during `ExecutingPreview`, Esc → interrupt, new mode handling |
| `crates/atuin-ai/src/tui/view/mod.rs` | `executing_preview_view()`, modify `input_view()` |

---

## Execution Order

1. **eye_declare: Viewport component** — standalone, testable in isolation
2. **Cargo.toml: Add `vt100` dependency**
3. **tools/mod.rs: Streaming execution + state** — `execute_shell_command_streaming()` with VT100 parser, `ExecutingPreview` variant
4. **events.rs + state.rs: New variants** — `InterruptToolExecution`, `AppMode::ExecutingPreview`
5. **dispatch.rs: Streaming execution path** — modify `on_select_permission`, add interrupt handler
6. **atuin_ai.rs: Key handling** — Ctrl+C and Esc routing for new mode
7. **view/mod.rs: Preview rendering** — `executing_preview_view()`, modify `input_view()`
8. **Build + test** — `cargo build`, `cargo clippy`, manual testing

---

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Streaming vs batch | Stream in real-time | Better UX, user sees output as it happens |
| Output display | Fixed height (10 lines), tail behavior | Keeps UI compact, doesn't push input off screen |
| Interruptible | Yes, Ctrl+C | Essential for long-running commands |
| Execution path | Separate streaming function | Preserves existing `execute()` for non-preview use |
| Preview location | Input area (where permission UI lives) | Consistent with existing tool call UI pattern |
| ANSI/escape handling | VT100 emulator (`vt100` crate) | Properly handles colors, `\r` progress bars, cursor movement |
| VT100 screen height | Match viewport height (10 rows) | No scrollback needed; VT100 screen IS the viewport |
| Update batching | Time-based (50ms) | Smooth updates without flicker |
| Preview height | Hardcoded 10 lines for now | Add settings later when we have a feel for defaults |
| Output line cap | VT100 screen is bounded by its row count | Natural cap — no unbounded memory growth |
| eye_declare change | New Viewport component | Reusable for read/write tool previews later |

---

## Future Work (Not in This PR)

1. **Styled output**: Extract `vt100::Cell` colors/styles and render as colored `ratatui::text::Line` in the Viewport. The `vt100` crate already parses colors — we just need to map `vt100::Color::Idx(n)` / `Rgb(r,g,b)` to `ratatui::style::Color`.

2. **Configurable preview height**: Add `preview_height` to AI settings, pass through to VT100 parser and Viewport.

3. **User scroll control**: Allow scrolling up in the preview to see earlier output (add scroll offset prop to Viewport).

4. **Read/Write tool previews**: Apply the same Viewport pattern. Read tool shows file content with syntax highlighting (using `syntect`). Write tool shows a diff preview.

5. **Full-width terminal**: Query actual terminal width and use it for the VT100 parser, so output wraps correctly at the right column.
