"""Shows a libadwaita window with a switch row first and two spin rows after it.

SIGUSR1 moves the keyboard focus to the last spin row. GTK 4 leaves spin rows out of the
accessibility tree, and lists them in its cache only once they have had the focus.

Usage: spin-rows.py"""
import signal
import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gio, GLib

GLib.set_prgname("spin-rows")


def spin_row(title, value):
    row = Adw.SpinRow.new_with_range(0, 100, 1)
    row.set_title(title)
    row.set_value(value)
    return row


def activate(app):
    window = Adw.ApplicationWindow(
        application=app, title="Spin rows", default_width=420, default_height=360
    )
    group = Adw.PreferencesGroup(title="Probe settings")
    group.add(Adw.SwitchRow(title="First switch", active=True))
    group.add(spin_row("Second spin", 7))
    third = spin_row("Third spin", 3)
    group.add(third)
    page = Adw.PreferencesPage()
    page.add(group)
    view = Adw.ToolbarView()
    view.add_top_bar(Adw.HeaderBar())
    view.set_content(page)
    window.set_content(view)

    def focus_third():
        third.grab_focus()
        return GLib.SOURCE_REMOVE

    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR1, focus_third)
    window.present()


app = Adw.Application(
    application_id="sh.atuin.DesktopCaptureSpinRows", flags=Gio.ApplicationFlags.NON_UNIQUE
)
app.connect("activate", activate)
sys.exit(app.run([sys.argv[0]]))
