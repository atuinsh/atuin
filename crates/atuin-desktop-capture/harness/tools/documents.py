"""Lists every document in every window of the named applications, with state and text size."""
import sys, gi
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi
def walk(o, depth=0):
    try: r = o.get_role()
    except Exception: return
    if r == Atspi.Role.DOCUMENT_WEB:
        st = o.get_state_set()
        n = [0, 0]
        def size(x, d=0):
            if d > 60: return
            n[0] += 1
            try:
                t = x.get_text_iface()
                if t: n[1] += t.get_character_count()
                for i in range(x.get_child_count()): size(x.get_child_at_index(i), d+1)
            except Exception: pass
        size(o)
        print(f"   doc {o.get_name()[:30]!r:34} showing={st.contains(Atspi.StateType.SHOWING)} visible={st.contains(Atspi.StateType.VISIBLE)} nodes={n[0]} chars={n[1]}")
        return
    if depth > 40: return
    for i in range(o.get_child_count()):
        walk(o.get_child_at_index(i), depth+1)
d = Atspi.get_desktop(0)
for i in range(d.get_child_count()):
    a = d.get_child_at_index(i)
    if a is None or a.get_name() not in sys.argv[1:]: continue
    for j in range(a.get_child_count()):
        w = a.get_child_at_index(j); print(a.get_name(), "window", repr(w.get_name()[:40]))
        walk(w)
