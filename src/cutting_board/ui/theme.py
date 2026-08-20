from __future__ import annotations

import tkinter as tk
from tkinter import font as tkfont
from tkinter import ttk

# A near-black canvas with a single cyan accent. Colour is reserved for the
# brand marks and for state, so the eye lands on the services themselves.
CANVAS = "#0B0E14"
SURFACE = "#111721"
SURFACE_ALT = "#151C28"
HAIRLINE = "#1A212C"
BORDER = "#232C3A"

TEXT = "#E6EDF3"
TEXT_MUTED = "#7D8590"
TEXT_DIM = "#4E5966"

ACCENT = "#22D3EE"
ACCENT_DIM = "#0E7490"
VIOLET = "#A78BFA"
DANGER = "#FB7185"
DANGER_DIM = "#7F1D2E"
OK = "#34D399"
WARNING = "#FBBF24"

# Category tints for the port chips.
CATEGORY_COLORS = {
    "web": "#22D3EE",
    "api": "#A78BFA",
    "database": "#60A5FA",
    "cache": "#FB923C",
    "proxy": "#34D399",
    "runtime": "#94A3B8",
    "other": "#7D8590",
}

TILE_WIDTH = 176
TILE_HEIGHT = 152
TILE_PAD = 14
TILE_SPAN = TILE_WIDTH + TILE_PAD * 2
GRID_GUTTER = 4


def configure_theme(root: tk.Tk) -> dict[str, tuple[str, int, str]]:
    style = ttk.Style(root)
    try:
        style.theme_use("clam")
    except tk.TclError:
        pass

    family = _preferred_font_family(root)
    mono = _preferred_mono_family(root)
    fonts = {
        "wordmark": (mono, 13, "bold"),
        "section": (family, 11, "bold"),
        "section_path": (mono, 8, "normal"),
        "tile_name": (family, 10, "bold"),
        "tile_meta": (mono, 8, "normal"),
        "chip": (mono, 8, "bold"),
        "count": (mono, 9, "normal"),
        "body": (family, 10, "normal"),
        "body_bold": (family, 10, "bold"),
        "small": (family, 9, "normal"),
        "label": (mono, 8, "bold"),
        "mono": (mono, 9, "normal"),
        "empty": (family, 12, "normal"),
    }

    root.configure(background=CANVAS)
    style.configure(
        "Board.Vertical.TScrollbar",
        background=BORDER,
        troughcolor=CANVAS,
        bordercolor=CANVAS,
        arrowcolor=TEXT_DIM,
        darkcolor=BORDER,
        lightcolor=BORDER,
        relief="flat",
    )
    style.map(
        "Board.Vertical.TScrollbar",
        background=[("active", ACCENT_DIM)],
    )
    return fonts


def _preferred_font_family(root: tk.Tk) -> str:
    available = set(tkfont.families(root))
    for candidate in ("Inter", "Ubuntu", "Noto Sans CJK KR", "Noto Sans", "DejaVu Sans"):
        if candidate in available:
            return candidate
    return tkfont.nametofont("TkDefaultFont").actual("family")


def _preferred_mono_family(root: tk.Tk) -> str:
    available = set(tkfont.families(root))
    for candidate in ("JetBrains Mono", "Ubuntu Mono", "Noto Sans Mono", "DejaVu Sans Mono"):
        if candidate in available:
            return candidate
    return tkfont.nametofont("TkFixedFont").actual("family")


def rounded_rect(
    canvas: tk.Canvas,
    x0: float,
    y0: float,
    x1: float,
    y1: float,
    radius: float,
    **options: object,
) -> int:
    """Draw a rounded rectangle as a smoothed polygon.

    Tk has no rounded primitive; a polygon whose corner points are doubled up
    and then smoothed gives a clean approximation at these sizes.
    """
    radius = min(radius, (x1 - x0) / 2, (y1 - y0) / 2)
    points = [
        x0 + radius, y0,
        x1 - radius, y0,
        x1, y0,
        x1, y0 + radius,
        x1, y1 - radius,
        x1, y1,
        x1 - radius, y1,
        x0 + radius, y1,
        x0, y1,
        x0, y1 - radius,
        x0, y0 + radius,
        x0, y0,
    ]
    return canvas.create_polygon(points, smooth=True, splinesteps=16, **options)
