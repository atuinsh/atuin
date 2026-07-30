"""Layer 1 (TUI): does Ctrl-R open interactive search and filter to a match?

Records two commands, opens the atuin TUI with Ctrl-R, types a filter, and
asserts the rendered screen (via pyte) shows the matching entry. Screen-buffer
assertions are resilient to ANSI/redraw noise; timing is the fragile part, so
this is marked `tui` (deselect with `-m "not tui"`) and reruns on transient PTY
misses.
"""

import time
import uuid

import pytest

pytestmark = [
    pytest.mark.tui,
    pytest.mark.flaky(reruns=2, reruns_delay=1, only_rerun=["TIMEOUT", "EOF"]),
]

VERIFIED_SHELLS = ["zsh", "bash"]
CTRL_R = b"\x12"
ESC = b"\x1b"


@pytest.mark.parametrize("shell", VERIFIED_SHELLS)
def test_ctrl_r_filters_to_match(shell, make_client, shells):
    client = make_client(shell)
    needle = f"needle{uuid.uuid4().hex[:6]}"

    pty = shells(client, shell)
    pty.run(f"echo {needle}")
    pty.run("echo unrelated-command")

    pty.send_keys(CTRL_R)
    time.sleep(0.5)  # let the TUI draw
    pty.send_keys(needle.encode())
    time.sleep(0.5)  # let the filter settle and the list redraw

    screen = pty.screen_text()
    pty.send_keys(ESC)  # cancel the TUI before the assertion (screen already captured)
    assert needle in screen, f"{needle!r} not visible in TUI screen:\n{screen}"
