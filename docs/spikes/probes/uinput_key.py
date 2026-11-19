#!/usr/bin/env python3
"""Throwaway helper: press keys through uinput. Usage: uinput_key.py KEYCODE [KEYCODE...] (linux input codes; 28=Enter, 15=Tab, 57=Space)"""
import fcntl, os, struct, sys, time

UI_SET_EVBIT, UI_SET_KEYBIT = 0x40045564, 0x40045565
UI_DEV_CREATE, UI_DEV_DESTROY = 0x5501, 0x5502


def emit(fd, t, c, v):
    os.write(fd, struct.pack("llHHi", 0, 0, t, c, v))


fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
fcntl.ioctl(fd, UI_SET_EVBIT, 1)
codes = [int(a) for a in sys.argv[1:]]
for c in sorted(set(codes) | set(range(1, 128))):
    fcntl.ioctl(fd, UI_SET_KEYBIT, c)
os.write(fd, struct.pack("80sHHHHi64i64i64i64i", b"urdevice-key-probe", 3, 0x1234, 0x5679, 1, 0,
                         *([0] * 256)))
fcntl.ioctl(fd, UI_DEV_CREATE)
time.sleep(1.0)
for c in codes:
    emit(fd, 1, c, 1); emit(fd, 0, 0, 0); time.sleep(0.05)
    emit(fd, 1, c, 0); emit(fd, 0, 0, 0); time.sleep(0.3)
time.sleep(0.3)
fcntl.ioctl(fd, UI_DEV_DESTROY)
