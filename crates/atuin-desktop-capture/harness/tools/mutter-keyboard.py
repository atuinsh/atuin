"""Gives a headless mutter a keyboard through its RemoteDesktop API, and types text on request.

Usage: mutter-keyboard.py hold <seconds> | mutter-keyboard.py type <text>
'hold' presses Shift once, which makes mutter create the virtual keyboard, and keeps the session."""
import sys, time
from gi.repository import Gio, GLib
bus = Gio.bus_get_sync(Gio.BusType.SESSION)
def proxy(path, iface):
    return Gio.DBusProxy.new_sync(bus, 0, None, "org.gnome.Mutter.RemoteDesktop", path, iface, None)
rd = proxy("/org/gnome/Mutter/RemoteDesktop", "org.gnome.Mutter.RemoteDesktop")
path = rd.call_sync("CreateSession", None, 0, -1, None).unpack()[0]
s = proxy(path, "org.gnome.Mutter.RemoteDesktop.Session")
s.call_sync("Start", None, 0, -1, None)
def key(sym):
    for down in (True, False):
        s.call_sync("NotifyKeyboardKeysym", GLib.Variant("(ub)", (sym, down)), 0, -1, None)
        time.sleep(0.02)
if sys.argv[1] == "hold":
    key(0xffe1)
    time.sleep(float(sys.argv[2]))
else:
    for ch in sys.argv[2]:
        key(0xff0d if ch == "\n" else ord(ch))
s.call_sync("Stop", None, 0, -1, None)
