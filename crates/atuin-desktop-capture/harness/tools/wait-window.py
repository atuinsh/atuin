"""Waits until an application whose name contains a text shows a window.

Usage: wait-window.py <application> <seconds>

Exits with 1 when the time runs out. A test that closes a dialog a moment after it appears
must count from its appearance, because a program can take seconds to start."""
import sys
import time
import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

wanted, deadline = sys.argv[1].lower(), time.time() + float(sys.argv[2])
while time.time() < deadline:
    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(i)
        if app is None or wanted not in (app.get_name() or "").lower():
            continue
        for j in range(app.get_child_count()):
            window = app.get_child_at_index(j)
            if window is not None and window.get_state_set().contains(Atspi.StateType.SHOWING):
                sys.exit(0)
    time.sleep(0.02)
sys.exit(1)
