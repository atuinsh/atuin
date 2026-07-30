"""Layer 2: does history sync end-to-end between two clients via a real server?

Client A registers, records a command through its shell hooks, and syncs. Client
B logs into the same account with A's key and syncs. B must then see A's command.
This is the encrypted client<->server round-trip, through the real CLI.
"""

import uuid

import pytest

from harness import atuin


@pytest.mark.sync
def test_history_syncs_between_two_clients(server, make_client):
    shell = "zsh"
    client_a = make_client(shell)
    client_b = make_client(shell)

    suffix = uuid.uuid4().hex[:8]
    username = f"user{suffix}"
    password = uuid.uuid4().hex
    email = f"{username}@example.com"
    marker = f"echo synced-{suffix}"

    # A: register, record a command via real hooks, push.
    atuin.register(client_a, username, email, password)
    key = atuin.read_key(client_a)
    pty = atuin.open_shell(client_a, shell)
    try:
        pty.run(marker)
    finally:
        pty.close()
    atuin.sync(client_a)

    # B: log into the same account with A's key, pull.
    atuin.login(client_b, username, password, key)
    atuin.sync(client_b)

    received = atuin.search_cmd_only(client_b, f"synced-{suffix}")
    assert any(marker in line for line in received), (
        f"{marker!r} did not sync to client B: {received}"
    )
