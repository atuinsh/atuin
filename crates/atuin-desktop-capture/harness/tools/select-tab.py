"""Selects the tab whose title contains a text, through its accessibility action.

Usage: select-tab.py <application> <text in the tab title>

Works the same in every session, because it doesn't need a keyboard: KWin doesn't offer a
virtual keyboard to clients, and tabs have an action that selects them."""
import sys
import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

app_name, wanted = sys.argv[1].lower(), sys.argv[2].lower()


def tabs(node, depth=0):
    if depth > 30:
        return
    try:
        role = node.get_role()
    except Exception:
        return
    if role == Atspi.Role.PAGE_TAB:
        yield node
        return
    if role in (Atspi.Role.DOCUMENT_WEB, Atspi.Role.DOCUMENT_FRAME):
        return
    for i in range(node.get_child_count()):
        child = node.get_child_at_index(i)
        if child is not None:
            yield from tabs(child, depth + 1)


desktop = Atspi.get_desktop(0)
for i in range(desktop.get_child_count()):
    app = desktop.get_child_at_index(i)
    if app is None or app_name not in (app.get_name() or "").lower():
        continue
    for tab in tabs(app):
        if wanted in (tab.get_name() or "").lower():
            action = tab.get_action_iface()
            if action is not None and action.get_n_actions() > 0:
                action.do_action(0)
                sys.exit(0)
print(f"no tab of {sys.argv[1]} is named like {sys.argv[2]!r}", file=sys.stderr)
sys.exit(1)
