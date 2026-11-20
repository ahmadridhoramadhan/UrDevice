#!/usr/bin/env python3
"""T10.1 probe: walk the org.freedesktop.portal.InputCapture flow on the session bus.

Usage: t10_1_inputcapture.py [--no-start]
Prints one line per step. Needs python3-gi. Throwaway; not part of the build.
"""
import sys
import time
from gi.repository import Gio, GLib

BUS = Gio.bus_get_sync(Gio.BusType.SESSION)
NAME = "org.freedesktop.portal.Desktop"
PATH = "/org/freedesktop/portal/desktop"
IFACE = "org.freedesktop.portal.InputCapture"
SENDER = BUS.get_unique_name()[1:].replace(".", "_")
LOOP = GLib.MainLoop()
N = [0]


def tok():
    N[0] += 1
    return f"probe{int(time.time())}_{N[0]}"


def call(method, args, sig, iface=IFACE):
    return BUS.call_sync(NAME, PATH, iface, method, GLib.Variant(f"({sig})", args),
                         None, Gio.DBusCallFlags.NONE, 30000, None)


def request(method, args_fn, sig, timeout=60):
    """Call a portal method whose last options dict carries handle_token; wait for Response."""
    t = tok()
    path = f"/org/freedesktop/portal/desktop/request/{SENDER}/{t}"
    out = {}

    def on_resp(_c, _s, _p, _i, _sig, params):
        out["r"] = params.unpack()
        LOOP.quit()

    sub = BUS.signal_subscribe(NAME, "org.freedesktop.portal.Request", "Response", path,
                               None, Gio.DBusSignalFlags.NONE, on_resp)
    args, s = args_fn(t)
    t0 = time.time()
    call(method, args, s)
    GLib.timeout_add_seconds(timeout, LOOP.quit)
    LOOP.run()
    print(f"  {method} took {time.time() - t0:.2f} s")
    BUS.signal_unsubscribe(sub)
    return out.get("r")


def main():
    props = BUS.call_sync(NAME, PATH, "org.freedesktop.DBus.Properties", "GetAll",
                          GLib.Variant("(s)", (IFACE,)), None, Gio.DBusCallFlags.NONE, 5000, None)
    print("properties:", props.unpack())

    # capabilities: 1 keyboard, 2 pointer, 4 touchscreen
    st = tok()
    r = request("CreateSession",
                lambda t: (("", {"handle_token": GLib.Variant("s", t),
                                "session_handle_token": GLib.Variant("s", st),
                                "capabilities": GLib.Variant("u", 3)}), "sa{sv}"),
                "sa{sv}")
    print("CreateSession ->", r)
    if not r or r[0] != 0:
        return 1
    session = r[1]["session_handle"]

    r = request("GetZones",
                lambda t: ((session, {"handle_token": GLib.Variant("s", t)}), "oa{sv}"),
                "oa{sv}")
    print("GetZones ->", r)
    if not r or r[0] != 0:
        return 2
    zones = r[1]["zones"]
    zone_set = r[1]["zone_set"]

    # zones: (width, height, x, y). Barrier on the right edge of the first zone.
    w, h, x, y = zones[0]
    bx = x + w - (1 if "--inside" in sys.argv else 0)
    barriers = [{"barrier_id": GLib.Variant("u", 1),
                 "position": GLib.Variant("(iiii)", (bx, y, bx, y + h - 1))}]
    r = request("SetPointerBarriers",
                lambda t: ((session, {"handle_token": GLib.Variant("s", t)}, barriers, zone_set),
                           "oa{sv}aa{sv}u"),
                "oa{sv}aa{sv}u")
    print("SetPointerBarriers ->", r)

    def on_sig(_c, _s, _p, _i, name, params):
        print("signal", name, params.unpack())

    BUS.signal_subscribe(NAME, IFACE, None, PATH, None, Gio.DBusSignalFlags.NONE, on_sig)

    reply = BUS.call_with_unix_fd_list_sync(
        NAME, PATH, IFACE, "ConnectToEIS",
        GLib.Variant("(oa{sv})", (session, {})), GLib.VariantType("(h)"),
        Gio.DBusCallFlags.NONE, 10000, None, None)
    fd_list = reply[1]
    print("ConnectToEIS -> fd ok:", fd_list.get_length() == 1)

    call("Enable", (session, {}), "oa{sv}")
    print("Enable -> ok; waiting 10 s for Activated (move the pointer to the right edge)")
    if "--push" in sys.argv:
        import subprocess, os
        here = os.path.dirname(os.path.abspath(__file__))
        subprocess.Popen([sys.executable, os.path.join(here, "uinput_mouse.py"), "3000", "20"])
    GLib.timeout_add_seconds(10, LOOP.quit)
    LOOP.run()
    call("Disable", (session, {}), "oa{sv}")
    BUS.call_sync(NAME, session, "org.freedesktop.portal.Session", "Close", None, None,
                  Gio.DBusCallFlags.NONE, 5000, None)
    print("closed")
    return 0


sys.exit(main())
