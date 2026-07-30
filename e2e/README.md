# atuin end-to-end tests

Full-stack tests that exercise atuin the way a user does: **real shells with
hooks installed, driven through a PTY, plus multi-client sync through a real
server** — the surface the Rust unit/integration tests can't reach.

Everything runs in containers via [`testcontainers`](https://testcontainers.com/):
the `atuin` client + daemon and the `atuin-server` are compiled from the repo
source into a base image, per-shell images layer a shell on top, `pexpect` drives
each shell over `docker exec -t`, and [`pyte`](https://github.com/selectel/pyte)
parses the rendered screen so assertions are "what the user sees".

## Requirements

- A running Docker daemon (the tests build images and start containers).
- [`uv`](https://docs.astral.sh/uv/).

## Run

```sh
cd e2e
uv sync
uv run pytest                 # everything
uv run pytest -m "not tui"    # skip the timing-sensitive TUI tests
uv run pytest tests/test_sync.py -v
uv run pytest --html=report.html --self-contained-html   # debuggable report
```

Tests carry a per-body timeout and retry only on transient PTY misses
(`pexpect` TIMEOUT/EOF), never on real assertion failures. On any failure the
rendered screen and container logs are printed and attached to the HTML report.
See [`DESIGN.md`](./DESIGN.md#tooling-pytest-plugins) for the plugin set.

The first run builds the base image (compiles atuin — a few minutes). It's
cached afterwards. Force a rebuild after changing atuin source:

```sh
ATUIN_E2E_REBUILD=1 uv run pytest
```

Leave containers running after the suite to poke at them:

```sh
ATUIN_E2E_KEEP=1 uv run pytest tests/test_capture.py
```

## Layout

```
harness/
  images.py      build the base + per-shell images (from repo source)
  containers.py  testcontainers wrappers: server + shell clients on a network
  session.py     PtyShell: pexpect over `docker exec -t` + pyte screen parsing
  atuin.py       high-level actions (register/login/sync/search) + shell adapters
images/          Dockerfile.base (+ .zsh/.bash/.fish/.nu)
tests/           test_capture.py, test_search_tui.py, test_sync.py
conftest.py      fixtures (network, base image, server, client factory)
```

## Shell coverage

`zsh` and `bash` are the verified baseline. `fish` and `nu` have images and
adapters but their hook/prompt handling still needs tuning — see
`VERIFIED_SHELLS` in the test files. The server defaults to sqlite; point
`ATUIN_DB_URI` at a Postgres container for prod-parity.

## Status

This is an initial foundation (a walking skeleton + structure to grow into).
See [`DESIGN.md`](./DESIGN.md) for the full design and the staged roadmap.
