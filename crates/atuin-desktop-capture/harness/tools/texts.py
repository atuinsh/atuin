"""Prints the names and texts of the showing elements of an application, one per line.

Usage: texts.py <application>"""
import sys
import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

wanted = sys.argv[1].lower()


def walk(node, depth=0):
    if depth > 40:
        return
    try:
        if not node.get_state_set().contains(Atspi.StateType.SHOWING) and depth > 1:
            return
        name = node.get_name() or ""
        text = node.get_text_iface()
        content = text.get_text(0, text.get_character_count()) if text else ""
        if name or content:
            print(f"{'  ' * min(depth, 12)}{node.get_role_name()}: {name!r} {content!r}")
        for i in range(node.get_child_count()):
            child = node.get_child_at_index(i)
            if child is not None:
                walk(child, depth + 1)
    except Exception:
        return


desktop = Atspi.get_desktop(0)
for i in range(desktop.get_child_count()):
    app = desktop.get_child_at_index(i)
    if app is not None and wanted in (app.get_name() or "").lower():
        walk(app)
