"""Prints the accessibility events of an application with absolute times, for timelines.

Usage: timeline.py <application> <seconds>

Prints the window, state, focus, and structure events whose source belongs to an application
whose name contains the text, and the registry's announcements of applications that join."""
import sys
import time
import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib

wanted = sys.argv[1].lower()


def describe(node):
    try:
        return f"{node.get_role_name()}:{(node.get_name() or '')[:30]!r}"
    except Exception:
        return "?"


def on(event):
    try:
        app = event.source.get_application()
        app_name = (app.get_name() or "") if app is not None else ""
    except Exception:
        app_name = ""
    registry = event.type.startswith("object:children-changed") and event.source.get_role() == Atspi.Role.DESKTOP_FRAME
    if wanted not in app_name.lower() and not registry:
        return
    child = describe(event.any_data) if isinstance(event.any_data, Atspi.Accessible) else ""
    print(f"{time.time():.3f} event {event.type} d1={event.detail1} {app_name} {describe(event.source)} {child}", flush=True)


listener = Atspi.EventListener.new(on)
for kind in ["window:", "object:children-changed", "object:state-changed", "focus:", "object:property-change:accessible-name"]:
    listener.register(kind)
GLib.timeout_add(int(float(sys.argv[2]) * 1000), Atspi.event_quit)
Atspi.event_main()
