# End-to-end tests

Run the installed binary in a temporary home:

```sh
ATUIN_E2E_REQUIRE_SHELLS=1 cargo nextest run -p atuin --test 'e2e_*'
```

CI runs Linux and macOS, with separate macOS jobs for Apple and Homebrew Bash.
Missing shells or prerequisite files fail when `ATUIN_E2E_REQUIRE_SHELLS` is set;
otherwise the affected cases print a skip message and return.

## Add a shell setup

Drop a TOML file into `tests/shells/`. Every PTY test discovers `shells/*.toml`
through `rstest`; there is no Rust list to update. For example:

```toml
shell = "zsh"
rc = ".zshrc"
script = '''
bindkey -v
eval "$(atuin init zsh)"
PROMPT="E2E_PROMPT> "
'''
```

- `shell`: executable to find on PATH. `ATUIN_E2E_<SHELL>` overrides it, e.g.
  `ATUIN_E2E_BASH=/bin/bash`.
- `args`: optional shell arguments, such as `["-i"]`.
- `rc`: path relative to the temporary home.
- `script`: the complete rc file. Set the prompt to `E2E_PROMPT> `, without a
  right prompt. The tests use the common command syntax supported by bash, zsh,
  and fish.
- `multiline_accept`: bytes to run an inserted multiline command. Defaults to
  `"\r"`; ble.sh uses `"\n"` (Ctrl-J).
- `required_files`: optional environment-variable/path pairs. Existing values
  override the paths; `$HOME` in a default path means the caller's home. The
  resolved values are passed to the shell. See `shells/bash-blesh.toml`.

Each case gets its own home, databases, and daemon socket. PTY input waits for
rendered characters; command completion waits for an empty prompt and terminal
input mode. Polling
for history persistence and daemon health has deadlines; there are no test
retries or fixed startup sleeps.

`e2e_smoke` covers CLI startup and keys. `e2e_pty` covers recording, search,
selection, cancellation, quoting, resize, and filter switching. `e2e_daemon`
covers foreground startup, automatic startup with concurrent writers, history
persistence, shutdown, and startup again.
