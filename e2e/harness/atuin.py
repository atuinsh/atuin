"""High-level atuin actions against a client container, plus per-shell adapters.

Interactive work (capture, TUI) goes through :class:`~harness.session.PtyShell`.
One-shot commands (register, login, sync, search) run non-interactively via
``docker exec`` and return parsed output -- more deterministic than scraping the
interactive shell.
"""

from __future__ import annotations

from testcontainers.core.container import DockerContainer

from .session import PtyShell

# Per-shell: how to start an interactive session, the `atuin init` keyword, and
# the prompt sentinel baked into that shell's rc (see images/Dockerfile.<shell>).
SHELLS = {
    "zsh": {"argv": ["zsh", "-i"], "init": "zsh"},
    "bash": {"argv": ["bash", "-i"], "init": "bash"},
    "fish": {"argv": ["fish", "-i"], "init": "fish"},
    "nu": {"argv": ["nu"], "init": "nu"},
}
PROMPT = b"ATUIN_E2E> "

# atuin's current_context() requires $ATUIN_SESSION for any command that touches
# history (search, sync, ...). In a hooked interactive shell `atuin init` sets it;
# a headless `docker exec` has to supply one. The value only matters for
# session-scoped filtering, which we avoid by searching with --filter-mode global.
_SESSION_ENV = {"ATUIN_SESSION": "00000000000000000000000000000e2e"}


def open_shell(container: DockerContainer, shell: str, **kwargs) -> PtyShell:
    spec = SHELLS[shell]
    container_id = container.get_wrapped_container().id
    return PtyShell(container_id, spec["argv"], PROMPT, **kwargs)


def _exec(container: DockerContainer, cmd: list[str]) -> tuple[int, str, str]:
    res = container.get_wrapped_container().exec_run(cmd, demux=True, environment=_SESSION_ENV)
    out, err = res.output
    return res.exit_code, (out or b"").decode(), (err or b"").decode()


def _atuin(container: DockerContainer, args: list[str]) -> tuple[int, str, str]:
    return _exec(container, ["atuin", *args])


def register(container: DockerContainer, username: str, email: str, password: str) -> None:
    code, out, err = _atuin(container, ["register", "-u", username, "-e", email, "-p", password])
    assert code == 0, f"atuin register failed: {err or out}"


def login(container: DockerContainer, username: str, password: str, key: str) -> None:
    code, out, err = _atuin(container, ["login", "-u", username, "-p", password, "-k", key])
    assert code == 0, f"atuin login failed: {err or out}"


def sync(container: DockerContainer, force: bool = True) -> None:
    args = ["sync"] + (["-f"] if force else [])
    code, out, err = _atuin(container, args)
    assert code == 0, f"atuin sync failed: {err or out}"


def read_key(container: DockerContainer) -> str:
    """Return the base64 encryption key from the client's data dir.

    There is no `atuin key` command in this version; the key lives in a file that
    `atuin login -k` accepts verbatim, so we read it directly to share between clients.
    """
    code, out, err = _exec(
        container,
        ["sh", "-c", 'cat "${ATUIN_DATA_DIR:-$HOME/.local/share/atuin}/key"'],
    )
    assert code == 0 and out.strip(), f"could not read key: {err or out}"
    return out.strip()


def search_cmd_only(container: DockerContainer, *query: str, limit: int | None = None) -> list[str]:
    args = ["search", "--cmd-only", "--filter-mode", "global"]
    if limit is not None:
        args += ["--limit", str(limit)]
    args += list(query)
    code, out, err = _atuin(container, args)
    assert code == 0, f"atuin search failed: {err or out}"
    return [line for line in out.splitlines() if line]
