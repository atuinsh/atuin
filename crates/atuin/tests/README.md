# End-to-end tests

Tests run the Cargo-built binary in temporary homes with private daemon sockets.

```sh
ATUIN_E2E_REQUIRE_SHELLS=1 cargo nextest run -p atuin --test 'e2e_*'
```

Install Bash, Zsh, Fish, and ble.sh first. Missing dependencies fail with
`ATUIN_E2E_REQUIRE_SHELLS=1`; otherwise their cases skip. CI runs on Linux and
macOS, with Homebrew Bash on macOS.

- `e2e_fresh_install`: CLI startup, keys, shell init, and doctor.
- `e2e_pty`: shell hooks, search, selection, quoting, resize, and filters.
- `e2e_daemon`: startup, concurrent writers, persistence, and restart.
- `e2e_ai_sessions`: AI agent sessions captured from each agent's transcripts,
  synced between two machines through an in-process sync server, and resumed:
  restored, caught up, forked, switched, or continued in another agent.
- `e2e_ai_resume_picker`: `atuin ai resume`'s picker and chooser on a PTY, and
  through each shell's widget.

The AI session tests write transcripts the way Claude Code, Codex, opencode and
Pi do (`common/agents.rs`), and put stand-ins for those agents on `PATH` that
print how they were run. Set `ATUIN_E2E_LOG` (an `ATUIN_LOG` filter) to have
their daemons log more for a failure to show.

## Add a shell setup

Copy a file in [shells/](shells/). `rstest` runs every PTY test against each
`shells/*.toml` file.

- `shell`: executable on PATH; override with `ATUIN_E2E_<SHELL>`.
- `args`: optional shell arguments.
- `rc`: path relative to the temporary home.
- `script`: rc contents. Set the prompt to `E2E_PROMPT> ` with no right prompt.
- `multiline_accept`: key to execute multiline input. Default: `"\r"`; ble.sh: `"\n"`.
- `required_files`: environment variables and default file paths. Caller values
  override defaults; `$HOME` expands to the caller's home. Resolved paths are
  passed to the shell. See [bash-blesh.toml](shells/bash-blesh.toml).
