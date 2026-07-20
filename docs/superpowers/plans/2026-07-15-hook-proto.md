# Hook Protocol Types (`hook/proto.rs`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the bare `serde_json::Value` poking in `crates/atuin/src/command/client/hook.rs` with a typed protocol module (`hook/proto.rs`) that models the JSON coding agents send to `atuin hook`, while preserving the current parsing behavior exactly.

**Architecture:** Coding agents (Claude Code, Codex) invoke `atuin hook <agent>` per tool use and pass the event as JSON on stdin. Today `parse_hook_stdin` walks a `serde_json::Value` field-by-field, and `add_hook_entries` builds/inspects install config with `json!` literals and stringly `value["key"]` indexing. We introduce `hook/proto.rs` with (1) **inbound** wire structs that deserialize the agent event plus a small `HookEvent` domain enum the command acts on, and (2) **outbound** typed structs for the hook-registration entry Atuin writes into the agent's config file. The install code keeps the user's overall config document as `Value` (it is arbitrary foreign JSON we must round-trip), but constructs and detects *its own* entries through the typed structs.

**Tech Stack:** Rust (edition 2024), `serde` (derive), `serde_json`, `eyre`.

## Global Constraints

- Module path is `crates/atuin/src/command/client/hook/proto.rs`, declared as `mod proto;` inside `crates/atuin/src/command/client/hook.rs` (Rust 2024 supports a `hook.rs` file with a sibling `hook/` directory for submodules).
- `serde` is already a dependency of the `atuin` crate with the `derive` feature (workspace default). Do **not** add new dependencies.
- The `hook` module builds under the crate's default features (`client` is default); tests run with `cargo test -p atuin`.
- **Behavior parity is mandatory.** The reduction from a decoded event to a `HookEvent` must match the current `parse_hook_stdin` for every well-formed agent payload:
  - `tool_name` must equal `"Bash"`, else `Skip`.
  - `tool_use_id` must be present and non-empty, else `Skip`.
  - `PreToolUse`: `command` = `tool_input.command` or `""`; empty ⇒ `Skip`; `intent` = `tool_input.description` (optional).
  - `PostToolUse`: `exit` = `tool_response.exitCode` or `0`.
  - `PostToolUseFailure`: `exit` = `1` (ignore `tool_response` entirely).
  - Any other / missing `hook_event_name` ⇒ `Skip`.
  - Unknown fields anywhere (e.g. `session_id`, `cwd`, extra `tool_response` keys) are ignored.
- One **intentional, documented** difference from the old code: a field that is present but has a grossly wrong JSON type (e.g. `"tool_name": 5`) now yields a JSON parse error instead of a silent `Skip`. Real agents never send this. Invalid JSON already errored before and continues to.

---

### Task 1: Inbound protocol module (`hook/proto.rs`)

Create the typed protocol and move the parsing logic out of `hook.rs`. This task is self-contained: `proto.rs` compiles and is fully unit-tested on its own before `hook.rs` is rewired in Task 2.

**Files:**
- Create: `crates/atuin/src/command/client/hook/proto.rs`
- Modify: `crates/atuin/src/command/client/hook.rs` (add `mod proto;` only — the switch-over happens in Task 2)

**Interfaces:**
- Consumes: nothing (leaf module).
- Produces (used by Task 2):
  - `pub const BASH_TOOL_NAME: &str`
  - `pub enum HookEvent { Start { command: String, intent: Option<String>, tool_use_id: String }, End { tool_use_id: String, exit: i64 }, Skip }`
  - `pub fn parse_hook_stdin(input: &str) -> eyre::Result<HookEvent>`
  - Wire types `WireHookEvent`, `HookEventName`, `WireToolInput`, `WireToolResponse` and `WireHookEvent::into_event(self) -> HookEvent`.

- [ ] **Step 1: Declare the submodule from `hook.rs`**

In `crates/atuin/src/command/client/hook.rs`, add the module declaration directly after the existing `use super::history;` line (around line 10):

```rust
use super::history;

mod proto;
```

- [ ] **Step 2: Write the failing tests in `hook/proto.rs`**

Create `crates/atuin/src/command/client/hook/proto.rs` with only the test module for now, so the tests fail to compile (the types don't exist yet):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_tool_use_becomes_start() {
        let input = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hello", "description": "Test greeting"},
            "tool_use_id": "toolu_abc123",
            "session_id": "sess1",
            "cwd": "/tmp"
        }"#;

        assert_eq!(
            parse_hook_stdin(input).unwrap(),
            HookEvent::Start {
                command: "echo hello".to_string(),
                intent: Some("Test greeting".to_string()),
                tool_use_id: "toolu_abc123".to_string(),
            }
        );
    }

    #[test]
    fn post_tool_use_becomes_end_with_exit_code() {
        let input = r#"{
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hello"},
            "tool_response": {"exitCode": 3, "stdout": "hello\n"},
            "tool_use_id": "toolu_abc123"
        }"#;

        assert_eq!(
            parse_hook_stdin(input).unwrap(),
            HookEvent::End {
                tool_use_id: "toolu_abc123".to_string(),
                exit: 3,
            }
        );
    }

    #[test]
    fn post_tool_use_without_exit_code_defaults_to_zero() {
        let input = r#"{
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {},
            "tool_use_id": "toolu_abc123"
        }"#;

        assert_eq!(
            parse_hook_stdin(input).unwrap(),
            HookEvent::End {
                tool_use_id: "toolu_abc123".to_string(),
                exit: 0,
            }
        );
    }

    #[test]
    fn failure_event_forces_exit_one_and_ignores_response() {
        let input = r#"{
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash",
            "tool_input": {"command": "false"},
            "tool_response": {"exitCode": 0},
            "tool_use_id": "toolu_abc123"
        }"#;

        assert_eq!(
            parse_hook_stdin(input).unwrap(),
            HookEvent::End {
                tool_use_id: "toolu_abc123".to_string(),
                exit: 1,
            }
        );
    }

    #[test]
    fn non_bash_tool_is_skipped() {
        let input = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": {"file_path": "/tmp/test.txt", "content": "hello"},
            "tool_use_id": "toolu_abc123"
        }"#;

        assert_eq!(parse_hook_stdin(input).unwrap(), HookEvent::Skip);
    }

    #[test]
    fn missing_tool_use_id_is_skipped() {
        let input = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hi"}
        }"#;

        assert_eq!(parse_hook_stdin(input).unwrap(), HookEvent::Skip);
    }

    #[test]
    fn empty_tool_use_id_is_skipped() {
        let input = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hi"},
            "tool_use_id": ""
        }"#;

        assert_eq!(parse_hook_stdin(input).unwrap(), HookEvent::Skip);
    }

    #[test]
    fn empty_command_is_skipped() {
        let input = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": ""},
            "tool_use_id": "toolu_abc123"
        }"#;

        assert_eq!(parse_hook_stdin(input).unwrap(), HookEvent::Skip);
    }

    #[test]
    fn pre_tool_use_without_description_has_no_intent() {
        let input = r#"{
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_use_id": "toolu_abc123"
        }"#;

        assert_eq!(
            parse_hook_stdin(input).unwrap(),
            HookEvent::Start {
                command: "ls".to_string(),
                intent: None,
                tool_use_id: "toolu_abc123".to_string(),
            }
        );
    }

    #[test]
    fn unknown_event_name_is_skipped() {
        let input = r#"{
            "hook_event_name": "SomeFutureEvent",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "tool_use_id": "toolu_abc123"
        }"#;

        assert_eq!(parse_hook_stdin(input).unwrap(), HookEvent::Skip);
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse_hook_stdin("not json").is_err());
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail to compile**

Run: `cargo test -p atuin proto 2>&1 | tail -20`
Expected: FAIL — compile errors like ``cannot find function `parse_hook_stdin` in this scope`` / `cannot find type `HookEvent``.

- [ ] **Step 4: Implement the inbound protocol above the test module**

Insert this at the **top** of `crates/atuin/src/command/client/hook/proto.rs`, before the `#[cfg(test)] mod tests`:

```rust
//! Typed protocol for the hook events coding agents send to `atuin hook`.
//!
//! Claude Code and Codex invoke `atuin hook <agent>` for each tool use and
//! pass the event as JSON on stdin. Both agents share the same schema (Codex
//! mirrors Claude Code's hook format), so a single set of wire types serves
//! both. This module decodes that JSON into typed structs and reduces it to a
//! small [`HookEvent`] the rest of the hook command acts on, instead of
//! walking a bare `serde_json::Value`.
//!
//! Compatibility notes:
//! - Unknown fields (e.g. `session_id`, `cwd`, and everything in
//!   `tool_response` besides `exitCode`) are ignored, so new agent fields
//!   never break parsing.
//! - Unrecognized `hook_event_name` values decode to [`HookEventName::Other`]
//!   and are skipped rather than erroring.
//! - Every field an agent may omit is optional, matching the previous
//!   permissive parsing.

use eyre::Result;
use serde::Deserialize;

/// The tool name agents use for shell execution. Only these events are
/// recorded; every other tool (file writes, web fetches, ...) is skipped.
pub const BASH_TOOL_NAME: &str = "Bash";

/// A hook event exactly as an agent serializes it on stdin.
///
/// Field types mirror the agent JSON schema. Missing fields decode to `None`;
/// unknown fields are ignored.
#[derive(Debug, Deserialize)]
pub struct WireHookEvent {
    #[serde(default)]
    pub hook_event_name: Option<HookEventName>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub tool_input: Option<WireToolInput>,
    #[serde(default)]
    pub tool_response: Option<WireToolResponse>,
}

/// The lifecycle stage an event represents.
///
/// The wire values are PascalCase and match these variant names exactly.
/// Unrecognized values map to [`HookEventName::Other`] so future or
/// agent-specific events are skipped rather than rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum HookEventName {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    #[serde(other)]
    Other,
}

/// The `tool_input` object: what the agent is about to run.
#[derive(Debug, Deserialize)]
pub struct WireToolInput {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// The `tool_response` object: how the command finished. Only the exit code is
/// consumed; all other fields are ignored.
#[derive(Debug, Deserialize)]
pub struct WireToolResponse {
    #[serde(rename = "exitCode", default)]
    pub exit_code: Option<i64>,
}

/// The reduced event the hook command acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    /// A Bash command is about to run; open a history entry.
    Start {
        command: String,
        intent: Option<String>,
        tool_use_id: String,
    },
    /// A Bash command finished; close the matching history entry.
    End { tool_use_id: String, exit: i64 },
    /// Nothing to record (non-Bash tool, missing id, empty command, or an
    /// event we don't track).
    Skip,
}

impl WireHookEvent {
    /// Reduce a decoded wire event to a [`HookEvent`]. Infallible: anything we
    /// do not act on becomes [`HookEvent::Skip`].
    pub fn into_event(self) -> HookEvent {
        if self.tool_name.as_deref() != Some(BASH_TOOL_NAME) {
            return HookEvent::Skip;
        }

        let tool_use_id = match self.tool_use_id {
            Some(id) if !id.is_empty() => id,
            _ => return HookEvent::Skip,
        };

        match self.hook_event_name {
            Some(HookEventName::PreToolUse) => {
                let (command, intent) = match self.tool_input {
                    Some(input) => (input.command.unwrap_or_default(), input.description),
                    None => (String::new(), None),
                };

                if command.is_empty() {
                    return HookEvent::Skip;
                }

                HookEvent::Start {
                    command,
                    intent,
                    tool_use_id,
                }
            }
            Some(HookEventName::PostToolUse) => {
                let exit = self
                    .tool_response
                    .and_then(|response| response.exit_code)
                    .unwrap_or(0);
                HookEvent::End { tool_use_id, exit }
            }
            Some(HookEventName::PostToolUseFailure) => HookEvent::End {
                tool_use_id,
                exit: 1,
            },
            Some(HookEventName::Other) | None => HookEvent::Skip,
        }
    }
}

/// Parse a raw hook payload (the JSON an agent writes to stdin) into a
/// [`HookEvent`]. Errors only when the input is not valid JSON.
pub fn parse_hook_stdin(input: &str) -> Result<HookEvent> {
    let event: WireHookEvent = serde_json::from_str(input)?;
    Ok(event.into_event())
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p atuin proto 2>&1 | tail -20`
Expected: PASS — all 11 tests in `command::client::hook::proto::tests` pass.

- [ ] **Step 6: Confirm no warnings from the new module**

Run: `cargo clippy -p atuin 2>&1 | grep -i "hook/proto\|hook::proto" || echo "no proto warnings"`
Expected: `no proto warnings`. (An `unused` warning on the not-yet-used public items is acceptable if it appears — Task 2 wires them in. If clippy flags dead code, do not silence it; proceed to Task 2 which removes the duplication.)

- [ ] **Step 7: Commit**

```bash
git add crates/atuin/src/command/client/hook/proto.rs crates/atuin/src/command/client/hook.rs
git commit -m "feat(hook): add typed proto module for agent hook events"
```

---

### Task 2: Rewire `hook.rs` onto the typed protocol

Delete the inline `HookEvent` enum and `parse_hook_stdin` from `hook.rs` and use `proto` instead. The four parse-focused tests in `hook.rs` are now covered by `proto.rs` (Task 1), so remove them from `hook.rs` to avoid duplication; the clap/argument-parsing tests stay.

**Files:**
- Modify: `crates/atuin/src/command/client/hook.rs`

**Interfaces:**
- Consumes: `proto::{HookEvent, parse_hook_stdin}` (from Task 1).
- Produces: unchanged public surface of `hook::Cmd` (the `run`/`handle`/`install` behavior is identical).

- [ ] **Step 1: Import the protocol types**

In `crates/atuin/src/command/client/hook.rs`, immediately after the `mod proto;` line added in Task 1, add:

```rust
mod proto;

use proto::{HookEvent, parse_hook_stdin};
```

- [ ] **Step 2: Delete the inline `HookEvent` enum and `parse_hook_stdin`**

Remove the entire block from `#[derive(Debug)] enum HookEvent { ... }` through the end of `fn parse_hook_stdin(...) { ... }` (currently lines 122–185). Concretely, delete this span:

```rust
#[derive(Debug)]
enum HookEvent {
    Start {
        command: String,
        intent: Option<String>,
        tool_use_id: String,
    },
    End {
        tool_use_id: String,
        exit: i64,
    },
    Skip,
}

fn parse_hook_stdin(input: &str) -> Result<HookEvent> {
    let v: Value = serde_json::from_str(input)?;

    if v.get("tool_name").and_then(|t| t.as_str()) != Some("Bash") {
        return Ok(HookEvent::Skip);
    }

    let tool_use_id = match v.get("tool_use_id").and_then(|t| t.as_str()) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => return Ok(HookEvent::Skip),
    };

    match v.get("hook_event_name").and_then(|e| e.as_str()) {
        Some("PreToolUse") => {
            let tool_input = v.get("tool_input");
            let command = tool_input
                .and_then(|ti| ti.get("command"))
                .and_then(|c| c.as_str())
                .unwrap_or("");

            if command.is_empty() {
                return Ok(HookEvent::Skip);
            }

            let intent = tool_input
                .and_then(|ti| ti.get("description"))
                .and_then(|d| d.as_str())
                .map(String::from);

            Ok(HookEvent::Start {
                command: command.to_string(),
                intent,
                tool_use_id,
            })
        }
        Some(event @ ("PostToolUse" | "PostToolUseFailure")) => {
            let exit = if event == "PostToolUseFailure" {
                1
            } else {
                v.get("tool_response")
                    .and_then(|tr| tr.get("exitCode"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
            };

            Ok(HookEvent::End { tool_use_id, exit })
        }
        _ => Ok(HookEvent::Skip),
    }
}
```

Leave the `id_file_path` function and everything after it in place.

- [ ] **Step 3: Fix the `parse_hook_stdin` call site in `handle`**

In `handle`, `parse_hook_stdin` and the `HookEvent` variants are now the imported ones — no code change is needed to the `match parse_hook_stdin(&input)? { ... }` block because the variant names and fields are identical. Verify the block still reads:

```rust
    match parse_hook_stdin(&input)? {
        HookEvent::Start {
            command,
            intent,
            tool_use_id,
        } => {
```

(No edit expected here; this step is a visual confirmation.)

- [ ] **Step 4: Remove the now-duplicated parse tests from `hook.rs`**

In the `#[cfg(test)] mod tests` block at the bottom of `hook.rs`, delete these four test functions (their coverage moved to `proto.rs`): `test_parse_pre_tool_use`, `test_parse_post_tool_use`, `test_parse_non_bash_tool_skipped`, and `test_parse_failure_event`. Keep every other test (`parse_hook_agent_command`, `parse_hook_install_command`, `parse_hook_install_pi_command`, `agent_from_name_supports_pi`, `parse_top_level_hook_command`).

- [ ] **Step 5: Build to check `serde_json::Value` is still used**

`Value` is still needed by the install code (`install` / `add_hook_entries`), so the `use serde_json::Value;` import stays. Confirm the crate compiles:

Run: `cargo build -p atuin 2>&1 | tail -20`
Expected: builds with no errors and no `unused import: serde_json::Value` warning (install code still uses it).

- [ ] **Step 6: Run the full hook test suite**

Run: `cargo test -p atuin hook 2>&1 | tail -25`
Expected: PASS — the surviving clap tests in `hook::tests` and all `hook::proto::tests` pass; no test references a removed symbol.

- [ ] **Step 7: Commit**

```bash
git add crates/atuin/src/command/client/hook.rs
git commit -m "refactor(hook): parse agent events through typed proto module"
```

---

### Task 3: Type the outbound install-registration entry

Replace the `serde_json::json!` literal and the stringly `entry["hooks"]` / `hook["command"]` indexing in `add_hook_entries` with typed `HookMatcher` / `HookCommand` structs. The user's overall config document stays a `serde_json::Value` (it is arbitrary foreign JSON that must round-trip untouched); only the entry Atuin owns is typed.

**Files:**
- Modify: `crates/atuin/src/command/client/hook/proto.rs` (add serialize types + tests)
- Modify: `crates/atuin/src/command/client/hook.rs` (use them in `add_hook_entries`)

**Interfaces:**
- Consumes: nothing new.
- Produces (used by `hook.rs`):
  - `pub struct HookMatcher { pub matcher: String, pub hooks: Vec<HookCommand> }` — `Serialize + Deserialize`
  - `pub struct HookCommand { #[serde(rename = "type")] pub kind: String, pub command: String }` — `Serialize + Deserialize`
  - `HookCommand::command_hook(command: impl Into<String>) -> HookCommand` (sets `kind = "command"`).

- [ ] **Step 1: Write failing tests for the outbound types**

Add these tests inside the existing `#[cfg(test)] mod tests` in `crates/atuin/src/command/client/hook/proto.rs`:

```rust
    #[test]
    fn hook_matcher_serializes_to_agent_schema() {
        let entry = HookMatcher {
            matcher: "Bash".to_string(),
            hooks: vec![HookCommand::command_hook("atuin hook claude-code")],
        };

        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            serde_json::json!({
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "atuin hook claude-code"}]
            })
        );
    }

    #[test]
    fn hook_matcher_roundtrips_and_exposes_command() {
        let value = serde_json::json!({
            "matcher": "Bash",
            "hooks": [{"type": "command", "command": "atuin hook claude-code"}]
        });

        let entry: HookMatcher = serde_json::from_value(value).unwrap();
        assert!(
            entry
                .hooks
                .iter()
                .any(|hook| hook.command == "atuin hook claude-code")
        );
    }

    #[test]
    fn hook_matcher_tolerates_foreign_fields() {
        // Other tools add entries with extra keys; deserializing our view of
        // them must ignore those keys rather than fail.
        let value = serde_json::json!({
            "matcher": "Bash",
            "hooks": [{"type": "command", "command": "some-other-tool", "timeout": 5}],
            "comment": "installed by another tool"
        });

        let entry: HookMatcher = serde_json::from_value(value).unwrap();
        assert_eq!(entry.hooks[0].command, "some-other-tool");
    }
```

- [ ] **Step 2: Run the tests to verify they fail to compile**

Run: `cargo test -p atuin proto 2>&1 | tail -20`
Expected: FAIL — ``cannot find type `HookMatcher``` / ``cannot find type `HookCommand```.

- [ ] **Step 3: Implement the outbound types**

In `crates/atuin/src/command/client/hook/proto.rs`, change the top-of-file import to bring in `Serialize`:

```rust
use serde::{Deserialize, Serialize};
```

Then add, just below `WireToolResponse` (before the `HookEvent` enum), the outbound types:

```rust
/// One entry in an agent's hook array: a matcher plus the command hooks to run
/// when it matches. This is the shape Atuin writes into (and looks for in) the
/// agent config file (`~/.claude/settings.json`, `~/.codex/hooks.json`).
///
/// Deserialization is permissive (unknown keys ignored) because the array also
/// holds entries other tools installed, which we must read past without error.
#[derive(Debug, Serialize, Deserialize)]
pub struct HookMatcher {
    pub matcher: String,
    pub hooks: Vec<HookCommand>,
}

/// A single command hook. `kind` serializes as the `"type"` field and is always
/// `"command"` for the hooks Atuin installs.
#[derive(Debug, Serialize, Deserialize)]
pub struct HookCommand {
    #[serde(rename = "type")]
    pub kind: String,
    pub command: String,
}

impl HookCommand {
    /// Build a `"command"`-type hook that runs `command`.
    pub fn command_hook(command: impl Into<String>) -> Self {
        Self {
            kind: "command".to_string(),
            command: command.into(),
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p atuin proto 2>&1 | tail -20`
Expected: PASS — all `proto::tests` pass, including the three new ones.

- [ ] **Step 5: Use the typed entry in `add_hook_entries`**

In `crates/atuin/src/command/client/hook.rs`, extend the proto import to include the new types:

```rust
use proto::{HookCommand, HookEvent, HookMatcher, parse_hook_stdin};
```

Then, in `add_hook_entries`, normalize the destructured `&'static str` fields to plain `&str` by adding this line right after the `let InstallKind::JsonHooks { ... } = agent.install_kind() else { ... };` destructure (the `matcher` and `hook_command` bindings come out as `&&'static str` under match ergonomics; deref once so the code below reads cleanly):

```rust
    let (matcher, hook_command) = (*matcher, *hook_command);
```

Replace the already-installed detection block:

```rust
        let already_installed = arr.iter().any(|entry| {
            entry["hooks"].as_array().is_some_and(|h| {
                h.iter()
                    .any(|hook| hook["command"].as_str() == Some(hook_command))
            })
        });
```

with a typed version (foreign entries that don't fit our view simply don't match):

```rust
        let already_installed = arr.iter().any(|entry| {
            serde_json::from_value::<HookMatcher>(entry.clone())
                .is_ok_and(|entry| entry.hooks.iter().any(|hook| hook.command == hook_command))
        });
```

Replace the entry construction:

```rust
        arr.push(serde_json::json!({
            "matcher": matcher,
            "hooks": [{"type": "command", "command": hook_command}]
        }));
```

with:

```rust
        let entry = HookMatcher {
            matcher: matcher.to_string(),
            hooks: vec![HookCommand::command_hook(hook_command)],
        };
        arr.push(serde_json::to_value(entry)?);
```

- [ ] **Step 6: Build and run the full hook suite**

Run: `cargo test -p atuin hook 2>&1 | tail -25`
Expected: PASS — install/parse tests all pass; no compile errors. (`serde_json::Value` remains in use for the config-document root and array walking.)

- [ ] **Step 7: Manually verify install output is byte-identical to the old format**

The typed entry must serialize to the exact JSON the old `json!` produced (so re-running install on a config written by an old Atuin still detects the existing hook). Confirm with a scratch run against a temp HOME:

```bash
tmp="$(mktemp -d)"; HOME="$tmp" cargo run -q -p atuin -- hook install claude-code >/dev/null 2>&1; \
cat "$tmp/.claude/settings.json"; rm -rf "$tmp"
```

Expected: the `hooks` object contains `PreToolUse`, `PostToolUse`, and `PostToolUseFailure`, each an array with one entry of the form:

```json
{ "matcher": "Bash", "hooks": [ { "type": "command", "command": "atuin hook claude-code" } ] }
```

- [ ] **Step 8: Verify idempotent re-install still skips**

Run install a second time against the same temp HOME and confirm it reports the entries as already installed (this exercises the typed detection path against a document the first run wrote):

```bash
tmp="$(mktemp -d)"; \
HOME="$tmp" cargo run -q -p atuin -- hook install claude-code >/dev/null 2>&1; \
HOME="$tmp" cargo run -q -p atuin -- hook install claude-code 2>&1; \
rm -rf "$tmp"
```

Expected output on the second run:

```
hooks.PreToolUse: already installed, skipping
hooks.PostToolUse: already installed, skipping
hooks.PostToolUseFailure: already installed, skipping
```

- [ ] **Step 9: Commit**

```bash
git add crates/atuin/src/command/client/hook/proto.rs crates/atuin/src/command/client/hook.rs
git commit -m "refactor(hook): build and detect install entries with typed proto"
```

---

## Self-Review

**Spec coverage:**
- "Create `proto.rs` defining the protocol" → Task 1 (inbound) + Task 3 (outbound). ✓
- "Avoid raw JSONs / bare `json::Value`" → inbound `parse_hook_stdin` fully typed (Task 2 deletes the `Value`-walking parser); outbound construction/detection typed (Task 3). The config-document *root* intentionally stays `Value` because it is arbitrary foreign user JSON — documented in the Task 3 preamble. ✓
- "Good compatibility with the existing protocol" → Global Constraints enumerate the exact behavior contract; Task 1 tests assert each rule (Bash-only, id required, empty-command skip, failure⇒exit 1, missing exitCode⇒0, unknown event⇒skip, unknown fields ignored); Task 3 steps 7–8 verify install output byte-parity and idempotent re-install. The single intentional divergence (wrong-typed field ⇒ parse error) is called out. ✓

**Placeholder scan:** No TBD/TODO/"handle edge cases"/"write tests for the above" — every step shows concrete code or an exact command with expected output. ✓

**Type consistency:** `HookEvent`, `WireHookEvent`, `WireToolInput`, `WireToolResponse`, `HookEventName`, `HookMatcher`, `HookCommand`, `HookCommand::command_hook`, `parse_hook_stdin`, and `BASH_TOOL_NAME` are named identically across the Interfaces blocks, implementation, and call sites. `HookEvent` variants (`Start { command, intent, tool_use_id }`, `End { tool_use_id, exit }`, `Skip`) match the existing `handle` match arms exactly, so Task 2 needs no change to `handle`'s body. ✓
