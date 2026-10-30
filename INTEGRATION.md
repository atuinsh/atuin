# Wiring `mkt/kiro-hooks-mcp` into the `Kiro` harness

This branch adds standalone functions and no enum variants. Once `AnyHarness::Kiro` and
`kiro::kiro_home()` exist (the reader branch), add these arms and delete this file.

## `crates/atuin-common/src/harnesstools/kiro/mod.rs`

Replace the stub module with the reader's, keeping `pub mod hooks;` (drop the
`#[allow(dead_code)]` on it). In `impl Harness for Kiro`:

```rust
/// Kiro loads every `*.json` in `$KIRO_HOME/hooks` when a session starts (see `hooks`).
async fn install_hooks(&self) -> Result<PathBuf, InstallHookError> {
    hooks::install(&kiro_home(), &std::env::current_exe()?).await
}
```

`hooks::install` hard-codes the harness name `kiro` in the command it writes
(`hooks::HARNESS`); it must equal `Kiro::name()`.

## `crates/atuin-common/src/harnesstools/mcp.rs`

- `config_path`: add `AnyHarness::Kiro(_) => kiro_config_path(&kiro::kiro_home()),`
- `install`: add `AnyHarness::Kiro(_) => install_kiro(&config_path(harness), atuin).await,`
- Remove the `#[allow(dead_code)]` and `// TODO(kiro)` lines above `kiro_config_path` and
  `install_kiro`; both can become private `fn`.
- Tests: add `#[case("kiro", ".kiro/settings/mcp.json")]` to `detects_existing_config_files`
  and `#[case("kiro", "KIRO_HOME", "settings/mcp.json")]` to
  `detection_respects_config_overrides`. The override case needs `kiro_home()` to accept the
  test's `KIRO_HOME` (an absolute temp dir, not the mocked `HOME`), which Kiro's own rule does.

## Nothing to add elsewhere on this branch

- `atuin hook kiro` and `atuin hook install kiro` dispatch through `AnyHarness::from_name`, so
  the CLI needs no change. `hook.rs`'s `every_harness_author_is_a_known_agent` then needs
  `"kiro"` in `atuin-client`'s `KNOWN_AGENTS` (the reader branch adds it).
- The hook wire (`crates/atuin/src/command/client/hook/{wire,event}.rs`) is harness-agnostic:
  `execute_bash` / `execute_pwsh` alias the shell tool, `run_in_background` skips like Copilot's
  `mode: async`, and a bare-string `tool_response` yields the exit code on its last line.
- The e2e branch's fake Kiro should send `tool_name: "execute_bash"` and a `tool_response` of
  the form `"Output:\n<out>\n\n\nExit Code: <n>"` with no `tool_use_id`.

## Decisions worth a second look

- A call Kiro reports with `Exit Code: -1` (denied, or its permission request failed) is
  recorded with exit -1, closing the entry its `PreToolUse` opened. Dropping the end instead
  would leave the entry open forever. Whether Kiro sends `PostToolUse` at all for a denied call
  was not observed; the `-1` shape comes from `tool_result` rows in its transcript.
- A Codex `tool_response` whose last line happens to be `Exit Code: <n>` now reads as that exit
  instead of falling through to the transcript lookup. Only a command that prints such a line
  itself hits this.
- The docs say `atuin mcp install` (no agent) detects Kiro and that `kiro` is a recognized
  author name. Both hold only after the arms above and `KNOWN_AGENTS` land.
