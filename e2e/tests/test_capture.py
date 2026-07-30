"""Layer 1: does running a command in a real shell get captured by atuin?

Drives an interactive shell over a PTY (hooks active), runs a command, then
asserts via `atuin search` that it was recorded.
"""

import uuid

import pytest

from harness import atuin

# Retry only transient pexpect PTY misses (TIMEOUT/EOF), never assertion failures --
# a real capture bug still fails fast.
pytestmark = pytest.mark.flaky(reruns=2, reruns_delay=1, only_rerun=["TIMEOUT", "EOF"])

# bash + zsh are the verified baseline. fish + nu images/adapters exist but their
# hook + prompt handling still needs tuning -- add them here as they stabilise.
VERIFIED_SHELLS = ["zsh", "bash"]


@pytest.mark.parametrize("shell", VERIFIED_SHELLS)
def test_command_is_captured(shell, make_client, shells):
    client = make_client(shell)
    marker = f"echo capture-{uuid.uuid4().hex[:8]}"

    shells(client, shell).run(marker)

    recorded = atuin.search_cmd_only(client, marker.split()[1])
    assert any(marker in line for line in recorded), (
        f"{marker!r} not found in captured history: {recorded}"
    )
