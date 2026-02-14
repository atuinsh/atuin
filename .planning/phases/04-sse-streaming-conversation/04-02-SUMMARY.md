---
phase: 04-sse-streaming-conversation
plan: 02
subsystem: ai-chat-conversation
tags: [markdown, edit-mode, multi-turn, conversation-history]
dependency-graph:
  requires: [phase-04-01-sse-streaming]
  provides: [markdown-rendering, edit-mode, conversation-building]
  affects: [text-rendering, review-mode, refine-api-integration]
tech-stack:
  added: []
  patterns: [markdown-parsing, conversation-history-building, mode-transitions]
key-files:
  created: []
  modified:
    - crates/atuin-ai/src/tui/render.rs (markdown_to_spans function)
    - crates/atuin-ai/src/tui/app.rs (edit mode, conversation history)
    - crates/atuin-ai/src/commands/inline.rs (refine mode integration)
decisions:
  - id: D04-02-001
    summary: Task 3 footer update already completed in Plan 01
    rationale: AppMode::Streaming footer was added in previous plan, no additional changes needed
metrics:
  duration: 3m
  tasks-completed: 3
  commits: 2
  files-modified: 3
  completion-date: 2026-02-14
---

# Phase 04 Plan 02: Markdown Rendering & Edit Mode

Markdown rendering with edit mode for multi-turn conversation flow using SSE streaming.

## What Was Built

Added markdown rendering support and edit mode for multi-turn conversations:

1. **Markdown Rendering**: `markdown_to_spans()` function parses markdown and applies styling:
   - **bold** → BOLD modifier
   - *italics* → UNDERLINED modifier (per CONV-06 requirement)
   - `inline code` → Important theme styling with backticks
   - Handles paragraphs, soft/hard breaks, nested formatting

2. **Edit Mode Transition**: Users can press 'e' in Review mode to refine commands:
   - `start_edit_mode()` method transitions to Input mode with refine flag
   - Makes all existing blocks static before accepting new input
   - Sets `is_refine_mode` flag to trigger SSE streaming instead of regular generation

3. **Conversation History**: `build_conversation_history()` creates API payload:
   - Extracts Input blocks as user messages
   - Extracts Text blocks as assistant explanations
   - Extracts Command blocks as assistant responses (formatted as "Command: {cmd}")
   - Skips spinner and error blocks

4. **Refine Stream Integration**: Updated inline.rs event loop:
   - Detects refine mode and triggers `create_refine_stream()` instead of `generate_command()`
   - Passes conversation history as RefineMessage array to API
   - Transitions to AppMode::Streaming for SSE chunk display
   - Retry logic excludes refine mode (only applies to initial generation)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Already Complete] Task 3 footer update done in Plan 01**
- **Found during:** Task 3 verification
- **Issue:** Task 3 requested updating `get_footer_text()` for AppMode::Streaming, but this was already completed in Plan 04-01
- **Fix:** No changes needed - verified existing implementation is correct
- **Files affected:** None
- **Commit:** N/A (no code changes required)

## Tasks Completed

| # | Name | Status | Commit | Key Changes |
|---|------|--------|--------|-------------|
| 1 | Add markdown_to_spans() function | Complete | 70dd4277 | markdown parsing, **bold**, *italic*, `code` styling |
| 2 | Implement edit mode transition and conversation flow | Complete | 8a83bcd1 | start_edit_mode(), build_conversation_history(), refine stream trigger |
| 3 | Update footer and render for streaming mode | Complete | N/A | Verified - already implemented in Plan 01 |

## Verification Results

All verification steps passed:

- `cargo check -p atuin-ai` - passes
- `cargo clippy -p atuin-ai -- -D warnings` - passes (no warnings)
- `cargo build -p atuin-ai` - succeeds
- `cargo fmt --check` - passes

Code structure verified:
- `markdown_to_spans()` function exists and handles markdown events correctly
- `render_text_block()` uses markdown_to_spans() for styled output
- `start_edit_mode()` transitions to Input mode with refine flag set
- `build_conversation_history()` extracts (role, content) pairs from blocks
- KeyCode::Char('e') in Review mode calls start_edit_mode()
- inline.rs detects is_refine_mode and triggers refine stream
- Retry logic skips refine mode (only initial generation can retry)
- AppMode::Streaming footer already present from Plan 01

## Technical Notes

### Markdown Rendering Architecture

The markdown rendering uses pulldown_cmark's event-based parser:

1. **Event Processing**:
   - Start/End tags for Strong (bold), Emphasis (italic)
   - Code events for inline code
   - Text events split by newlines for proper line handling
   - Paragraph events create blank lines between blocks

2. **Style Stack**:
   - Maintains stack of active styles for nested formatting
   - Base style from theme's Meaning::Base
   - Code style from theme's Meaning::Important
   - Modifiers (BOLD, UNDERLINED) added on top of current style

3. **Line Building**:
   - Accumulates Span elements into Vec<Vec<Span>> structure
   - Converts to Vec<Line> for ratatui Paragraph widget
   - Handles wrapping via Paragraph::wrap()

### Edit Mode Flow

The edit mode enables multi-turn conversation:

1. **User presses 'e' in Review mode**:
   - `handle_review_key()` detects KeyCode::Char('e')
   - Calls `start_edit_mode()`
   - Makes all blocks static, clears input, sets is_refine_mode=true
   - Transitions to AppMode::Input

2. **User types follow-up and presses Enter**:
   - `handle_input_key()` calls `start_generating()`
   - Adds input block, adds spinner, transitions to AppMode::Generating

3. **Event loop detects refine mode**:
   - Checks `app.is_refine_mode` flag
   - Calls `build_conversation_history()` to get message pairs
   - Converts to RefineMessage array
   - Calls `app.start_streaming_response()` (creates empty streaming text block)
   - Calls `create_refine_stream()` to start SSE stream

4. **Stream chunks arrive**:
   - Tick handler polls stream without blocking
   - Calls `app.append_to_streaming_block(chunk)` for each chunk
   - Markdown rendering happens on each frame redraw

5. **Stream completes**:
   - Calls `app.finalize_streaming()` (makes streaming block static, transitions to Review)
   - User can press 'e' again for another turn, or Enter/Tab to execute/insert command

### Conversation History Format

The conversation history builder creates this structure:

```json
[
  {"role": "user", "content": "list files"},
  {"role": "assistant", "content": "Command: ls -la"},
  {"role": "assistant", "content": "This lists all files including hidden ones..."},
  {"role": "user", "content": "sort by date"},
  {"role": "assistant", "content": "...streaming response..."}
]
```

Commands are prefixed with "Command: " to distinguish them from explanations.

### Future Integration

The markdown renderer supports basic inline formatting. For Phase 5, termimad integration may extend support to:
- Code blocks (fenced with ```)
- Lists (ordered/unordered)
- Headings
- Links

Current implementation focuses on inline elements most relevant to LLM explanations.

## Self-Check: PASSED

All files modified are present:
- FOUND: /Users/binarymuse/src/atuin/crates/atuin-ai/src/tui/render.rs (markdown_to_spans added)
- FOUND: /Users/binarymuse/src/atuin/crates/atuin-ai/src/tui/app.rs (edit mode methods added)
- FOUND: /Users/binarymuse/src/atuin/crates/atuin-ai/src/commands/inline.rs (refine integration added)

All commits exist in history:
- FOUND: 70dd4277 (feat(04-02): add markdown_to_spans() function)
- FOUND: 8a83bcd1 (feat(04-02): implement edit mode transition and conversation flow)
