"""Layer 1: does running a command in a real shell get captured by atuin?

Drives an interactive shell over a PTY (hooks active), runs a command, then
asserts via `atuin search` that it was recorded.
"""

import uuid

import pytest

from harness import atuin

# bash + zsh are the verified baseline. fish + nu images/adapters exist but their
# hook + prompt handling still needs tuning -- add them here as they stabilise.
VERIFIED_SHELLS = ["zsh", "bash"]


@pytest.mark.parametrize("shell", VERIFIED_SHELLS)
def test_command_is_captured(shell, make_client):
    client = make_client(shell)
    marker = f"echo capture-{uuid.uuid4().hex[:8]}"

    pty = atuin.open_shell(client, shell)
    try:
        pty.run(marker)
    finally:
        pty.close()

    recorded = atuin.search_cmd_only(client, marker.split()[1])
    assert any(marker in line for line in recorded), (
        f"{marker!r} not found in captured history: {recorded}"
    )
