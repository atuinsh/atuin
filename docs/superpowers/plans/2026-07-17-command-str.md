# CommandStr Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce a `CommandStr` type in `atuin-common` that carries a command string only up to its first NUL byte, and use it on the hook ingress so commands with stray NULs (issue #3589) are truncated at the deserialization boundary instead of reaching history.

**Architecture:** `CommandStr` is a newtype over `std::ffi::CString`. Every constructor — including its custom `serde::Deserialize` impl — keeps only the bytes before the first NUL, so the wrapped value can never hold an interior NUL and always dereferences to valid UTF-8. The hook wire type (`WireToolInput.command`) deserializes straight into `Option<CommandStr>`, so truncation happens once, at the JSON boundary; the truncated value is then carried through `HookEvent::Start.command` down to `start_history_entry`, which takes `&str` (reached via `Deref`).

**Tech Stack:** Rust (edition 2024), `serde` (already a hard dependency of `atuin-common`), `serde_json` (test-only), `proptest` + `rstest` for tests, `std::ffi::CString`.

## Global Constraints

- Type lives in `atuin-common` under `src/string/`, matching the existing `ellipsis` / `escape_non_printable_posix_ext` submodule pattern.
- `Serialize` / `Deserialize` are **unconditional** — no `serde` feature flag. `serde` is already a non-optional dependency of `atuin-common`.
- The type wraps `CString` and truncates at the **first NUL byte**. UTF-8 in is UTF-8 out (NUL `0x00` is always a single-byte char, so a byte-offset truncation is always on a char boundary).
- Minimal comments: doc comments on the public type and its methods; inline comments only where an invariant isn't obvious (e.g. why an `.expect()` can't fire).
- Use standard derives only (`Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default`). The `Deref`/`AsRef`/`From`/`Display` impls are hand-written because they must target `str` and must truncate — `derive_more` would target the inner `CString` and would not truncate.
- Follow existing test style in `crates/atuin-common/src/string/escape_non_printable_posix_ext.rs`: an `rstest` table of exact cases plus `proptest` invariants.

---

### Task 1: Create the `CommandStr` type in `atuin-common`

**Files:**
- Create: `crates/atuin-common/src/string/command_str.rs`
- Modify: `crates/atuin-common/src/string/mod.rs:1-9`
- Modify: `crates/atuin-common/Cargo.toml:43-46` (add `serde_json` dev-dependency)

**Interfaces:**
- Consumes: nothing (leaf type).
- Produces:
  - `pub struct CommandStr(CString)` in `atuin_common::string`, re-exported as `atuin_common::string::CommandStr`.
  - `CommandStr::new(s: impl AsRef<str>) -> CommandStr` — truncates at first NUL.
  - `CommandStr::as_str(&self) -> &str` — always-valid-UTF-8 view.
  - `impl Deref<Target = str>`, `impl AsRef<str>`, `impl Display`, `impl From<&str>`, `impl From<String>`, `impl Default` (empty), `impl Serialize`, `impl<'de> Deserialize<'de>`.

- [ ] **Step 1: Add the `serde_json` dev-dependency**

The unit tests exercise the serde impls with `serde_json`, which `atuin-common` does not yet pull in for tests. Add it to `[dev-dependencies]` in `crates/atuin-common/Cargo.toml`:

```toml
[dev-dependencies]
pretty_assertions = { workspace = true }
proptest = { workspace = true }
rstest = { workspace = true }
serde_json = { workspace = true }
```

- [ ] **Step 2: Write the failing test file**

Create `crates/atuin-common/src/string/command_str.rs` with **only** the test module for now (the type doesn't exist yet, so this must fail to compile — that is the "red" state):

```rust
#[cfg(test)]
mod tests {
    use super::CommandStr;
    use proptest::prelude::*;
    use rstest::rstest;

    /// Exact truncation cases. Each is its own test so a failure names the input.
    #[rstest]
    // No NUL: preserved verbatim.
    #[case("echo hello", "echo hello")]
    #[case("", "")]
    // Multi-byte UTF-8 before a NUL survives intact.
    #[case("🦀 build\0junk", "🦀 build")]
    // Interior NUL: everything from the NUL onward is dropped (issue #3589).
    #[case("echo hi\0rm -rf /", "echo hi")]
    // Trailing NUL: the NUL and anything after it.
    #[case("ls\0", "ls")]
    // Leading NUL: nothing survives.
    #[case("\0danger", "")]
    // Only the first NUL matters.
    #[case("a\0b\0c", "a")]
    fn truncates_at_first_nul(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(CommandStr::new(input).as_str(), expected);
    }

    #[test]
    fn default_is_empty() {
        assert_eq!(CommandStr::default().as_str(), "");
        assert!(CommandStr::default().is_empty());
    }

    #[test]
    fn equal_when_truncations_match() {
        assert_eq!(CommandStr::new("a\0b"), CommandStr::new("a"));
        assert_ne!(CommandStr::new("a"), CommandStr::new("b"));
    }

    #[test]
    fn derefs_to_str() {
        let c = CommandStr::new("echo hi\0x");
        assert_eq!(c.len(), 7);
        assert!(c.starts_with("echo"));
        assert!(!c.is_empty());
    }

    #[test]
    fn display_shows_truncated_text() {
        assert_eq!(CommandStr::new("ls -la\0\0").to_string(), "ls -la");
    }

    #[test]
    fn from_str_and_string_agree() {
        assert_eq!(
            CommandStr::from("cat\0x"),
            CommandStr::from(String::from("cat\0y"))
        );
    }

    #[test]
    fn serializes_as_plain_string() {
        let json = serde_json::to_string(&CommandStr::new("echo hi\0x")).unwrap();
        assert_eq!(json, r#""echo hi""#);
    }

    #[test]
    fn deserializes_and_truncates() {
        // `\u0000` is a NUL escaped inside a JSON string.
        let c: CommandStr = serde_json::from_str(r#""echo hi\u0000rm -rf /""#).unwrap();
        assert_eq!(c.as_str(), "echo hi");
    }

    #[test]
    fn deserialize_rejects_non_string() {
        // A number is not a command; serde surfaces a data-category error.
        let err = serde_json::from_str::<CommandStr>("42").unwrap_err();
        assert!(err.is_data());
    }

    proptest! {
        /// A string with no NUL is preserved byte-for-byte.
        #[test]
        fn nul_free_is_unchanged(s in r"[^\x00]*") {
            prop_assert_eq!(CommandStr::new(&s).as_str(), s.as_str());
        }

        /// The stored value never contains a NUL, whatever the input.
        #[test]
        fn never_contains_nul(s in r"(?s).*") {
            prop_assert!(!CommandStr::new(&s).as_str().contains('\0'));
        }

        /// The result is exactly the input up to (excluding) its first NUL.
        #[test]
        fn equals_prefix_before_first_nul(s in r"(?s).*") {
            let expected = s.split('\0').next().unwrap();
            prop_assert_eq!(CommandStr::new(&s).as_str(), expected);
        }

        /// serialize → deserialize round-trips any already-NUL-free command.
        #[test]
        fn serde_round_trip(s in r"[^\x00]*") {
            let original = CommandStr::new(&s);
            let json = serde_json::to_string(&original).unwrap();
            let back: CommandStr = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(back, original);
        }
    }
}
```

- [ ] **Step 3: Wire the module into `string/mod.rs`**

So the test module (and later the type) are compiled. Edit `crates/atuin-common/src/string/mod.rs` to add the submodule and re-export. Final file:

```rust
//! String-related utilities and extension traits.
mod command_str;
#[cfg(feature = "unicode")]
pub mod ellipsis;
mod escape_non_printable_posix_ext;

pub use command_str::CommandStr;
#[cfg(feature = "unicode")]
pub use ellipsis::EllipsizeExt;
pub use escape_non_printable_posix_ext::EscapeNonPrintablePosixExt;
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p atuin-common --lib string::command_str`
Expected: FAIL — compile error, `cannot find type CommandStr in this scope` (the type isn't defined yet).

- [ ] **Step 5: Write the minimal implementation**

Prepend the implementation to `crates/atuin-common/src/string/command_str.rs`, **above** the existing `#[cfg(test)] mod tests` block:

```rust
//! A command string sanitized at ingest: everything up to its first NUL byte.
//!
//! Coding agents occasionally hand `atuin hook` a command carrying a NUL byte
//! and trailing junk. A C string ends at its first NUL, and so does this: the
//! stored value holds no interior NUL, so the parts of Atuin that treat a
//! command as text never see one.

use std::ffi::CString;
use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A command truncated at its first NUL byte.
///
/// Every constructor keeps only the bytes before the first NUL, so the wrapped
/// [`CString`] never contains an interior NUL and always dereferences to valid
/// UTF-8.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct CommandStr(CString);

impl CommandStr {
    /// Build a `CommandStr`, keeping only the text before the first NUL byte.
    pub fn new(s: impl AsRef<str>) -> Self {
        let s = s.as_ref();
        let end = s.find('\0').unwrap_or(s.len());
        // `s[..end]` stops before the first NUL, so it has no interior NUL and
        // `CString::new` cannot fail.
        Self(CString::new(&s.as_bytes()[..end]).expect("slice has no interior NUL byte"))
    }

    /// The command as a string slice — always valid UTF-8 by construction.
    pub fn as_str(&self) -> &str {
        self.0
            .to_str()
            .expect("constructed from valid UTF-8, so always valid UTF-8")
    }
}

impl Deref for CommandStr {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for CommandStr {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for CommandStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for CommandStr {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for CommandStr {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl Serialize for CommandStr {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CommandStr {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::new(s))
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p atuin-common --lib string::command_str`
Expected: PASS — all `truncates_at_first_nul` cases, the unit tests, and the four proptests pass.

- [ ] **Step 7: Lint**

Run: `cargo clippy -p atuin-common --all-targets`
Expected: no warnings introduced by `command_str.rs`.

- [ ] **Step 8: Commit**

```bash
git add crates/atuin-common/src/string/command_str.rs \
        crates/atuin-common/src/string/mod.rs \
        crates/atuin-common/Cargo.toml
git commit -m "feat(common): add CommandStr, a command truncated at its first NUL"
```

---

### Task 2: Adopt `CommandStr` on the hook ingress (wire + event)

**Files:**
- Modify: `crates/atuin/src/command/client/hook/wire.rs:6-56`
- Modify: `crates/atuin/src/command/client/hook/event.rs:7-93` (types + `From` impl)
- Modify: `crates/atuin/src/command/client/hook/event.rs:117-326` (tests)

**Interfaces:**
- Consumes: `atuin_common::string::CommandStr` from Task 1.
- Produces:
  - `WireToolInput.command: Option<CommandStr>` (was `Option<String>`).
  - `HookEvent::Start.command: CommandStr` (was `String`).
  - No change to `crates/atuin/src/command/client/hook.rs`: it passes `&command` to `start_history_entry(command: &str, …)`, and `&CommandStr` reaches `&str` by deref coercion.

> These two files must change together: making `WireToolInput.command` a `CommandStr` forces `event.rs` (which reads `input.command`) to change in the same edit, so this is a single reviewable, independently-testable task.

- [ ] **Step 1: Add the end-to-end failing tests in `event.rs`**

Add two `#[case]` arms to the existing `parses_agent_event` `rstest` in `crates/atuin/src/command/client/hook/event.rs`, immediately after the `empty_command_skipped` case (around line 227):

```rust
    // A command carrying a NUL is truncated at ingest; the trailing garbage
    // (issue #3589) never reaches history.
    #[case::command_truncated_at_nul(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hi\0rm -rf /"},
            "tool_use_id": "toolu_abc123"
        }),
        Some(HookEvent::Start {
            command: "echo hi".into(),
            intent: None,
            tool_use_id: "toolu_abc123".into(),
        })
    )]
    // A command that is nothing but a NUL prefix truncates to empty → skipped.
    #[case::command_only_nul_skipped(
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "\0rm -rf /"},
            "tool_use_id": "toolu_abc123"
        }),
        None
    )]
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test -p atuin --lib command::client::hook::event::tests::parses_agent_event`
Expected: FAIL — `command_truncated_at_nul` returns `Some(Start { command: "echo hi\0rm -rf /", … })` (the NUL and trailing text are still present), and `command_only_nul_skipped` returns a `Start` instead of `None`, because the wire type still carries a plain `String`.

- [ ] **Step 3: Change the wire type to carry `CommandStr`**

Edit `crates/atuin/src/command/client/hook/wire.rs`. Add the import after the existing `use serde::Deserialize;` (line 6):

```rust
use serde::Deserialize;

use atuin_common::string::CommandStr;
```

Change the `command` field of `WireToolInput` (line 53) from `Option<String>` to `Option<CommandStr>`:

```rust
/// See [`WireHookEvent::tool_input`].
#[derive(Debug, Deserialize)]
pub struct WireToolInput {
    #[serde(default)]
    pub command: Option<CommandStr>,
    #[serde(default)]
    pub description: Option<String>,
}
```

- [ ] **Step 4: Change the event type and `From` impl**

Edit `crates/atuin/src/command/client/hook/event.rs`. Add the import alongside the existing `use super::wire::…` (line 9):

```rust
use serde_json::error::Category;

use atuin_common::string::CommandStr;

use super::wire::{HookEventName, WireHookEvent, WireToolName};
```

Change the `command` field of `HookEvent::Start` (line 35) from `String` to `CommandStr`:

```rust
/// An agent hook event Atuin cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    /// A Bash command is about to run; open a history entry.
    Start {
        command: CommandStr,
        intent: Option<String>,
        tool_use_id: String,
    },
    /// A Bash command finished; close the matching history entry.
    End { tool_use_id: String, exit: i64 },
}
```

In the `From<WireHookEvent>` impl, change the `None` arm of the `tool_input` match (line 66) so both arms produce a `CommandStr`. The `command.is_empty()` check below it (line 69) is unchanged — it reaches `str::is_empty` through `Deref`:

```rust
            HookEventName::PreToolUse => {
                let (command, intent) = match wire.tool_input {
                    Some(input) => (input.command.unwrap_or_default(), input.description),
                    None => (CommandStr::default(), None),
                };

                if command.is_empty() {
                    return None;
                }

                Some(HookEvent::Start {
                    command,
                    intent,
                    tool_use_id,
                })
            }
```

- [ ] **Step 5: Fix the `bash_pre_tool_use_yields_start` proptest expectation**

In the same file, the proptest around line 305 builds the expected `Start` with a shorthand `command` field that is now a `String` where a `CommandStr` is required. Change that one field (line 324) to convert. The strategy `r"[^\p{Cc}]+"` excludes NUL, so the value is unchanged by truncation:

```rust
            prop_assert_eq!(
                HookEvent::from_json_str(&input.to_string()).unwrap(),
                Some(HookEvent::Start { command: command.into(), intent: description, tool_use_id })
            );
```

> The existing `#[case]` arms that write `command: "echo hello".into()` need no change — `.into()` now resolves through `From<&str> for CommandStr`.

- [ ] **Step 6: Run the hook tests to verify they pass**

Run: `cargo test -p atuin --lib command::client::hook`
Expected: PASS — the two new NUL cases, every pre-existing `parses_agent_event` case, `well_formed_non_events_are_skipped`, `malformed_json_is_an_error`, and all proptests.

- [ ] **Step 7: Verify the downstream consumer still compiles (no edit expected)**

`crates/atuin/src/command/client/hook.rs:153` passes `&command` to `start_history_entry(command: &str, …)`. With `command: CommandStr`, `&command` reaches `&str` by deref coercion, so no change is needed. Confirm the whole crate builds:

Run: `cargo build -p atuin`
Expected: SUCCESS with no edits to `hook.rs`. If it fails to coerce, change the call site to `command.as_str()` — but it should not be necessary.

- [ ] **Step 8: Lint**

Run: `cargo clippy -p atuin --all-targets`
Expected: no new warnings from `wire.rs` or `event.rs`.

- [ ] **Step 9: Commit**

```bash
git add crates/atuin/src/command/client/hook/wire.rs \
        crates/atuin/src/command/client/hook/event.rs
git commit -m "fix(hook): truncate command at first NUL via CommandStr on ingress (#3589)"
```

---

## Self-Review

**1. Spec coverage:**
- "New type `CommandStr` wrapping a CStr, up to the first NUL" → Task 1 (`CommandStr(CString)`, `new` truncates at first NUL).
- "In `atuin-common`, under `string/`" → Task 1 (`src/string/command_str.rs`, re-exported from `string/mod.rs`).
- "All sane derives" → Task 1 Step 5 (`Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default`).
- "Serialize/Deserialize (perhaps behind serde feature)" → Task 1 Step 5; resolved to **unconditional** per user decision (serde is already a hard dep, no feature precedent).
- "Simple, clean, focused, minimal comments" → Task 1: doc comments + two invariant comments only.
- "Comprehensive tests" → Task 1 Steps 2/5 (7 exact cases + 6 unit tests + 4 proptests) and Task 2 (end-to-end truncation cases).
- "Use it in event.rs (or wire.rs) instead of the plain command String" → Task 2; resolved to **wire.rs boundary + carried through event.rs** per user decision.
- "Ignore strings with unnecessary NUL characters" → `truncates_at_first_nul` cases + `command_truncated_at_nul` / `command_only_nul_skipped` end-to-end cases.

**2. Placeholder scan:** No TBD/TODO/"handle edge cases" placeholders; every code and test step shows complete code and an exact command with expected output.

**3. Type consistency:** `CommandStr::new`, `as_str`, `From<&str>`/`From<String>`, `Default`, `Serialize`/`Deserialize` are defined in Task 1 and used with the same names/signatures in Task 2. `WireToolInput.command: Option<CommandStr>` and `HookEvent::Start.command: CommandStr` line up with `unwrap_or_default()` (needs `Default`) and `command.into()` / `"…".into()` (needs `From`). `command.is_empty()` and `&command` as `&str` rely on the `Deref<Target = str>` impl from Task 1.
