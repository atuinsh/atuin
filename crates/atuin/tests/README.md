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
