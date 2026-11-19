#!/usr/bin/env python3
"""Take a screenshot of the Linux desktop through the xdg-desktop-portal.

Works on Wayland (GNOME, KDE) where gnome-screenshot and X11 tools cannot.
GNOME may ask once whether to allow screenshots. Usage: linux-shot.py OUT.png
"""
import shutil
import sys
from urllib.parse import unquote, urlparse

from gi.repository import Gio, GLib

out = sys.argv[1] if len(sys.argv) > 1 else "shot.png"
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
loop = GLib.MainLoop()
result = {}


def on_response(_conn, _sender, _path, _iface, _signal, params):
    code, results = params.unpack()
    result["code"] = code
    result["uri"] = results.get("uri")
    loop.quit()


token = "urdevice_shot"
sender = bus.get_unique_name()[1:].replace(".", "_")
handle = f"/org/freedesktop/portal/desktop/request/{sender}/{token}"
bus.signal_subscribe(
    "org.freedesktop.portal.Desktop",
    "org.freedesktop.portal.Request",
    "Response",
    handle,
    None,
    Gio.DBusSignalFlags.NO_MATCH_RULE,
    on_response,
)
bus.call_sync(
    "org.freedesktop.portal.Desktop",
    "/org/freedesktop/portal/desktop",
    "org.freedesktop.portal.Screenshot",
    "Screenshot",
    GLib.Variant("(sa{sv})", ("", {"handle_token": GLib.Variant("s", token),
                                   "interactive": GLib.Variant("b", False)})),
    None,
    Gio.DBusCallFlags.NONE,
    -1,
    None,
)
GLib.timeout_add_seconds(30, loop.quit)
loop.run()
if result.get("code") != 0 or not result.get("uri"):
    sys.exit(f"screenshot failed: {result}")
shutil.move(unquote(urlparse(result["uri"]).path), out)
print(out)
