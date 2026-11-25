import gi
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi
d = Atspi.get_desktop(0)
for i in range(d.get_child_count()):
    a = d.get_child_at_index(i)
    if a is None: continue
    for j in range(a.get_child_count()):
        w = a.get_child_at_index(j)
        st = w.get_state_set()
        print("win", a.get_name(), repr(w.get_name()[:30]), w.get_role_name(), "active" if st.contains(Atspi.StateType.ACTIVE) else "-", "showing" if st.contains(Atspi.StateType.SHOWING) else "-", "iconified" if st.contains(Atspi.StateType.ICONIFIED) else "-")
