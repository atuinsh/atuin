# Integrating `mkt/kiro-e2e` with the Kiro harness (`mkt/kiro-harness`)

This branch holds the e2e side of the Kiro CLI harness: the fake `kiro-cli` agent and its V3
transcript writer (`crates/atuin/tests/common/agents.rs`), and the Kiro cases in
`crates/atuin/tests/e2e_ai_sessions.rs`. Everything compiles on its own
(`cargo test -p atuin --test e2e_ai_sessions --no-run`, clippy clean). Nothing is gated or
ignored: the Kiro cases are live in the `#[values]` / `#[case]` lists and in `Agent::ALL`, so
nothing needs flipping once the reader lands. Until then each Kiro case fails after the 30 s
wait with `timed out waiting for session sess_… to say …: there's no session sess_…`.

## The cases, and what the reader must do for each

| test | what it needs from the harness |
|---|---|
| `capture_keeps_the_conversation_and_tool_calls` (`Agent::Kiro`, `capture_tools` on and off) | `harness` is `"kiro"`; `cwd` is `session.json.workspacePaths[0]`; `said_in` sees exactly one user text and one assistant text per turn, so `Reasoning` rows are thinking content, not text, and `agent_note`, `session_start`, `session_metadata`, `usage_summary`, `session_event`, `turn_start`, `turn_end` never become user or assistant text; the tool call is named `execute_bash`, its input holds `args` (the command) and its result holds the `Output:…\n\n\nExit Code: 0` string; the thinking text must not show or be searchable; the secret in a prompt and in a tool result is redacted. |
| `capture_follows_a_transcript_exactly_once_across_restarts` | the listener follows appends to `messages.jsonl`, and the daemon restarts from its checkpoint without rereading rows. Note the `turn_end` row is stamped earlier than the two rows before it: go by file order. |
| `capture_starts_when_the_config_turns_it_on` (`Agent::Kiro`) | a session written while capture was off is left alone until appended to, and `atuin threads import` brings in the rest. Import iterates `AnyHarness::all()`, so `AnyHarness::Kiro` is enough. |
| `a_local_session_resumes_in_its_agent_where_it_ran::case_4_kiro` | `resume_plan` prints `cd -- <cwd> && kiro-cli chat --resume-id <id>` and leaves the transcript untouched. `resume_line` in `e2e_ai_sessions.rs` encodes exactly `["chat", "--resume-id", id]`: if the harness adds `--v3`, change that arm to match. |
| `continuing_in_another_agent_carries_the_conversation::kiro_to_claude` | the continuation turns the `execute_bash` call into Claude's `Bash` with the command and output. `Canonical::parse` in `crates/atuin-common/src/harnesstools/continuation/tools.rs` only knows `bash`, `shell`, `shell_command`, `exec_command`, `local_shell`: add `execute_bash` (and `execute_pwsh`) to `known` and to the shell arm, reading `command` from `args`. Without that the call becomes a `[ran a shell command]` note and the case fails on `call.name`. |
| `rebuilding_the_index_reproduces_it` | takes Kiro through `Agent::ALL`; nothing Kiro-specific beyond capture. |

## Names that must agree with the harness

- `Agent::harness()` is `"kiro"`: must equal `Kiro::name()`.
- `Agent::program()` is `"kiro-cli"`: the stand-in on `PATH`, and what `ResumePlan` runs. The
  stand-in prints `AGENT-RAN kiro-cli <args> IN <cwd>`, so a PTY test can wait on
  `ran(Agent::Kiro, "chat --resume-id <id>", cwd)`.
- `Agent::label()` is `"Kiro"`: the name atuin's status lines give it (`forked into a new Kiro
  session`, `continuing in Kiro`). Only the write-side cases compare it, none of which run for
  Kiro this round, so it is a note for the write-side follow-up.
- `Agent::tool()` is `execute_bash`.
- The fake writes under `$HOME/.kiro/sessions`. `FreshEnv` clears the environment and sets
  `HOME` to the machine's temporary home, so `KIRO_HOME` is unset and the harness's `kiro_home()`
  must fall back to `~/.kiro`. If the harness ever needs `KIRO_HOME` set, add it to
  `FreshEnv::env_vars` in `crates/atuin/tests/common/mod.rs`.

## What the fake writes

`Transcript::create` makes `sessions/<bucket>/sess_<uuid v4>/` with `session.json` (schema
`1.0.0`, `agentMode` `vibe`, `modelId` `auto`, `status` `idle`, `title` `""`), an empty
`messages.jsonl`, and the `{"op":"add",…}` line in `session-index/<bucket>.jsonl`; the bucket is
`sha256(cwd)[0:16]` (`sha2` is now a dev-dependency of `atuin`). Each `append` writes, per turn:
`user` (with `_meta.kiro.userMessageTag`), `turn_start`, assistant `Reasoning` (the turn's
thinking text, where real Kiro writes `"..."`), `tool_call` (`run_command_tooluse_…`,
`execute_bash`, `args.command`), `tool_result` (`Output:\n<output>\n\n\nExit Code: 0`), assistant
`Say` (the reply), `session_metadata`, `usage_summary`, `session_event` `session_pause`, and
`turn_end` stamped before the two rows ahead of it; on the first append an `agent_note`
`<session_context>` row goes first and the `session_start` row (system prompt) last. It then
rewrites `session.json` with `lastModifiedAt` bumped, a model-style title the first time, and the
append's directory added to `rootPaths` when it differs (what a resume from elsewhere does).
`Transcript::find` scans every bucket for `<id>/messages.jsonl`.

Not written: `ContextualHookInvoked` rows, `tombstone`/`Summary` compaction, `sub-executions/`,
`tool-outputs/`, `.index/index.json`. A reader test for those belongs with the harness's own
fixtures (`kiro-samples/`).

## Not covered this round

Copilot's other e2e cases need the write side (rehydrate, catch up, switch, fork, continuation
into Kiro), which this round leaves `Unsupported`, so Kiro is not in: `restored` and the four
tests built on it, `a_session_gone_on_with_elsewhere_catches_up_where_it_came_from`,
`forking_a_session_leaves_it_and_links_the_fork`, the `*_to_kiro` continuation cases, and
`a_subagent_resumes_as_the_session_it_works_for` (subagents are a follow-up). Add them with the
write-side PR; the writer already models a resume from another directory.

There is no e2e hook-capture case for Copilot to mirror: the hook payload cases are rstest unit
tests in `crates/atuin/src/command/client/hook/event.rs`, which the hook branch (`B`) adds for
Kiro from `kiro-samples/hook-payloads/`.
