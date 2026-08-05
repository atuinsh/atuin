"""testcontainers wrappers for the atuin sync server and shell clients.

Everything runs on a shared docker network so clients reach the server by the
alias ``server``. The server is sqlite-backed and self-contained; swap
ATUIN_DB_URI for a Postgres container URI to run prod-parity.
"""

from __future__ import annotations

import subprocess
import time
import urllib.error
import urllib.request

from testcontainers.core.container import DockerContainer
from testcontainers.core.network import Network

from . import images

SERVER_PORT = 8888
SERVER_ALIAS = "server"
SYNC_ADDRESS = f"http://{SERVER_ALIAS}:{SERVER_PORT}"


def docker_available() -> bool:
    try:
        return (
            subprocess.run(
                ["docker", "info"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            ).returncode
            == 0
        )
    except FileNotFoundError:
        return False


def make_server(network: Network) -> DockerContainer:
    """A sqlite-backed atuin-server, reachable at ``http://server:8888`` on the network."""
    return (
        DockerContainer(images.BASE_IMAGE)
        .with_command(["atuin-server", "start"])
        .with_env("ATUIN_HOST", "0.0.0.0")
        .with_env("ATUIN_PORT", str(SERVER_PORT))
        .with_env("ATUIN_OPEN_REGISTRATION", "true")
        .with_env("ATUIN_DB_URI", "sqlite:///home/atuin/atuin-server.db")
        .with_env("ATUIN_CONFIG_DIR", "/home/atuin/server-config")
        .with_env("RUST_LOG", "atuin_server=info")
        .with_exposed_ports(SERVER_PORT)
        .with_network(network)
        .with_network_aliases(SERVER_ALIAS)
    )


def make_client(shell_image: str, network: Network) -> DockerContainer:
    """A long-lived shell-client container. Capture is local; sync targets the server alias."""
    return (
        DockerContainer(shell_image)
        .with_env("ATUIN_SYNC_ADDRESS", SYNC_ADDRESS)
        .with_env("ATUIN_AUTO_SYNC", "false")
        .with_network(network)
    )


def wait_healthy(server: DockerContainer, timeout: float = 90.0) -> None:
    """Block until the server answers /healthz on its published port."""
    host = server.get_container_host_ip()
    port = server.get_exposed_port(SERVER_PORT)
    url = f"http://{host}:{port}/healthz"
    deadline = time.time() + timeout
    last_err: Exception | None = None
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2) as resp:
                if resp.status == 200:
                    return
        except (urllib.error.URLError, ConnectionError, OSError) as err:
            last_err = err
        time.sleep(0.5)
    raise TimeoutError(f"server not healthy at {url} within {timeout}s: {last_err}")
