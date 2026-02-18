---
phase: 05-safety-polish
plan: 03
subsystem: ui
tags: [ratatui, tui, tool-status, markdown, pulldown-cmark, view-model]

# Dependency graph
requires:
  - phase: 05-01
    provides: "Content enum, WarningKind, Blocks::from_state pure derivation"
provides:
  - "Content::ToolStatus variant with spinner/checkmark display"
  - "count_tool_calls_since_last_user helper excluding suggest_command"
  - "Tool status derivation in Blocks::from_state during Streaming mode"
  - "Fenced code block rendering with Important styling in markdown_to_spans"
affects: [05-04]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "ToolStatus uses frame field for spinner animation, same as Spinner content type"
    - "count_tool_calls_since_last_user searches from last UserMessage to distinguish per-turn tools"
    - "pulldown-cmark strips fence delimiters before Text events - content styled with Important meaning"

key-files:
  created: []
  modified:
    - crates/atuin-ai/src/tui/view_model.rs
    - crates/atuin-ai/src/tui/render.rs
    - crates/atuin-ai/src/commands/debug_render.rs

key-decisions:
  - "ToolStatus shows ONE status line at a time - in-flight shows spinner, completed collapses to summary"
  - "suggest_command tool calls excluded from tool count (they render directly as Command blocks)"
  - "Code block content uses Important styling since pulldown-cmark strips fence characters before handler"

patterns-established:
  - "ToolStatus pattern: current_label=Some means in-flight (spinner), None means completed (checkmark)"
  - "Tool counting: scan from last UserMessage, track in_flight/completed transitions on ToolCall/ToolResult"

requirements-completed: []

# Metrics
duration: 4min
completed: 2026-02-18
---

# Phase 05 Plan 03: Tool Status Display and Code Block Rendering Summary

**Content::ToolStatus variant with spinner/checkmark tool tracking, suggest_command exclusion, and fenced code block Important-styled rendering via pulldown-cmark**

## Performance

- **Duration:** 4 min
- **Started:** 2026-02-18T20:08:22Z
- **Completed:** 2026-02-18T20:12:00Z
- **Tasks:** 3
- **Files modified:** 3

## Accomplishments
- Added Content::ToolStatus variant with completed_count, current_label (in-flight tool name), and frame (spinner)
- Added count_tool_calls_since_last_user helper that scans from last UserMessage, counting ToolCall/ToolResult transitions and excluding suggest_command
- Integrated tool status derivation in Blocks::from_state during Streaming mode: in-flight shows spinner, completed text streaming collapses to checkmark summary
- Added fenced code block rendering in markdown_to_spans: in_code_block state tracks CodeBlock start/end events, text inside rendered with Important styling
- Added ToolStatus render arm (Annotation style, spinner or checkmark prefix) and height calculation (always 1 line)

## Task Commits

Each task was committed atomically:

1. **Task 1: Add Content::ToolStatus variant and counting helper** - `7709e54b` (feat)
2. **Task 2: Render ToolStatus content and add height calculation** - `7709e54b` (feat - combined with Task 1)
3. **Task 3: Add fenced code block rendering to markdown_to_spans** - `c735b3fa` (feat - applied during plan 05-02 execution)

**Plan metadata:** (see final commit below)

## Files Created/Modified
- `crates/atuin-ai/src/tui/view_model.rs` - Added Content::ToolStatus variant, prefix_symbol arm, count_tool_calls_since_last_user helper, tool status derivation in from_state
- `crates/atuin-ai/src/tui/render.rs` - Added ToolStatus render arm, height=1 calculation, fenced code block handling in markdown_to_spans
- `crates/atuin-ai/src/commands/debug_render.rs` - Added ToolStatus arm to content_to_json serialization

## Decisions Made
- ToolStatus shows only ONE status line at a time - in-flight tools show spinner with current tool name, completed tools (when streaming text arrives) collapse to checkmark + "Used X tools" summary
- suggest_command tool calls are excluded from the count because they render as Command blocks directly
- Code block content uses Important styling because pulldown-cmark strips fence delimiter characters (```) before they reach the Text event handler

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added ToolStatus arm to debug_render.rs content_to_json**
- **Found during:** Task 1 (Content::ToolStatus variant addition)
- **Issue:** Non-exhaustive match in debug_render.rs content_to_json caused compilation failure
- **Fix:** Added Content::ToolStatus arm serializing completed_count, current_label, and frame fields
- **Files modified:** crates/atuin-ai/src/commands/debug_render.rs
- **Verification:** cargo check -p atuin-ai passes
- **Committed in:** 7709e54b (Tasks 1+2 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Required for compilation. No scope creep.

## Issues Encountered
- Tasks 1 and 2 committed together because the non-exhaustive match blocker (debug_render.rs) required fixing render.rs simultaneously to get a clean compile
- Task 3 fenced code block changes were already applied by plan 05-02 execution (which ran interleaved between plan 05-03's Task 1+2 and Task 3 phases)
- Clippy fixes for pre-existing collapsible_if warnings (from plan 05-01 code) were incorporated into plan 05-02's fix commit (470bbfc5)

## Next Phase Readiness
- Tool status display complete: one status line with spinner while in-flight, collapses to checkmark summary when text arrives
- Fenced code blocks render with visual distinction from regular text using Important styling
- Ready for plan 05-04

---
*Phase: 05-safety-polish*
*Completed: 2026-02-18*
