"""Shared fixtures for the atuin E2E suite.

Fixture graph (session-scoped unless noted):
  docker_ready -> skip everything if there's no docker daemon
  network      -> a shared docker network (clients reach the server by alias)
  base_image   -> builds atuin + atuin-server once
  shell_image  -> per-shell image factory (lazy, cached)
  server       -> a running sqlite-backed atuin-server
  make_client  -> (function-scoped) factory for shell-client containers
  shells       -> (function-scoped) factory that opens + tracks PTY sessions

On failure, the pytest_runtest_makereport hook attaches each test's rendered
screen(s) and container logs to the report (and prints them) -- so an opaque red
run in CI is actually debuggable.

Set ATUIN_E2E_KEEP=1 to leave containers running after the suite for debugging.
"""

from __future__ import annotations

import os

import pytest

from harness import atuin, containers, images


def _keep() -> bool:
    return os.environ.get("ATUIN_E2E_KEEP") == "1"


@pytest.fixture(scope="session")
def docker_ready() -> None:
    if not containers.docker_available():
        pytest.skip("docker daemon not available")


@pytest.fixture(scope="session")
def network(docker_ready):
    from testcontainers.core.network import Network

    net = Network()
    net.create()
    try:
        yield net
    finally:
        if not _keep():
            net.remove()


@pytest.fixture(scope="session")
def base_image(docker_ready) -> str:
    return images.ensure_base()


@pytest.fixture(scope="session")
def shell_image(base_image):
    """Factory: shell name -> built image tag, built once per shell."""
    built: dict[str, str] = {}

    def _get(shell: str) -> str:
        if shell not in built:
            built[shell] = images.ensure_shell(shell)
        return built[shell]

    return _get


@pytest.fixture(scope="session")
def server(base_image, network):
    container = containers.make_server(network)
    container.start()
    try:
        containers.wait_healthy(container)
        yield container
    finally:
        if not _keep():
            container.stop()


@pytest.fixture
def make_client(network, shell_image, request):
    """Factory: shell name -> a started client container. Torn down after the test."""
    started = []
    request.node.stash[_CONTAINERS] = started

    def _make(shell: str):
        container = containers.make_client(shell_image(shell), network)
        container.start()
        started.append(container)
        return container

    try:
        yield _make
    finally:
        if not _keep():
            for container in started:
                container.stop()


@pytest.fixture
def shells(request):
    """Factory: (container, shell) -> a PtyShell. Tracked for failure dumps, auto-closed."""
    opened = []
    request.node.stash[_PTYS] = opened

    def _open(container, shell: str, **kwargs):
        pty = atuin.open_shell(container, shell, **kwargs)
        opened.append(pty)
        return pty

    try:
        yield _open
    finally:
        for pty in opened:
            pty.close()


# --- failure context: attach rendered screens + container logs to the report ---

_PTYS = pytest.StashKey[list]()
_CONTAINERS = pytest.StashKey[list]()


def _failure_blocks(item) -> list[tuple[str, str]]:
    blocks: list[tuple[str, str]] = []
    for i, pty in enumerate(item.stash.get(_PTYS, [])):
        try:
            blocks.append((f"screen[{i}]", pty.screen_text()))
        except Exception as err:  # never let diagnostics mask the real failure
            blocks.append((f"screen[{i}]", f"<unavailable: {err}>"))

    clients = list(item.stash.get(_CONTAINERS, []))
    server = (item.funcargs or {}).get("server")
    for container in clients + ([server] if server is not None else []):
        try:
            wrapped = container.get_wrapped_container()
            logs = wrapped.logs().decode("utf-8", "replace")
            blocks.append((f"logs {wrapped.name}", logs[-4000:]))
        except Exception as err:
            blocks.append(("logs", f"<unavailable: {err}>"))
    return blocks


@pytest.hookimpl(wrapper=True)
def pytest_runtest_makereport(item, call):
    report = yield
    if report.when in ("setup", "call") and report.failed:
        blocks = _failure_blocks(item)
        for name, body in blocks:
            print(f"\n----- {name} -----\n{body}")
        try:
            import pytest_html

            extras = getattr(report, "extras", [])
            extras.extend(pytest_html.extras.text(body, name=name) for name, body in blocks)
            report.extras = extras
        except ImportError:
            pass
    return report
