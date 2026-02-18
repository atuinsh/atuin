# Roadmap: Atuin AI TUI

## Overview

This roadmap builds a conversational command generation TUI from foundation to polish in 5 phases. We start with core event loop infrastructure and panic handling (Phase 1), establish block-based rendering with static data (Phase 2), add SSE streaming complexity (Phase 3), extend to multi-turn conversation with safety features (Phase 4), and finish with markdown formatting and keyboard polish (Phase 5). Each phase delivers verifiable user-facing capabilities while maintaining clean architecture patterns.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [x] **Phase 1: Core Event Loop Infrastructure** - Async event loop with keyboard input, panic handling, basic state management
- [x] **Phase 2: Block Rendering System** - Block-based conversation UI with theme integration and word wrapping
- [x] **Phase 3: Command Generation** - Fast initial command suggestion via /api/cli/generate
- [x] **Phase 4: SSE Streaming & Conversation** - Multi-turn conversation with streaming responses
- [x] **Phase 04.1: Pure State Architecture Refactor** - Refactor to pure state -> derived view pattern (INSERTED)
- [x] **Phase 04.1.1: API Migration** - Migrate to unified /api/cli/chat endpoint (INSERTED)
- [ ] **Phase 5: Safety & Polish** - Dangerous command warnings, markdown formatting, keyboard refinements

## Phase Details

### Phase 1: Core Event Loop Infrastructure
**Goal**: Foundation for async TUI with keyboard input and terminal safety
**Depends on**: Nothing (first phase)
**Requirements**: TUI-01, TUI-03, TUI-07, TUI-08, TUI-10, TUI-11, ACT-05
**Success Criteria** (what must be TRUE):
  1. TUI renders at cursor position as inline popup
  2. Terminal restores correctly after panic or normal exit
  3. Keyboard events (Enter, Esc) are detected and handled correctly
  4. Block state machine transitions from Building to Active to Static
  5. Async event loop multiplexes keyboard input and render ticks without blocking
**Plans**: 3 plans in 3 waves

Plans:
- [x] 01-01-PLAN.md — Terminal lifecycle (panic hook, Drop guard) and async event loop infrastructure
- [x] 01-02-PLAN.md — Block state machine and keyboard handling
- [x] 01-03-PLAN.md — Wire infrastructure into run_inline_tui() (gap closure)

### Phase 2: Block Rendering System
**Goal**: Visual block-based conversation UI that works with static content
**Depends on**: Phase 1
**Requirements**: TUI-02, TUI-04, TUI-05, TUI-06, TUI-09, GEN-01, GEN-02
**Success Criteria** (what must be TRUE):
  1. Input blocks render with ">" symbol and user text
  2. Command blocks render with "$" symbol and command text
  3. Text blocks render with proper word wrapping for long content
  4. Thin bordered frame displays title (top-left) and keybinds (bottom-right)
  5. Theme colors from atuin-client/theme.rs apply correctly to all blocks
**Plans**: 1 plan

Plans:
- [x] 02-01-PLAN.md — Render module with theme integration and block-based rendering

### Phase 3: Command Generation
**Goal**: Users can generate shell commands from natural language via API
**Depends on**: Phase 2
**Requirements**: GEN-03, GEN-04, GEN-05, ACT-01, ACT-02, ACT-03, ACT-06, ACT-07
**Success Criteria** (what must be TRUE):
  1. User enters natural language request in input block
  2. Spinner displays during API call to /api/cli/generate
  3. Generated command appears in command block with explanation
  4. User can press Enter to run command, Tab to insert without running, or Esc to cancel
  5. TUI exits correctly with command in original prompt (erase mode) or underneath (keep mode)
**Plans**: 1 plan

Plans:
- [x] 03-01-PLAN.md — Error block rendering and exit mode configuration

### Phase 4: SSE Streaming & Conversation
**Goal**: Multi-turn conversation with real-time streaming responses
**Depends on**: Phase 3
**Requirements**: CONV-01, CONV-02, CONV-03, CONV-04, CONV-05, CONV-06, CONV-07, ACT-04
**Success Criteria** (what must be TRUE):
  1. User presses 'e' to add follow-up message after initial command
  2. SSE stream from /api/cli/refine displays text chunks in real-time
  3. Markdown formatting renders correctly (bold, italics as underline, inline code)
  4. Previous blocks become static as conversation progresses
  5. Conversation history is maintained across multiple turns
**Plans**: 2 plans in 2 waves

Plans:
- [x] 04-01-PLAN.md — SSE streaming infrastructure (dependencies, BlockState::Streaming, event loop integration)
- [x] 04-02-PLAN.md — Markdown rendering and edit mode conversation flow

### Phase 04.1: Pure State Architecture Refactor (INSERTED)
**Goal**: Eliminate rendering bugs by refactoring to pure state -> derived view pattern
**Depends on**: Phase 4
**Requirements**: None (internal refactor)
**Success Criteria** (what must be TRUE):
  1. View model is always a pure function of state (Blocks::from_state)
  2. Renderer only reads view model, never queries AppState directly
  3. State transitions happen through explicit methods on AppState
  4. Messages mutated in place during streaming (not replaced)
  5. Old blocks module removed, new state.rs + view_model.rs in place
**Plans**: 3 plans in 3 waves

Plans:
- [x] 04.1-01-PLAN.md — State and view model type definitions
- [x] 04.1-02-PLAN.md — State transition methods and keyboard handling
- [x] 04.1-03-PLAN.md — Render integration and blocks module removal

### Phase 04.1.1: Refactor to match new API endpoint and behavior (INSERTED)
**Goal:** Migrate TUI client from dual-endpoint architecture to unified /api/cli/chat endpoint
**Depends on:** Phase 04.1
**Requirements**: None (API migration)
**Success Criteria** (what must be TRUE):
  1. Single /api/cli/chat endpoint used for all conversation turns
  2. Session ID stored from first response, sent on subsequent requests
  3. Conversation events transform to Claude API message format
  4. Spinner text changes based on status events (Processing, Searching, Thinking, Waiting)
  5. Footer hides Enter/Tab keybinds when no command exists in conversation
  6. Cancellation preserves partial text with interrupted marker in history
**Plans:** 2 plans in 2 waves

Plans:
- [x] 04.1.1-01-PLAN.md — State types and API migration (session, status, events_to_messages, unified endpoint)
- [x] 04.1.1-02-PLAN.md — View model updates (status spinner, context-dependent footer, null command handling)

### Phase 5: Safety & Polish
**Goal**: Production-ready UI with safety warnings and refined interactions
**Depends on**: Phase 04.1.1
**Requirements**: SAFE-01, SAFE-02, SAFE-03, SAFE-04, SAFE-05, SAFE-06
**Success Criteria** (what must be TRUE):
  1. Dangerous commands display visual warning styling (alert colors from theme)
  2. Dangerous commands show textual explanation and require extra confirmation
  3. Low-confidence commands display visual warning styling
  4. Low-confidence commands show textual warning (single Enter to run per CONTEXT.md)
  5. All keyboard interactions are smooth and responsive
**Plans**: 4 plans in 3 waves

Plans:
- [ ] 05-01-PLAN.md — Warning infrastructure (Content::Warning, WarningKind, confirmation_pending state, footer updates)
- [ ] 05-02-PLAN.md — Warning rendering and double-Enter confirmation flow
- [ ] 05-03-PLAN.md — Tool call status display and fenced code block rendering
- [ ] 05-04-PLAN.md — Layout polish (separators, indicator column, content spacing)

## Progress

**Execution Order:**
Phases execute in numeric order: 1 -> 2 -> 3 -> 4 -> 04.1 -> 04.1.1 -> 5

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Core Event Loop Infrastructure | 3/3 | Complete | 2026-02-14 |
| 2. Block Rendering System | 1/1 | Complete | 2026-02-14 |
| 3. Command Generation | 1/1 | Complete | 2026-02-14 |
| 4. SSE Streaming & Conversation | 2/2 | Complete | 2026-02-14 |
| 04.1. Pure State Architecture Refactor | 3/3 | Complete | 2026-02-14 |
| 04.1.1. API Migration | 2/2 | Complete | 2026-02-16 |
| 5. Safety & Polish | 2/4 | In Progress|  |
