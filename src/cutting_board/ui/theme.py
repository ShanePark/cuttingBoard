from __future__ import annotations

import subprocess
import sys
import tkinter as tk
from collections.abc import Callable
from dataclasses import dataclass
from tkinter import font as tkfont
from tkinter import ttk

THEME_MODES = ("dark", "light", "system")


@dataclass(frozen=True, slots=True)
class Palette:
    canvas: str
    surface: str
    surface_alt: str
    surface_hover: str
    hairline: str
    border: str
    text: str
    text_muted: str
    text_dim: str
    accent: str
    accent_dim: str
    accent_hover: str
    on_accent: str
    violet: str
    danger: str
    danger_dim: str
    ok: str
    warning: str
    category_colors: tuple[tuple[str, str], ...]


DARK_PALETTE = Palette(
    canvas="#0E0E11",
    surface="#1C1C1E",
    surface_alt="#2C2C2E",
    surface_hover="#343438",
    hairline="#38383A",
    border="#48484A",
    text="#F5F5F7",
    text_muted="#C7C7CC",
    text_dim="#9EA0A5",
    accent="#60ADFF",
    accent_dim="#173A5E",
    accent_hover="#88C3FF",
    on_accent="#0E0E11",
    violet="#D38BFF",
    danger="#FF756E",
    danger_dim="#5C2422",
    ok="#30D158",
    warning="#FFD60A",
    category_colors=(
        ("web", "#60ADFF"),
        ("api", "#D38BFF"),
        ("database", "#85B8FF"),
        ("cache", "#FF9F5A"),
        ("proxy", "#30D158"),
        ("runtime", "#9EA0A5"),
        ("other", "#9EA0A5"),
    ),
)

LIGHT_PALETTE = Palette(
    canvas="#F2F2F7",
    surface="#FFFFFF",
    surface_alt="#E9E9EE",
    surface_hover="#E5E5EA",
    hairline="#D1D1D6",
    border="#C7C7CC",
    text="#1C1C1E",
    text_muted="#3A3A3C",
    text_dim="#636366",
    accent="#005EB8",
    accent_dim="#D9ECFF",
    accent_hover="#0053A6",
    on_accent="#FFFFFF",
    violet="#783399",
    danger="#C81D25",
    danger_dim="#FCE8E7",
    ok="#1B6F30",
    warning="#8A5100",
    category_colors=(
        ("web", "#005EB8"),
        ("api", "#783399"),
        ("database", "#1D4ED8"),
        ("cache", "#9A350E"),
        ("proxy", "#1B6F30"),
        ("runtime", "#475569"),
        ("other", "#636366"),
    ),
)

# Import-time defaults preserve the existing dark appearance. apply_palette()
# replaces the whole semantic set before widgets are constructed, so switching
# palettes never leaves one colour behind from an earlier test or window.
CANVAS = DARK_PALETTE.canvas
SURFACE = DARK_PALETTE.surface
SURFACE_ALT = DARK_PALETTE.surface_alt
SURFACE_HOVER = DARK_PALETTE.surface_hover
HAIRLINE = DARK_PALETTE.hairline
BORDER = DARK_PALETTE.border
TEXT = DARK_PALETTE.text
TEXT_MUTED = DARK_PALETTE.text_muted
TEXT_DIM = DARK_PALETTE.text_dim
ACCENT = DARK_PALETTE.accent
ACCENT_DIM = DARK_PALETTE.accent_dim
ACCENT_HOVER = DARK_PALETTE.accent_hover
ON_ACCENT = DARK_PALETTE.on_accent
VIOLET = DARK_PALETTE.violet
DANGER = DARK_PALETTE.danger
DANGER_DIM = DARK_PALETTE.danger_dim
OK = DARK_PALETTE.ok
WARNING = DARK_PALETTE.warning
CATEGORY_COLORS = dict(DARK_PALETTE.category_colors)
CURRENT_THEME = "dark"

# A compact spatial scale keeps every surface aligned while leaving enough
# breathing room for macOS typography and pointer targets.
SPACE_XS = 4
SPACE_SM = 8
SPACE_MD = 12
SPACE_LG = 16
CARD_RADIUS = 16
ICON_WELL_SIZE = 56
ICON_WELL_RADIUS = 14
CONTROL_SIZE = 30
CONTROL_RADIUS = CONTROL_SIZE / 2
CONTROL_HIT_SIZE = 36
CONTROL_ICON_SIZE = 18

# Cards are wider and shorter than the original logo-first tiles. Their
# generous corners and inset icon well retain that visual identity without
# sacrificing the status and port information developers scan for.
TILE_WIDTH = 268
# The visible card is 124 px high inside a 136 px grid cell. This leaves
# distinct rows for identity, status, ports, and quiet actions without
# returning to the oversized 180 px logo tiles.
TILE_HEIGHT = 120
TILE_PAD = SPACE_SM
TILE_SPAN = TILE_WIDTH + TILE_PAD * 2
GRID_GUTTER = 6


def apply_palette(
    mode: str,
    *,
    platform_name: str | None = None,
    system_style_reader: Callable[[], str | None] | None = None,
) -> str:
    """Apply a complete semantic palette and return ``dark`` or ``light``.

    Call this before constructing widgets. ``system`` follows the macOS
    appearance when it can be read and deliberately falls back to dark on
    unsupported platforms or detection failures.
    """
    resolved = resolve_theme_mode(
        mode,
        platform_name=platform_name,
        system_style_reader=system_style_reader,
    )
    palette = LIGHT_PALETTE if resolved == "light" else DARK_PALETTE

    global CANVAS, SURFACE, SURFACE_ALT, SURFACE_HOVER, HAIRLINE, BORDER
    global TEXT, TEXT_MUTED, TEXT_DIM
    global ACCENT, ACCENT_DIM, ACCENT_HOVER, ON_ACCENT
    global VIOLET, DANGER, DANGER_DIM, OK, WARNING
    global CATEGORY_COLORS, CURRENT_THEME

    (
        CANVAS,
        SURFACE,
        SURFACE_ALT,
        SURFACE_HOVER,
        HAIRLINE,
        BORDER,
        TEXT,
        TEXT_MUTED,
        TEXT_DIM,
        ACCENT,
        ACCENT_DIM,
        ACCENT_HOVER,
        ON_ACCENT,
        VIOLET,
        DANGER,
        DANGER_DIM,
        OK,
        WARNING,
    ) = (
        palette.canvas,
        palette.surface,
        palette.surface_alt,
        palette.surface_hover,
        palette.hairline,
        palette.border,
        palette.text,
        palette.text_muted,
        palette.text_dim,
        palette.accent,
        palette.accent_dim,
        palette.accent_hover,
        palette.on_accent,
        palette.violet,
        palette.danger,
        palette.danger_dim,
        palette.ok,
        palette.warning,
    )
    CATEGORY_COLORS = dict(palette.category_colors)
    CURRENT_THEME = resolved
    return resolved


def resolve_theme_mode(
    mode: str,
    *,
    platform_name: str | None = None,
    system_style_reader: Callable[[], str | None] | None = None,
) -> str:
    """Resolve a saved preference to the concrete palette name."""
    normalized = mode if mode in THEME_MODES else "dark"
    if normalized != "system":
        return normalized
    if (platform_name or sys.platform) != "darwin":
        return "dark"
    reader = system_style_reader or _read_macos_interface_style
    try:
        detected = reader()
    except (OSError, subprocess.SubprocessError):
        return "dark"
    return detected if detected in {"dark", "light"} else "dark"


def _read_macos_interface_style() -> str | None:
    """Read the global macOS appearance without adding a native dependency."""
    completed = subprocess.run(
        ("defaults", "read", "-g"),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=1.0,
        check=False,
    )
    if completed.returncode != 0:
        return None
    lowered = completed.stdout.casefold()
    if "appleinterfacestyle" not in lowered:
        return "light"
    return "dark" if "dark" in lowered else None


def configure_theme(root: tk.Tk) -> dict[str, tuple[str, int, str]]:
    style = ttk.Style(root)
    try:
        style.theme_use("clam")
    except tk.TclError:
        pass

    family = _preferred_font_family(root)
    mono = _preferred_mono_family(root)
    fonts = _font_specs(family, mono)

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


def _font_specs(
    family: str,
    mono: str,
) -> dict[str, tuple[str, int, str]]:
    """Return the complete font scale without requiring a live Tk display."""
    return {
        "wordmark": (mono, 14, "bold"),
        "section": (family, 12, "bold"),
        "section_path": (mono, 10, "normal"),
        "tile_name": (family, 12, "bold"),
        "tile_meta": (family, 10, "normal"),
        "chip": (family, 10, "bold"),
        "count": (mono, 10, "normal"),
        "body": (family, 11, "normal"),
        "body_bold": (family, 11, "bold"),
        "small": (family, 10, "normal"),
        "label": (family, 10, "bold"),
        "mono": (mono, 10, "normal"),
        "empty": (family, 13, "normal"),
    }


def _preferred_font_family(root: tk.Tk) -> str:
    available = set(tkfont.families(root))
    for candidate in (
        "SF Pro Text",
        "Apple SD Gothic Neo",
        "Helvetica Neue",
        "Inter",
        "Ubuntu",
        "Noto Sans CJK KR",
        "Noto Sans",
        "DejaVu Sans",
    ):
        if candidate in available:
            return candidate
    return tkfont.nametofont("TkDefaultFont").actual("family")


def _preferred_mono_family(root: tk.Tk) -> str:
    available = set(tkfont.families(root))
    for candidate in (
        "SF Mono",
        "Menlo",
        "JetBrains Mono",
        "Ubuntu Mono",
        "Noto Sans Mono",
        "DejaVu Sans Mono",
    ):
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
