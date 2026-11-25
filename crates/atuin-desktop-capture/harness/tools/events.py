"""Prints window and state events of every application, with the states of the windows after each."""
import sys, gi, time
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib
t0 = time.time()
def name(o):
    try: return f"{o.get_application().get_name()}:{o.get_role_name()}:{(o.get_name() or '')[:25]}"
    except Exception: return "?"
def on(e):
    print(f"{time.time()-t0:6.2f} {e.type:40} d1={e.detail1} {name(e.source)}", flush=True)
l = Atspi.EventListener.new(on)
for t in (sys.argv[2].split(",") if len(sys.argv) > 2 else ["window:", "object:state-changed:active", "object:state-changed:focused", "focus:"]):
    l.register(t)
def states():
    d = Atspi.get_desktop(0)
    for i in range(d.get_child_count()):
        a = d.get_child_at_index(i)
        if a is None: continue
        for j in range(a.get_child_count()):
            w = a.get_child_at_index(j); st = w.get_state_set()
            print("   state", a.get_name(), repr((w.get_name() or "")[:25]), w.get_role_name(), "active" if st.contains(Atspi.StateType.ACTIVE) else "-", "showing" if st.contains(Atspi.StateType.SHOWING) else "-", flush=True)
    return True
GLib.timeout_add(3000, states)
GLib.timeout_add(int(sys.argv[1]) * 1000, lambda: Atspi.event_quit())
Atspi.event_main()
