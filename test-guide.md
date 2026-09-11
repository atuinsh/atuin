# Test Zsh HISTFILE selection for issue #2593

This branch fixes the guidance and adds regression tests. It does **not** change
which history file the Rust importer selects. An exported `HISTFILE` already
works; a Zsh parameter that hasn't been exported isn't visible to Atuin.

The before/after comparison below is therefore **bare import versus explicit
HISTFILE forwarding**, using binaries built from the original revision and this
branch. The corrected command also works with the original binary. A bare import
on this branch still reproduces the original symptom.

All examples use synthetic commands and temporary homes. Each case starts with a
fresh Atuin database, so an earlier import can't make a later test appear to pass.
You don't need to log in, sync, or install Atuin's shell integration.

## 0. Start a disposable Docker container (recommended)

Run this on your machine with Docker running. Use `--rm` (two dashes):

```bash
docker run --rm -it --name atuin-2593-test rust:1.98.0-bookworm bash
```

The container has no host directory mounts or forwarded credentials. The following
commands run **inside the container's Bash shell**. The first build downloads
Rust dependencies and can take several minutes and several GB of disk space.

```bash
set -e
apt-get update
apt-get install -y --no-install-recommends ca-certificates git zsh pkg-config libssl-dev

git clone --branch fix/zsh-import-histfile-guidance --single-branch \
  https://github.com/jamiechicago312/atuin.git /tmp/atuin-src
cd /tmp/atuin-src

# Build the original revision, before this branch's changes.
git worktree add --detach /tmp/atuin-before-src \
  c0c717ab04c881764bcad4b3d169a507e2432643
export CARGO_TARGET_DIR=/tmp/atuin-target
cargo build --locked --manifest-path /tmp/atuin-before-src/Cargo.toml \
  -p atuin --no-default-features --features client
cp /tmp/atuin-target/debug/atuin /tmp/atuin-before

# Build the branch with the documentation fix and regression tests.
cargo build --locked -p atuin --no-default-features --features client
cp /tmp/atuin-target/debug/atuin /tmp/atuin-after
```

This builds the client needed for importing and listing history. Some existing
unused-variable warnings can appear with this reduced feature set.

Create a helper in the same Bash session:

```bash
run_case() (
  set -eu
  atuin_case_home=$(mktemp -d /tmp/atuin-2593-case.XXXXXX)
  trap 'rm -rf -- "$atuin_case_home"' EXIT
  mkdir -p "$atuin_case_home/bin" "$atuin_case_home/.config/atuin" \
    "$atuin_case_home/.local/share" "$atuin_case_home/.cache"
  ln -s "$1" "$atuin_case_home/bin/atuin"
  printf '%s\n' 'echo LEGACY_2593' > "$atuin_case_home/.zhistory"
  printf '%s\n' 'echo EXPECTED_2593' > "$atuin_case_home/.zsh_history"
  printf '%s\n' 'echo SPACE_2593' > "$atuin_case_home/history with spaces"
  printf '%s\n' 'auto_sync = false' 'update_check = false' \
    '[daemon]' 'enabled = false' > "$atuin_case_home/.config/atuin/config.toml"
  cd "$atuin_case_home"
  env -i HOME="$atuin_case_home" \
    PATH="$atuin_case_home/bin:/usr/bin:/bin" \
    XDG_CONFIG_HOME="$atuin_case_home/.config" \
    XDG_DATA_HOME="$atuin_case_home/.local/share" \
    XDG_CACHE_HOME="$atuin_case_home/.cache" \
    ATUIN_SESSION=b9c063b7b7204f81a50e3e0d51031f01 \
    SHELL=/usr/bin/zsh TERM=dumb LANG=C.UTF-8 \
    /usr/bin/zsh -f -e
)
```

The helper starts Zsh without user startup files and supplies a clean process
environment. It creates both fallback files, passes the selected test binary as
`atuin`, supplies a synthetic session ID for history listing, and deletes the
temporary home on return, including on test failure.
The assertions below stop the case if the result differs from the expectation.

## 1. Reproduce the original symptom

```bash
run_case /tmp/atuin-before <<'ZSH'
HISTFILE="$HOME/.zsh_history"
print -r -- "Zsh parameter: $HISTFILE"
if printenv HISTFILE; then
  print -u2 'Unexpected: HISTFILE was exported'
  exit 1
fi
atuin import zsh
actual=$(atuin history list --cmd-only)
print -r -- "Imported: $actual"
[[ "$actual" == 'echo LEGACY_2593' ]]
print 'PASS: reproduced legacy fallback selection'
ZSH
```

Expected: Zsh knows `.zsh_history`, but `printenv` finds no `HISTFILE` in the child
environment. Atuin imports only `echo LEGACY_2593` from `.zhistory`, which comes
first in the fallback list. This explains how the issue can happen without the
importer ignoring an exported variable.

## 2. Use the corrected command with this branch

```bash
run_case /tmp/atuin-after <<'ZSH'
HISTFILE="$HOME/.zsh_history"
HISTFILE="${HISTFILE:?Set HISTFILE to your Zsh history file}" atuin import zsh
actual=$(atuin history list --cmd-only)
print -r -- "Imported: $actual"
[[ "$actual" == 'echo EXPECTED_2593' ]]
if printenv HISTFILE; then
  print -u2 'Unexpected: the command permanently exported HISTFILE'
  exit 1
fi
print 'PASS: selected the current Zsh history file'
ZSH
```

Expected: only `echo EXPECTED_2593`. The assignment before `atuin` exports the
parameter for that command. It doesn't change the export state of your shell
parameter. `.zhistory` still exists but isn't selected.

For additional controls, rerun section 1 with `/tmp/atuin-after`: it should still
select the legacy file. Rerun section 2 with `/tmp/atuin-before`: it should select
the expected file. These controls confirm that the improvement is correct usage
and documentation, not a new ability to inspect the parent shell.

## 3. Edge case: History path contains spaces

```bash
run_case /tmp/atuin-after <<'ZSH'
HISTFILE="$HOME/history with spaces"
HISTFILE="${HISTFILE:?Set HISTFILE to your Zsh history file}" atuin import zsh
actual=$(atuin history list --cmd-only)
print -r -- "Imported: $actual"
[[ "$actual" == 'echo SPACE_2593' ]]
print 'PASS: a quoted path with spaces works'
ZSH
```

Expected: only `echo SPACE_2593`. Keep the quotes around the parameter expansion.

## 4. Edge case: The explicitly selected file doesn't exist

```bash
run_case /tmp/atuin-after <<'ZSH'
HISTFILE="$HOME/missing-history"
if HISTFILE="${HISTFILE:?Set HISTFILE to your Zsh history file}" atuin import zsh; then
  print -u2 'FAIL: importing a missing file unexpectedly succeeded'
  exit 1
else
  result=$?
  print -r -- "Import exit status: $result"
fi
actual=$(atuin history list --cmd-only)
[[ -z "$actual" ]]
print 'PASS: missing explicit file fails without importing a fallback'
ZSH
```

Expected: a nonzero import status and an error naming `missing-history`. No
history is imported, even though both fallback files exist. This preserves the
existing contract for an explicit `HISTFILE`.

The `:?` guard also rejects an unset or empty parameter before Atuin starts.
A nonempty path to a missing file instead reaches Atuin and fails as shown above.

## 5. Run the regression tests

From `/tmp/atuin-src` inside the container:

```bash
cargo test --locked -p atuin-client import::zsh::test::path_selection
cargo test --locked -p atuin-client import::
```

The new cases check all three fallback priorities, an exported custom path with
spaces, an exported missing file, and absence of any history file. They test path
selection directly in child processes without changing the test runner's environment.

## 6. Reset and cleanup

Every `run_case` deletes its own temporary home and database. To repeat a case,
just run its block again; it starts fresh. If an assertion fails, the outer
`set -e` can end the container session. Restart from section 0 in that case.

When finished, exit the container:

```bash
exit
```

The Docker [`--rm` option](https://docs.docker.com/reference/cli/docker/container/run/#clean-up---rm)
removes the container and its writable filesystem on exit, including the clone,
builds, and test data. Your real Zsh configuration, history files, and Atuin
databases were never mounted, so they need no reset. The downloaded Rust image
remains cached by Docker.

If the session is stuck, run this from another terminal on your machine:

```bash
docker stop atuin-2593-test
```

Optional: remove the cached image if you don't need it for other containers:

```bash
docker image rm rust:1.98.0-bookworm
```

Avoid broad Docker prune commands; they can remove unrelated resources.

## Without Docker

You can use the same `run_case` helper on Linux with Zsh installed. Build the two
binaries in a separate checkout using Rust 1.98.0 and pass their **absolute paths**
in place of `/tmp/atuin-before` and `/tmp/atuin-after`. Skip the container's
`apt-get`, fixed `/tmp` checkout setup, and Docker cleanup commands.

The helper's `env -i` and temporary homes isolate your real Atuin data and shell
configuration. No shell integration is sourced. Delete only the checkout/build
artifacts you created when finished; the test homes clean themselves up.

## Validation of this guide

The four test blocks and both before/after controls were run against the real
CLI on Linux with Zsh 5.9 using isolated temporary homes. Docker itself wasn't
available in the authoring workspace, so the outer Docker launch/build workflow
still needs a run on your machine. The guide doesn't claim that a `HISTFILE` parameter that hasn't been exported
is now automatically detected.
