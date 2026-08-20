"""Publish the application icon as ``_NET_WM_ICON``.

Tk 8.6 only fills in the legacy WM_HINTS icon pixmap on X11, which modern
desktops ignore, so ``wm iconphoto`` leaves the title bar showing a generic
placeholder. Writing the EWMH property directly is what actually puts the mark
in front of the user.

Everything here is best effort: no X11, no library, no artwork or an
unexpected window layout all leave the window exactly as Tk left it.
"""

from __future__ import annotations

import ctypes
import struct
import tkinter as tk
from pathlib import Path

_XA_CARDINAL = 6
_PROP_MODE_REPLACE = 0
_FORMAT_32 = 32

# The chain from the Tk window up to the root is three windows deep at most;
# the bound only exists so a surprising layout cannot spin forever.
_MAX_ANCESTORS = 8

BLOB_NAME = "window-icon.argb"


def apply_window_icon(root: tk.Tk, blob: Path) -> bool:
    """Set ``_NET_WM_ICON`` on the managed window. True when it was written."""
    try:
        payload = blob.read_bytes()
    except OSError:
        return False
    if not payload or len(payload) % 4:
        return False

    try:
        xlib = _load_xlib()
    except OSError:
        return False

    display = xlib.XOpenDisplay(root.winfo_screen().encode("ascii"))
    if not display:
        return False
    try:
        targets = _property_targets(xlib, display, root)
        if not targets:
            return False

        count = len(payload) // 4
        # Format 32 means an array of C longs, which are 64 bit here, so the
        # packed 32-bit values have to be widened before they are handed over.
        values = (ctypes.c_ulong * count)(*struct.unpack(f"<{count}I", payload))
        atom = xlib.XInternAtom(display, b"_NET_WM_ICON", False)
        if not atom:
            return False
        for window in targets:
            xlib.XChangeProperty(
                display,
                window,
                atom,
                _XA_CARDINAL,
                _FORMAT_32,
                _PROP_MODE_REPLACE,
                ctypes.cast(values, ctypes.POINTER(ctypes.c_ubyte)),
                count,
            )
        xlib.XFlush(display)
        return True
    finally:
        xlib.XCloseDisplay(display)


def _property_targets(xlib: ctypes.CDLL, display: int, root: tk.Tk) -> list[int]:
    """Every window between the Tk toplevel and the screen root.

    Tk nests its toplevel inside a wrapper window and the window manager
    reparents that wrapper into a decoration frame, so the window the desktop
    actually reads properties from is somewhere in the middle. Which one it is
    depends on the window manager and on whether reparenting has happened yet,
    so the property is written to the whole chain: the copies that land on the
    windows nobody reads are inert.
    """
    screen_root = xlib.XDefaultRootWindow(display)
    windows: list[int] = []
    window = root.winfo_id()
    for _ in range(_MAX_ANCESTORS):
        if not window or window == screen_root:
            break
        windows.append(window)
        window = _parent_of(xlib, display, window)
    return windows


def _parent_of(xlib: ctypes.CDLL, display: int, window: int) -> int:
    root_return = ctypes.c_ulong()
    parent_return = ctypes.c_ulong()
    children = ctypes.POINTER(ctypes.c_ulong)()
    child_count = ctypes.c_uint()
    status = xlib.XQueryTree(
        display,
        window,
        ctypes.byref(root_return),
        ctypes.byref(parent_return),
        ctypes.byref(children),
        ctypes.byref(child_count),
    )
    if children:
        xlib.XFree(children)
    return parent_return.value if status else 0


def _load_xlib() -> ctypes.CDLL:
    xlib = ctypes.CDLL("libX11.so.6")
    xlib.XOpenDisplay.argtypes = [ctypes.c_char_p]
    xlib.XOpenDisplay.restype = ctypes.c_void_p
    xlib.XCloseDisplay.argtypes = [ctypes.c_void_p]
    xlib.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    xlib.XDefaultRootWindow.restype = ctypes.c_ulong
    xlib.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    xlib.XInternAtom.restype = ctypes.c_ulong
    xlib.XChangeProperty.argtypes = [
        ctypes.c_void_p,
        ctypes.c_ulong,
        ctypes.c_ulong,
        ctypes.c_ulong,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.POINTER(ctypes.c_ubyte),
        ctypes.c_int,
    ]
    xlib.XQueryTree.argtypes = [
        ctypes.c_void_p,
        ctypes.c_ulong,
        ctypes.POINTER(ctypes.c_ulong),
        ctypes.POINTER(ctypes.c_ulong),
        ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)),
        ctypes.POINTER(ctypes.c_uint),
    ]
    xlib.XQueryTree.restype = ctypes.c_int
    xlib.XFree.argtypes = [ctypes.c_void_p]
    xlib.XFlush.argtypes = [ctypes.c_void_p]
    return xlib
