#!/usr/bin/env python3
"""T10.3 probe: ask the ScreenCast portal for a VIRTUAL source (SourceType 4) and report what happens.

Usage: t10_3_virtual_screencast.py [hold_seconds]
Prints the stream node id, size and position, then the monitors Mutter reports while the session is open.
Throwaway; needs python3-gi.
"""
import sys
import time
from gi.repository import Gio, GLib

BUS = Gio.bus_get_sync(Gio.BusType.SESSION)
NAME = "org.freedesktop.portal.Desktop"
PATH = "/org/freedesktop/portal/desktop"
SC = "org.freedesktop.portal.ScreenCast"
SENDER = BUS.get_unique_name()[1:].replace(".", "_")
LOOP = GLib.MainLoop()
N = [0]


def tok():
    N[0] += 1
    return f"vprobe{int(time.time())}_{N[0]}"


def request(method, build, sig, timeout=60):
    t = tok()
    path = f"/org/freedesktop/portal/desktop/request/{SENDER}/{t}"
    out = {}

    def on_resp(_c, _s, _p, _i, _sig, params):
        out["r"] = params.unpack()
        LOOP.quit()

    sub = BUS.signal_subscribe(NAME, "org.freedesktop.portal.Request", "Response", path, None,
                               Gio.DBusSignalFlags.NONE, on_resp)
    args = build(t)
    t0 = time.time()
    BUS.call_sync(NAME, PATH, SC, method, GLib.Variant(f"({sig})", args), None,
                  Gio.DBusCallFlags.NONE, 30000, None)
    GLib.timeout_add_seconds(timeout, LOOP.quit)
    LOOP.run()
    BUS.signal_unsubscribe(sub)
    print(f"  {method} took {time.time() - t0:.2f} s")
    return out.get("r")


def monitors():
    r = BUS.call_sync("org.gnome.Mutter.DisplayConfig", "/org/gnome/Mutter/DisplayConfig",
                      "org.gnome.Mutter.DisplayConfig", "GetCurrentState", None, None,
                      Gio.DBusCallFlags.NONE, 5000, None).unpack()
    for m in r[1]:
        spec, modes, props = m
        cur = [x[0] for x in modes if x[6].get("is-current")]
        print("  monitor", spec[0], spec[1], "current mode", cur, "is-builtin", props.get("is-builtin"))
    for lm in r[2]:
        print("  logical monitor at", lm[0], lm[1], "scale", lm[2], "primary", lm[4], "monitors", [m[0] for m in lm[5]])


def main():
    hold = int(sys.argv[1]) if len(sys.argv) > 1 else 5
    props = BUS.call_sync(NAME, PATH, "org.freedesktop.DBus.Properties", "GetAll",
                          GLib.Variant("(s)", (SC,)), None, Gio.DBusCallFlags.NONE, 5000, None)
    print("ScreenCast properties:", props.unpack())
    print("monitors before:")
    monitors()

    st = tok()
    r = request("CreateSession", lambda t: ({"handle_token": GLib.Variant("s", t),
                                             "session_handle_token": GLib.Variant("s", st)},),
                "a{sv}")
    print("CreateSession ->", r)
    session = r[1]["session_handle"]

    r = request("SelectSources", lambda t: (session, {"handle_token": GLib.Variant("s", t),
                                                      "types": GLib.Variant("u", 4),  # VIRTUAL
                                                      "cursor_mode": GLib.Variant("u", 2),  # embedded
                                                      "multiple": GLib.Variant("b", False)}),
                "oa{sv}")
    print("SelectSources(types=VIRTUAL) ->", r)

    r = request("Start", lambda t: (session, "", {"handle_token": GLib.Variant("s", t)}), "osa{sv}")
    print("Start ->", r)
    if r and r[0] == 0:
        print("monitors while the session is open:")
        time.sleep(1.5)
        monitors()
        import subprocess, json
        node = r[1]["streams"][0][0]
        for o in json.loads(subprocess.run(["pw-dump"], capture_output=True, text=True).stdout):
            if o.get("id") == node:
                pr = o["info"]["props"]
                print("  pipewire node", node, {k: pr.get(k) for k in
                      ("media.class", "node.name", "node.description", "media.name", "application.name")})
                for f in o["info"]["params"].get("EnumFormat", [])[:4]:
                    print("   EnumFormat", f.get("format"), f.get("size"), f.get("framerate"))
        GLib.timeout_add_seconds(hold, LOOP.quit)
        LOOP.run()
    BUS.call_sync(NAME, session, "org.freedesktop.portal.Session", "Close", None, None,
                  Gio.DBusCallFlags.NONE, 5000, None)
    time.sleep(1)
    print("monitors after close:")
    monitors()


main()
