import Gio from 'gi://Gio';
import Meta from 'gi://Meta';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';

const IFACE = `
<node>
  <interface name="com.tiarun.ShellIntegration">
    <method name="List">
      <arg type="s" direction="out" name="windows_json"/>
    </method>
    <method name="Activate">
      <arg type="t" direction="in" name="id"/>
      <arg type="b" direction="out" name="success"/>
    </method>
  </interface>
</node>`;

export default class TiaRunShellIntegration extends Extension {
    enable() {
        this._dbus = Gio.DBusExportedObject.wrapJSObject(IFACE, this);
        this._dbus.export(Gio.DBus.session, '/com/tiarun/ShellIntegration');
        this._owner = Gio.bus_own_name(
            Gio.BusType.SESSION,
            'com.tiarun.ShellIntegration',
            Gio.BusNameOwnerFlags.NONE,
            null,
            null,
            null,
        );
    }

    disable() {
        if (this._dbus) {
            this._dbus.unexport();
            this._dbus = null;
        }
        if (this._owner) {
            Gio.bus_unown_name(this._owner);
            this._owner = null;
        }
    }

    List() {
        const wins = [];
        for (const actor of global.get_window_actors()) {
            const w = actor.meta_window;
            if (!w || w.get_window_type() !== Meta.WindowType.NORMAL || w.is_skip_taskbar()) {
                continue;
            }
            wins.push({
                id: w.get_id(),
                title: w.get_title() || '',
                wm_class: w.get_wm_class() || '',
                pid: w.get_pid() || 0,
            });
        }
        return JSON.stringify(wins);
    }

    Activate(id) {
        for (const actor of global.get_window_actors()) {
            const w = actor.meta_window;
            if (w && w.get_id() === id) {
                Main.activateWindow(w);
                return true;
            }
        }
        return false;
    }
}
