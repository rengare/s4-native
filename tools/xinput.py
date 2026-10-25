#!/usr/bin/env python3
"""Synthetic X11 input (XTest) for the original game under Wine/Xwayland.
   tools/xinput.py "key Escape" "sleep 2" "click 631 219" "move 400 300" "drag x0 y0 x1 y1 3 2.0" ...   (coordinates: X root)"""
import ctypes, sys, time
x11 = ctypes.CDLL('libX11.so.6'); xt = ctypes.CDLL('libXtst.so.6')
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XKeysymToKeycode.restype = ctypes.c_ubyte
x11.XStringToKeysym.restype = ctypes.c_ulong
d = x11.XOpenDisplay(None)
assert d, "no X display"
def flush(): x11.XFlush(ctypes.c_void_p(d))
def move(x, y): xt.XTestFakeMotionEvent(ctypes.c_void_p(d), 0, int(x), int(y), 0); flush()
def button(b, down): xt.XTestFakeButtonEvent(ctypes.c_void_p(d), b, down, 0); flush()
for cmd in sys.argv[1:]:
    p = cmd.split()
    if p[0] == 'sleep': time.sleep(float(p[1]))
    elif p[0] == 'move': move(p[1], p[2])
    elif p[0] in ('click', 'rclick'):
        b = 1 if p[0] == 'click' else 3
        move(p[1], p[2]); time.sleep(0.15); button(b, 1); time.sleep(0.15); button(b, 0)
    elif p[0] == 'drag':
        # drag x0 y0 x1 y1 button seconds: press at p0, move linearly to p1 over `seconds`, release
        x0, y0, x1, y1, b, secs = float(p[1]), float(p[2]), float(p[3]), float(p[4]), int(p[5]), float(p[6])
        move(x0, y0); time.sleep(0.15); button(b, 1)
        n = max(1, int(secs / 0.03))
        for i in range(1, n + 1):
            move(x0 + (x1 - x0) * i / n, y0 + (y1 - y0) * i / n); time.sleep(secs / n)
        time.sleep(0.2); button(b, 0)
    elif p[0] == 'pos':
        # pos: print the pointer's root coordinates (x y)
        r, c = ctypes.c_ulong(), ctypes.c_ulong()
        rx, ry, wx, wy, m = ctypes.c_int(), ctypes.c_int(), ctypes.c_int(), ctypes.c_int(), ctypes.c_uint()
        x11.XQueryPointer.restype = ctypes.c_int
        x11.XDefaultRootWindow.restype = ctypes.c_ulong
        root = x11.XDefaultRootWindow(ctypes.c_void_p(d))
        x11.XQueryPointer(ctypes.c_void_p(d), ctypes.c_ulong(root), ctypes.byref(r), ctypes.byref(c), ctypes.byref(rx), ctypes.byref(ry), ctypes.byref(wx), ctypes.byref(wy), ctypes.byref(m))
        print(rx.value, ry.value, flush=True)
    elif p[0] == 'key':
        k = x11.XKeysymToKeycode(ctypes.c_void_p(d), ctypes.c_ulong(x11.XStringToKeysym(p[1].encode())))
        xt.XTestFakeKeyEvent(ctypes.c_void_p(d), k, 1, 0); flush(); time.sleep(0.1)
        xt.XTestFakeKeyEvent(ctypes.c_void_p(d), k, 0, 0); flush()
