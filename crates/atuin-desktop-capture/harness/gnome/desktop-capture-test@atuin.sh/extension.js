// Lets the desktop capture test harness focus and list windows in a private GNOME Shell session.
//
// GNOME Shell doesn't offer a way to focus a window from outside, and its Eval method works only
// in unsafe mode, which doesn't have a command-line switch. The harness installs this extension in
// the session's own home directory.
import Gio from 'gi://Gio';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const INTERFACE = `
<node>
  <interface name="sh.atuin.DesktopCaptureTest">
    <method name="Focus">
      <arg type="s" direction="in" name="app"/>
      <arg type="b" direction="out" name="found"/>
    </method>
    <method name="Windows">
      <arg type="s" direction="out" name="windows"/>
    </method>
  </interface>
</node>`;

function matches(window, app) {
    const wanted = app.toLowerCase();
    return [window.get_wm_class(), window.get_wm_class_instance(), window.get_gtk_application_id()]
        .some(name => (name ?? '').toLowerCase().includes(wanted));
}

export default class DesktopCaptureTest extends Extension {
    enable() {
        this._object = Gio.DBusExportedObject.wrapJSObject(INTERFACE, this);
        this._object.export(Gio.DBus.session, '/sh/atuin/DesktopCaptureTest');
        this._name = Gio.bus_own_name_on_connection(
            Gio.DBus.session, 'sh.atuin.DesktopCaptureTest', Gio.BusNameOwnerFlags.NONE, null, null);
        // The shell starts in the overview, which would keep every window from being active.
        if (Main.layoutManager._startingUp)
            this._startup = Main.layoutManager.connect('startup-complete', () => Main.overview.hide());
        else
            Main.overview.hide();
    }

    disable() {
        if (this._startup)
            Main.layoutManager.disconnect(this._startup);
        this._object.unexport();
        Gio.bus_unown_name(this._name);
    }

    Focus(app) {
        const window = global.get_window_actors()
            .map(actor => actor.meta_window)
            .find(window => matches(window, app));
        if (!window)
            return false;
        // A shell element such as a notification can hold the keyboard focus, and the window
        // gets it only once the shell lets go, as after a click on the window.
        Main.overview.hide();
        global.stage.set_key_focus(null);
        window.activate(global.get_current_time());
        return true;
    }

    Windows() {
        return global.get_window_actors()
            .map(actor => actor.meta_window)
            .map(window => `${window.has_focus() ? '*' : ' '} ${window.get_wm_class()} ${JSON.stringify(window.get_title())}`)
            .join('\n');
    }
}
