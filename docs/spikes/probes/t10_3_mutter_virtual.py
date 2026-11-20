"""T10.3 probe: Mutter native ScreenCast RecordVirtual (no dialog) plus a PipeWire consumer, to see the virtual monitor appear.
Usage: t10_3_mutter_virtual.py   (needs python3-gi, gstreamer pipewiresrc, GNOME Shell on Wayland). Throwaway."""
import time
from gi.repository import Gio, GLib
B=Gio.bus_get_sync(Gio.BusType.SESSION)
def c(path,iface,m,args=None,sig=None,dest="org.gnome.Mutter.ScreenCast"):
    return B.call_sync(dest,path,iface,m,GLib.Variant(sig,args) if sig else None,None,Gio.DBusCallFlags.NONE,10000,None).unpack()
def mons():
    r=c("/org/gnome/Mutter/DisplayConfig","org.gnome.Mutter.DisplayConfig","GetCurrentState",dest="org.gnome.Mutter.DisplayConfig")
    for lm in r[2]: print("  logical monitor at",lm[0],lm[1],"scale",lm[2],[m[0] for m in lm[5]])
    for m in r[1]:
        cur=[x[0] for x in m[1] if x[6].get("is-current")]
        print("  monitor",m[0][0],"current mode",cur)
print("before"); mons()
s=c("/org/gnome/Mutter/ScreenCast","org.gnome.Mutter.ScreenCast","CreateSession",({},),"(a{sv})")[0]
print("session",s)
st=c(s,"org.gnome.Mutter.ScreenCast.Session","RecordVirtual",({"cursor-mode":GLib.Variant("u",1),"is-platform":GLib.Variant("b",False)},),"(a{sv})")[0]
print("stream",st)
NODE=[None]
def on(_c,_s,_p,_i,name,params):
    NODE[0]=params.unpack()[0]; print("signal",name,params.unpack())
B.signal_subscribe("org.gnome.Mutter.ScreenCast",None,"PipeWireStreamAdded",st,None,Gio.DBusSignalFlags.NONE,on)
c(s,"org.gnome.Mutter.ScreenCast.Session","Start")
import subprocess
ctx0=GLib.MainContext.default(); t0=time.time()
while NODE[0] is None and time.time()-t0<5: ctx0.iteration(False); time.sleep(0.02)
g=subprocess.Popen(["gst-launch-1.0","-q","pipewiresrc","path=%d"%NODE[0],"num-buffers=60","!","video/x-raw,width=1024,height=768","!","fakesink","sync=false"],stderr=subprocess.PIPE,text=True)
ctx=GLib.MainContext.default(); t=time.time()
while time.time()-t<4: ctx.iteration(False); time.sleep(0.05)
print("while recording"); mons()
import subprocess, json
for o in json.loads(subprocess.run(["pw-dump"],capture_output=True,text=True).stdout):
    if o.get("id")==NODE[0] or (o.get("info",{}).get("props",{}).get("node.name","").startswith("mutter")):
        pr=o["info"]["props"]; print("  node",o["id"],{k:pr.get(k) for k in ("media.class","node.name","media.name","application.name")})
        for f in o["info"]["params"].get("EnumFormat",[])[:3]: print("   EnumFormat",f.get("format"),f.get("size"),f.get("framerate"))

print("gst rc", g.poll()); g.terminate(); print(g.stderr.read()[:300]); c(s,"org.gnome.Mutter.ScreenCast.Session","Stop"); time.sleep(1); print("after stop"); mons()
