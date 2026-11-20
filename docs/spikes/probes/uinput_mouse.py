#!/usr/bin/env python3
"""Throwaway helper: create a uinput relative mouse and move it. Usage: uinput_mouse.py DX_TOTAL STEP"""
import fcntl, os, struct, sys, time

UI_SET_EVBIT, UI_SET_RELBIT, UI_SET_KEYBIT = 0x40045564, 0x40045566, 0x40045565
UI_DEV_CREATE, UI_DEV_DESTROY = 0x5501, 0x5502
EV_SYN, EV_REL, REL_X, REL_Y = 0, 2, 0, 1


def emit(fd, t, c, v):
    os.write(fd, struct.pack("llHHi", 0, 0, t, c, v))


fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
fcntl.ioctl(fd, UI_SET_EVBIT, EV_REL)
fcntl.ioctl(fd, UI_SET_EVBIT, 1)  # EV_KEY
fcntl.ioctl(fd, UI_SET_KEYBIT, 0x110)  # BTN_LEFT, so libinput treats it as a mouse
fcntl.ioctl(fd, UI_SET_RELBIT, REL_X)
fcntl.ioctl(fd, UI_SET_RELBIT, REL_Y)
os.write(fd, struct.pack("80sHHHHi64i64i64i64i", b"urdevice-probe", 3, 0x1234, 0x5678, 1, 0,
                         *([0] * 256)))
fcntl.ioctl(fd, UI_DEV_CREATE)
time.sleep(1.0)
total, step = int(sys.argv[1]), int(sys.argv[2])
n = abs(total) // step
for _ in range(n):
    emit(fd, EV_REL, REL_X, step if total > 0 else -step)
    emit(fd, EV_SYN, 0, 0)
    time.sleep(0.01)
time.sleep(0.3)
fcntl.ioctl(fd, UI_DEV_DESTROY)
