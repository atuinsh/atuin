"""Shared fixtures for the atuin E2E suite.

Fixture graph (session-scoped unless noted):
  docker_ready -> skip everything if there's no docker daemon
  network      -> a shared docker network (clients reach the server by alias)
  base_image   -> builds atuin + atuin-server once
  shell_image  -> per-shell image factory (lazy, cached)
  server       -> a running sqlite-backed atuin-server
  make_client  -> (function-scoped) factory for shell-client containers

Set ATUIN_E2E_KEEP=1 to leave containers running after the suite for debugging.
"""

from __future__ import annotations

import os

import pytest

from harness import containers, images


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
def make_client(network, shell_image):
    """Factory: shell name -> a started client container. Torn down after the test."""
    started = []

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
