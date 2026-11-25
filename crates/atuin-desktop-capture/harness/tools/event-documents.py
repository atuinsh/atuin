"""Attributes an application's content events to (window, nearest document, document showing)."""
import sys, gi, collections
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib
APP = sys.argv[1]
count = collections.Counter()
def on(e):
    try:
        if (e.source.get_application().get_name() or "") != APP: return
        o = e.source; doc = None
        while True:
            p = o.get_parent()
            if p is None or p.get_role() == Atspi.Role.APPLICATION: break
            if doc is None and o.get_role() == Atspi.Role.DOCUMENT_WEB: doc = o
            o = p
        win = (o.get_name() or "?")[:18]
        if doc is None: key = (win, "-", "outside documents")
        else:
            showing = doc.get_state_set().contains(Atspi.StateType.SHOWING)
            key = (win, (doc.get_name() or "?")[:18], "showing" if showing else "hidden")
    except Exception: return
    count[key] += 1
l = Atspi.EventListener.new(on)
for t in ["object:children-changed", "object:state-changed:showing", "object:text-changed", "document:load-complete"]:
    l.register(t)
def done():
    for k, v in count.most_common(): print("doc", v, k)
    Atspi.event_quit()
GLib.timeout_add(int(sys.argv[2]) * 1000, done)
Atspi.event_main()
