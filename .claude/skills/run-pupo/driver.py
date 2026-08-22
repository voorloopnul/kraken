#!/usr/bin/env python3
"""Drive the Pupo Qt/QML window from a shell.

Pupo is a frameless Qt Quick window with no accessibility surface and no
scripting port, so this driver talks to it the only way available: raw X11.
It uses ctypes against libX11/libXtst directly and writes PNGs with zlib,
so it needs nothing installed beyond python3 + libX11 + libXtst (both are
already pulled in by Qt's xcb platform plugin).

Everything is stateless -- each command re-finds the window by name and
re-reads its geometry, because the compositor moves frameless windows
around between (and during) runs.

Commands:
  launch [--binary PATH]   start Pupo, wait for its window, print pid + wid
  info                     print window id, absolute position, size
  shot OUT.png             capture the window (works under XWayland)
  click X Y                click at window-relative X,Y
  type TEXT                type ASCII text into the focused item
  key KEYSYM               send one keysym, e.g. Return, Escape, Tab
  quit                     SIGTERM the process that owns the window
  smoke [OUTDIR]           full scripted flow, writes numbered screenshots
"""

import ctypes
import ctypes.util
import os
import struct
import subprocess
import sys
import time
import zlib

WIN_NAME = "Pupo"          # QML `title:` -- the real toplevel
DEFAULT_BINARY = "target/debug/pupo"

IsViewable = 2
ZPixmap = 2
CurrentTime = 0

x11 = ctypes.CDLL(ctypes.util.find_library("X11"))
xtst = ctypes.CDLL(ctypes.util.find_library("Xtst"))


class XImage(ctypes.Structure):
    _fields_ = [
        ("width", ctypes.c_int), ("height", ctypes.c_int),
        ("xoffset", ctypes.c_int), ("format", ctypes.c_int),
        ("data", ctypes.c_void_p), ("byte_order", ctypes.c_int),
        ("bitmap_unit", ctypes.c_int), ("bitmap_bit_order", ctypes.c_int),
        ("bitmap_pad", ctypes.c_int), ("depth", ctypes.c_int),
        ("bytes_per_line", ctypes.c_int), ("bits_per_pixel", ctypes.c_int),
        ("red_mask", ctypes.c_ulong), ("green_mask", ctypes.c_ulong),
        ("blue_mask", ctypes.c_ulong),
    ]


class XWindowAttributes(ctypes.Structure):
    _fields_ = [
        ("x", ctypes.c_int), ("y", ctypes.c_int),
        ("width", ctypes.c_int), ("height", ctypes.c_int),
        ("border_width", ctypes.c_int), ("depth", ctypes.c_int),
        ("visual", ctypes.c_void_p), ("root", ctypes.c_ulong),
        ("class_", ctypes.c_int), ("bit_gravity", ctypes.c_int),
        ("win_gravity", ctypes.c_int), ("backing_store", ctypes.c_int),
        ("backing_planes", ctypes.c_ulong), ("backing_pixel", ctypes.c_ulong),
        ("save_under", ctypes.c_int), ("colormap", ctypes.c_ulong),
        ("map_installed", ctypes.c_int), ("map_state", ctypes.c_int),
        ("all_event_masks", ctypes.c_long), ("your_event_mask", ctypes.c_long),
        ("do_not_propagate_mask", ctypes.c_long),
        ("override_redirect", ctypes.c_int), ("screen", ctypes.c_void_p),
    ]


class XClientMessageEvent(ctypes.Structure):
    _fields_ = [
        ("type", ctypes.c_int), ("serial", ctypes.c_ulong),
        ("send_event", ctypes.c_int), ("display", ctypes.c_void_p),
        ("window", ctypes.c_ulong), ("message_type", ctypes.c_ulong),
        ("format", ctypes.c_int), ("data", ctypes.c_long * 5),
    ]


class XEvent(ctypes.Union):
    _fields_ = [("type", ctypes.c_int), ("xclient", XClientMessageEvent),
                ("pad", ctypes.c_long * 24)]


x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XGetImage.restype = ctypes.POINTER(XImage)
x11.XRootWindow.restype = ctypes.c_ulong
x11.XInternAtom.restype = ctypes.c_ulong
x11.XStringToKeysym.restype = ctypes.c_ulong
x11.XKeysymToKeycode.restype = ctypes.c_ubyte


def open_display():
    dpy = x11.XOpenDisplay(None)
    if not dpy:
        sys.exit("cannot open DISPLAY -- is an X server (or XWayland) running?")
    return ctypes.c_void_p(dpy)


def find_window(dpy):
    """Return the real toplevel. Qt also creates a 10x10 unmapped 'pupo'
    group-leader window, so filter on mapped + big."""
    root = x11.XRootWindow(dpy, 0)
    r, p = ctypes.c_ulong(), ctypes.c_ulong()
    kids = ctypes.POINTER(ctypes.c_ulong)()
    n = ctypes.c_uint()
    x11.XQueryTree(dpy, ctypes.c_ulong(root), ctypes.byref(r), ctypes.byref(p),
                   ctypes.byref(kids), ctypes.byref(n))
    for i in range(n.value):
        w = kids[i]
        name = ctypes.c_char_p()
        if not x11.XFetchName(dpy, ctypes.c_ulong(w), ctypes.byref(name)):
            continue
        if not name.value or name.value.decode(errors="replace") != WIN_NAME:
            continue
        a = XWindowAttributes()
        x11.XGetWindowAttributes(dpy, ctypes.c_ulong(w), ctypes.byref(a))
        if a.map_state == IsViewable and a.width > 200 and a.height > 200:
            return w
    return None


def require_window(dpy):
    w = find_window(dpy)
    if w is None:
        sys.exit(f"no mapped '{WIN_NAME}' window -- run `driver.py launch` first")
    return w


def geometry(dpy, win):
    a = XWindowAttributes()
    x11.XGetWindowAttributes(dpy, ctypes.c_ulong(win), ctypes.byref(a))
    ax, ay, child = ctypes.c_int(), ctypes.c_int(), ctypes.c_ulong()
    root = x11.XRootWindow(dpy, 0)
    x11.XTranslateCoordinates(dpy, ctypes.c_ulong(win), ctypes.c_ulong(root),
                              0, 0, ctypes.byref(ax), ctypes.byref(ay),
                              ctypes.byref(child))
    return ax.value, ay.value, a.width, a.height


def activate(dpy, win):
    """Ask the WM to raise+focus. XRaiseWindow alone is ignored by mutter for
    managed windows; the EWMH _NET_ACTIVE_WINDOW message is honoured."""
    root = x11.XRootWindow(dpy, 0)
    ev = XEvent()
    ev.xclient.type = 33  # ClientMessage
    ev.xclient.send_event = 1
    ev.xclient.window = win
    ev.xclient.message_type = x11.XInternAtom(dpy, b"_NET_ACTIVE_WINDOW", False)
    ev.xclient.format = 32
    ev.xclient.data[0] = 2  # source: pager
    ev.xclient.data[1] = CurrentTime
    # SubstructureRedirectMask | SubstructureNotifyMask
    x11.XSendEvent(dpy, ctypes.c_ulong(root), False, ctypes.c_long(1 << 20 | 1 << 19),
                   ctypes.byref(ev))
    x11.XRaiseWindow(dpy, ctypes.c_ulong(win))
    x11.XSetInputFocus(dpy, ctypes.c_ulong(win), 2, CurrentTime)
    x11.XFlush(dpy)
    time.sleep(0.4)


def focused_window(dpy):
    w, rev = ctypes.c_ulong(), ctypes.c_int()
    x11.XGetInputFocus(dpy, ctypes.byref(w), ctypes.byref(rev))
    return w.value


def is_or_contains(dpy, ancestor, w):
    """XGetInputFocus can name a child of our toplevel; walk up to compare."""
    root = x11.XRootWindow(dpy, 0)
    seen = 0
    while w and w != root and seen < 16:
        if w == ancestor:
            return True
        r, parent = ctypes.c_ulong(), ctypes.c_ulong()
        kids = ctypes.POINTER(ctypes.c_ulong)()
        n = ctypes.c_uint()
        if not x11.XQueryTree(dpy, ctypes.c_ulong(w), ctypes.byref(r),
                              ctypes.byref(parent), ctypes.byref(kids), ctypes.byref(n)):
            return False
        w = parent.value
        seen += 1
    return False


def ensure_focus(dpy, win, timeout=5.0):
    """Keyboard input is delivered to whatever X says is focused, NOT to the
    window we clicked. Right after launch mutter can sit on the activation
    request for a second or two, so poll until focus really lands."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        if is_or_contains(dpy, win, focused_window(dpy)):
            return
        activate(dpy, win)
    sys.exit("Pupo never took keyboard focus -- another window is holding it, "
             "or the WM refused the activation request")


def pointer(dpy):
    root = x11.XRootWindow(dpy, 0)
    rr, cr = ctypes.c_ulong(), ctypes.c_ulong()
    rx, ry, wx, wy = (ctypes.c_int() for _ in range(4))
    mask = ctypes.c_uint()
    x11.XQueryPointer(dpy, ctypes.c_ulong(root), ctypes.byref(rr), ctypes.byref(cr),
                      ctypes.byref(rx), ctypes.byref(ry), ctypes.byref(wx),
                      ctypes.byref(wy), ctypes.byref(mask))
    return rx.value, ry.value


def write_png(path, width, height, stride, raw, depth):
    buf = bytearray(raw)
    buf[0::4], buf[2::4] = buf[2::4], buf[0::4]   # BGRA -> RGBA
    if depth == 24:
        buf[3::4] = b"\xff" * (len(buf) // 4)     # depth-24 alpha byte is junk
    rows = b"".join(b"\x00" + bytes(buf[y * stride:y * stride + width * 4])
                    for y in range(height))

    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data +
                struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))

    png = (b"\x89PNG\r\n\x1a\n" +
           chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)) +
           chunk(b"IDAT", zlib.compress(rows, 6)) + chunk(b"IEND", b""))
    with open(path, "wb") as fh:
        fh.write(png)


def cmd_launch(args):
    binary = DEFAULT_BINARY
    if "--binary" in args:
        binary = args[args.index("--binary") + 1]
    env = dict(os.environ)
    # xcb, not wayland: XGetImage/XTEST only reach X11 clients, and this box
    # has no wayland screenshot tool (no grim/gnome-screenshot).
    env["QT_QPA_PLATFORM"] = "xcb"
    proc = subprocess.Popen([binary], env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    dpy = open_display()
    for _ in range(100):
        time.sleep(0.2)
        if proc.poll() is not None:
            sys.exit(f"{binary} exited with {proc.returncode} before mapping a window")
        w = find_window(dpy)
        if w is not None:
            ensure_focus(dpy, w)
            x, y, cw, ch = geometry(dpy, w)
            print(f"pid={proc.pid} wid=0x{w:x} geometry={cw}x{ch}+{x}+{y}")
            return
    sys.exit("timed out waiting for the Pupo window to map")


def cmd_info(args):
    dpy = open_display()
    w = require_window(dpy)
    x, y, cw, ch = geometry(dpy, w)
    print(f"wid=0x{w:x} geometry={cw}x{ch}+{x}+{y}")


def cmd_shot(args):
    out = args[0]
    dpy = open_display()
    w = require_window(dpy)
    _, _, cw, ch = geometry(dpy, w)
    img = x11.XGetImage(dpy, ctypes.c_ulong(w), 0, 0, ctypes.c_uint(cw),
                        ctypes.c_uint(ch), ctypes.c_ulong(0xFFFFFFFF), ZPixmap)
    if not img:
        sys.exit("XGetImage failed")
    im = img.contents
    raw = ctypes.string_at(im.data, im.bytes_per_line * im.height)
    write_png(out, im.width, im.height, im.bytes_per_line, raw, im.depth)
    print(f"{out} {im.width}x{im.height} depth={im.depth}")


def cmd_click(args):
    rx, ry = int(args[0]), int(args[1])
    dpy = open_display()
    w = require_window(dpy)
    ensure_focus(dpy, w)
    # Under XWayland the first press after a launch is spent activating the
    # surface rather than reaching the control under it, so warm the window up
    # by moving the pointer into it and letting the compositor catch up. Without
    # this the first scripted click of a run is silently swallowed.
    wx, wy, ww, wh = geometry(dpy, w)
    xtst.XTestFakeMotionEvent(dpy, -1, wx + ww // 2, wy + wh // 2, 0)
    x11.XFlush(dpy); time.sleep(0.15)
    x, y, _, _ = geometry(dpy, w)   # re-read: activating can move the window
    saved = pointer(dpy)
    xtst.XTestFakeMotionEvent(dpy, -1, x + rx, y + ry, 0)
    x11.XFlush(dpy); time.sleep(0.25)
    xtst.XTestFakeButtonEvent(dpy, 1, True, 0)
    x11.XFlush(dpy); time.sleep(0.08)
    xtst.XTestFakeButtonEvent(dpy, 1, False, 0)
    x11.XFlush(dpy); time.sleep(0.5)
    xtst.XTestFakeMotionEvent(dpy, -1, saved[0], saved[1], 0)  # put the user's cursor back
    x11.XFlush(dpy)
    print(f"clicked window({rx},{ry}) = screen({x + rx},{y + ry})")


def send_keysym(dpy, name, shift=False):
    ks = x11.XStringToKeysym(name.encode())
    if ks == 0:
        sys.exit(f"unknown keysym: {name}")
    kc = x11.XKeysymToKeycode(dpy, ctypes.c_ulong(ks))
    if kc == 0:
        sys.exit(f"keysym {name} is not on the current keymap")
    shift_kc = x11.XKeysymToKeycode(dpy, ctypes.c_ulong(x11.XStringToKeysym(b"Shift_L")))
    if shift:
        xtst.XTestFakeKeyEvent(dpy, shift_kc, True, 0)
    xtst.XTestFakeKeyEvent(dpy, kc, True, 0)
    xtst.XTestFakeKeyEvent(dpy, kc, False, 0)
    if shift:
        xtst.XTestFakeKeyEvent(dpy, shift_kc, False, 0)
    x11.XFlush(dpy)
    time.sleep(0.03)


ASCII_KEYSYM = {
    " ": "space", "!": "exclam", '"': "quotedbl", "#": "numbersign",
    "$": "dollar", "%": "percent", "&": "ampersand", "'": "apostrophe",
    "(": "parenleft", ")": "parenright", "*": "asterisk", "+": "plus",
    ",": "comma", "-": "minus", ".": "period", "/": "slash",
    ":": "colon", ";": "semicolon", "<": "less", "=": "equal",
    ">": "greater", "?": "question", "@": "at", "[": "bracketleft",
    "\\": "backslash", "]": "bracketright", "^": "asciicircum",
    "_": "underscore", "`": "grave", "{": "braceleft", "|": "bar",
    "}": "braceright", "~": "asciitilde",
}
SHIFTED = set('!"#$%&()*+:<>?@^_{|}~')


def cmd_type(args):
    text = " ".join(args)
    dpy = open_display()
    ensure_focus(dpy, require_window(dpy))
    for ch in text:
        if ch.isalnum():
            send_keysym(dpy, ch.lower() if ch.isalpha() else ch, shift=ch.isupper())
        else:
            send_keysym(dpy, ASCII_KEYSYM.get(ch, ch), shift=ch in SHIFTED)
    time.sleep(0.3)
    print(f"typed {text!r}")


def cmd_key(args):
    dpy = open_display()
    ensure_focus(dpy, require_window(dpy))
    send_keysym(dpy, args[0])
    time.sleep(0.4)
    print(f"key {args[0]}")


def cmd_quit(args):
    dpy = open_display()
    w = find_window(dpy)
    if w is None:
        print("no Pupo window; nothing to quit")
        return
    prop = x11.XInternAtom(dpy, b"_NET_WM_PID", False)
    actual_type, actual_fmt = ctypes.c_ulong(), ctypes.c_int()
    nitems, bytes_after = ctypes.c_ulong(), ctypes.c_ulong()
    data = ctypes.POINTER(ctypes.c_ubyte)()
    x11.XGetWindowProperty(dpy, ctypes.c_ulong(w), ctypes.c_ulong(prop), 0, 1, False,
                           ctypes.c_ulong(6),  # XA_CARDINAL
                           ctypes.byref(actual_type), ctypes.byref(actual_fmt),
                           ctypes.byref(nitems), ctypes.byref(bytes_after),
                           ctypes.byref(data))
    if not nitems.value:
        sys.exit("window has no _NET_WM_PID")
    pid = ctypes.cast(data, ctypes.POINTER(ctypes.c_uint32)).contents.value
    os.kill(pid, 15)
    print(f"sent SIGTERM to pid {pid}")


def cmd_smoke(args):
    out = args[0] if args else "."
    os.makedirs(out, exist_ok=True)
    steps = [
        ("launch",  lambda: cmd_launch([])),
        (None,      lambda: cmd_shot([f"{out}/01-startup.png"])),
        ("new session", lambda: cmd_click(["190", "63"])),
        (None,      lambda: cmd_shot([f"{out}/02-new-session.png"])),
        ("composer", lambda: cmd_click(["400", "820"])),
        (None,      lambda: cmd_type(["hello", "from", "the", "driver"])),
        (None,      lambda: cmd_shot([f"{out}/03-typed.png"])),
        ("submit",  lambda: cmd_key(["Return"])),
        (None,      lambda: cmd_shot([f"{out}/04-sent.png"])),
        ("dark mode", lambda: cmd_click(["20", "806"])),
        (None,      lambda: cmd_shot([f"{out}/05-dark.png"])),
    ]
    for label, fn in steps:
        if label:
            print(f"--- {label}")
        fn()
    print(f"\nsmoke complete -- screenshots in {out}/")


COMMANDS = {"launch": cmd_launch, "info": cmd_info, "shot": cmd_shot,
            "click": cmd_click, "type": cmd_type, "key": cmd_key,
            "quit": cmd_quit, "smoke": cmd_smoke}

if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in COMMANDS:
        sys.exit(__doc__)
    COMMANDS[sys.argv[1]](sys.argv[2:])
