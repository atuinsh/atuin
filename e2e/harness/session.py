"""Drive an interactive shell inside a container over a PTY.

``pexpect`` spawns ``docker exec -it <cid> <shell>`` so we get a real terminal
into the container. Everything the shell emits is mirrored into a ``pyte`` screen
so tests can assert on *what the user would see* (``display()``) rather than raw
ANSI. Synchronisation is on a fixed prompt sentinel baked into each shell's rc.
"""

from __future__ import annotations

import pexpect
import pyte


class _PyteFeeder:
    """A pexpect ``logfile_read`` sink that mirrors output into a pyte screen."""

    def __init__(self, stream: pyte.Stream) -> None:
        self._stream = stream

    def write(self, data) -> None:
        if isinstance(data, str):
            data = data.encode("utf-8", "replace")
        self._stream.feed(data)

    def flush(self) -> None:  # noqa: D102 - pexpect calls this
        pass


class PtyShell:
    def __init__(
        self,
        container_id: str,
        argv: list[str],
        prompt: bytes,
        cols: int = 120,
        rows: int = 40,
        timeout: int = 30,
    ) -> None:
        self.prompt = prompt
        self.timeout = timeout
        self.screen = pyte.Screen(cols, rows)
        stream = pyte.ByteStream(self.screen)
        self.child = pexpect.spawn(
            "docker",
            ["exec", "-i", "-t", container_id, *argv],
            encoding=None,  # bytes mode: robust against partial multibyte reads
            dimensions=(rows, cols),
            timeout=timeout,
        )
        self.child.logfile_read = _PyteFeeder(stream)
        self.wait_prompt()

    def wait_prompt(self, timeout: int | None = None) -> None:
        self.child.expect_exact(self.prompt, timeout=timeout or self.timeout)

    def run(self, command: str, timeout: int | None = None) -> None:
        """Type a command, press enter, and wait for the prompt to return."""
        self.child.send(command.encode() + b"\n")
        self.wait_prompt(timeout=timeout)

    def send_keys(self, data: bytes) -> None:
        """Send raw bytes (e.g. b'\\x12' for Ctrl-R) without waiting for a prompt."""
        self.child.send(data)

    def expect(self, pattern, timeout: int | None = None) -> None:
        self.child.expect_exact(
            pattern.encode() if isinstance(pattern, str) else pattern,
            timeout=timeout or self.timeout,
        )

    def display(self) -> list[str]:
        """The current rendered screen, one string per row, trailing space stripped."""
        return [line.rstrip() for line in self.screen.display]

    def screen_text(self) -> str:
        return "\n".join(self.display())

    def close(self) -> None:
        try:
            self.child.sendcontrol("d")
        except Exception:
            pass
        self.child.close(force=True)
