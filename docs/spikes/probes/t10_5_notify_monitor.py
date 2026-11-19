#!/usr/bin/env python3
"""T10.5 probe (Linux): become a D-Bus monitor for org.freedesktop.Notifications.Notify.

Usage: t10_5_notify_monitor.py [seconds]
While it runs, send a notification from another shell: notify-send -a probe-app "title" "body".
The probe prints sender, app name, summary, body and actions of each Notify call it sees.
Throwaway; needs python3-gi. Uses a private connection, as dbus-monitor does.
"""
import sys
from gi.repository import Gio, GLib

addr = Gio.dbus_address_get_for_bus_sync(Gio.BusType.SESSION)
conn = Gio.DBusConnection.new_for_address_sync(
    addr,
    Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
    None, None)

seen = []


def on_msg(_conn, msg, incoming):
    if msg.get_message_type() == Gio.DBusMessageType.METHOD_CALL and \
            msg.get_interface() in ("org.gtk.Notifications", "org.freedesktop.portal.Notification"):
        print("OTHER PATH:", msg.get_interface(), msg.get_member(), msg.get_body().unpack())
        seen.append(msg.get_interface())
    if msg.get_message_type() == Gio.DBusMessageType.METHOD_CALL and \
            msg.get_interface() == "org.freedesktop.Notifications" and msg.get_member() == "Notify":
        b = msg.get_body().unpack()
        app, replaces, icon, summary, body, actions, hints, timeout = b
        print("Notify:", {"sender": msg.get_sender(), "app": app, "replaces": replaces, "summary": summary,
                          "body": body, "actions": actions, "hint_keys": sorted(hints)})
        seen.append(app)
    return msg


conn.add_filter(on_msg)
rules = ["type='method_call',interface='org.freedesktop.Notifications',member='Notify'",
         "type='method_call',interface='org.gtk.Notifications'",
         "type='method_call',interface='org.freedesktop.portal.Notification'"]
try:
    conn.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus.Monitoring",
                   "BecomeMonitor", GLib.Variant("(asu)", (rules, 0)), None,
                   Gio.DBusCallFlags.NONE, 5000, None)
    print("BecomeMonitor: ok")
except GLib.Error as e:
    print("BecomeMonitor failed:", e.message)
    sys.exit(1)

loop = GLib.MainLoop()
GLib.timeout_add_seconds(int(sys.argv[1]) if len(sys.argv) > 1 else 8, loop.quit)
loop.run()
print("notifications seen:", len(seen))
